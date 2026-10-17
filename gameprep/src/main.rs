//! GamePrep window + background engine.
//!
//! The engine runs on its own thread (detection keeps working while the
//! window is minimized, and closing apps - which waits up to a few
//! seconds per app - never freezes the UI). Both sides share one
//! `Shared` behind a mutex; the UI sends one-off commands over a channel.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use eframe::egui;
use gameprep::config::{self, normalize, Config};
use gameprep::engine::{self, Action, Engine, ProcView};
use gameprep::{endsession, icon, procs, protect, tweaks};
use route_agent::{agentlog, windiag};

const TICK: Duration = Duration::from_secs(2);
const CLOSE_GRACE: Duration = Duration::from_secs(5);
const MAX_EVENTS: usize = 200;
/// Running apps below this aren't listed (unless already selected) - keeps
/// the Apps tab to things worth closing.
const LIST_MIN_MB: f64 = 40.0;

#[derive(Clone)]
struct AppRow {
    display: String,
    key: String,
    count: usize,
    mb: f64,
}

#[derive(Clone, Default)]
struct Session {
    game: String,
    closed: usize,
    freed_mb: f64,
}

struct Shared {
    cfg: Config,
    cfg_path: PathBuf,
    rows: Vec<AppRow>,
    session: Option<Session>,
    events: VecDeque<String>,
    busy: bool,
}

impl Shared {
    fn event(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        agentlog::info(&msg);
        self.events
            .push_front(format!("{}  {msg}", tweaks::local_hms()));
        self.events.truncate(MAX_EVENTS);
    }

    fn save(&mut self) {
        self.cfg.tidy();
        if let Err(e) = config::save(&self.cfg_path, &self.cfg) {
            self.event(format!("erro ao salvar a configuração: {e}"));
        }
    }
}

fn lock(s: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

enum Cmd {
    OptimizeNow,
}

fn gb(mb: f64) -> String {
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// Running, closable processes grouped by app, heaviest first.
fn group(procs: &[ProcView], cfg: &Config) -> Vec<AppRow> {
    let mut by: HashMap<String, AppRow> = HashMap::new();
    for p in procs {
        let key = normalize(&p.name);
        if protect::is_protected(&key) || cfg.is_game(&key) {
            continue;
        }
        let row = by.entry(key.clone()).or_insert_with(|| AppRow {
            display: p.name.clone(),
            key,
            count: 0,
            mb: 0.0,
        });
        row.count += 1;
        row.mb += p.working_set as f64 / 1_048_576.0;
    }
    let mut rows: Vec<AppRow> = by
        .into_values()
        .filter(|r| r.mb >= LIST_MIN_MB || cfg.is_app(&r.key))
        .collect();
    rows.sort_by(|a, b| b.mb.total_cmp(&a.mb));
    rows
}

fn execute(action: Action, shared: &Mutex<Shared>, engine: &mut Engine) {
    match action {
        Action::Log(m) => lock(shared).event(m),
        Action::Boost(pid) => {
            let ok = procs::boost(pid);
            lock(shared).event(if ok {
                "prioridade do jogo: acima do normal".to_string()
            } else {
                "não consegui mudar a prioridade do jogo".to_string()
            });
        }
        Action::Close { names, for_game } => {
            lock(shared).busy = true;
            let r = procs::close_apps(&names, CLOSE_GRACE);
            let mut s = lock(shared);
            s.busy = false;
            if r.processes == 0 {
                s.event("nada para fechar (os apps já tinham fechado)");
                return;
            }
            let apps: Vec<&str> = r.closed.iter().map(|c| c.name.as_str()).collect();
            s.event(format!(
                "fechados: {} ({} processos, {} liberados)",
                apps.join(", "),
                r.processes,
                gb(r.freed_mb)
            ));
            if !r.forced.is_empty() {
                s.event(format!(
                    "encerrados à força (não fecharam sozinhos em {} s): {}",
                    CLOSE_GRACE.as_secs(),
                    r.forced.join(", ")
                ));
            }
            if !r.failed.is_empty() {
                s.event(format!("não consegui fechar: {}", r.failed.join(", ")));
            }
            if for_game {
                if let Some(sess) = s.session.as_mut() {
                    sess.closed += r.closed.len();
                    sess.freed_mb += r.freed_mb;
                }
                drop(s);
                engine.record_closed(r.closed);
            }
        }
        Action::Relaunch(apps) => {
            for app in apps {
                let msg = match &app.path {
                    Some(p) => match procs::relaunch(p) {
                        Ok(()) => format!("reaberto: {}", app.name),
                        Err(e) => format!("não consegui reabrir {}: {e}", app.name),
                    },
                    None => format!("não consegui reabrir {} (caminho desconhecido)", app.name),
                };
                lock(shared).event(msg);
            }
        }
    }
}

fn engine_loop(shared: Arc<Mutex<Shared>>, rx: mpsc::Receiver<Cmd>, ctx: egui::Context) {
    let mut engine = Engine::new();
    loop {
        let list = procs::list();
        let cfg = lock(&shared).cfg.clone();
        let actions = engine.tick(&list, &cfg);
        {
            let mut s = lock(&shared);
            s.rows = group(&list, &cfg);
            match engine.current_game() {
                Some(g) if s.session.as_ref().map(|x| x.game.as_str()) != Some(g) => {
                    s.session = Some(Session {
                        game: g.to_string(),
                        ..Session::default()
                    });
                }
                None => s.session = None,
                _ => {}
            }
        }
        for a in actions {
            execute(a, &shared, &mut engine);
        }
        ctx.request_repaint();

        match rx.recv_timeout(TICK) {
            Ok(Cmd::OptimizeNow) => {
                let cfg = lock(&shared).cfg.clone();
                let names = engine::closable_running(&procs::list(), &cfg);
                if names.is_empty() {
                    lock(&shared).event("nenhum app da lista está aberto agora");
                } else {
                    lock(&shared).event("otimização manual");
                    execute(
                        Action::Close {
                            names,
                            for_game: false,
                        },
                        &shared,
                        &mut engine,
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Painel,
    Apps,
    Jogos,
    Diagnostico,
    Registro,
}

enum Diag {
    Idle,
    Running,
    Done(Vec<windiag::Finding>),
    Failed(String),
}

struct Gui {
    shared: Arc<Mutex<Shared>>,
    tx: mpsc::Sender<Cmd>,
    tab: Tab,
    new_app: String,
    new_game: String,
    diag: Arc<Mutex<Diag>>,
    autostart: bool,
    dvr: Option<bool>,
    minimize_first_frame: bool,
    quitting: bool,
}

const GREEN: egui::Color32 = egui::Color32::from_rgb(60, 170, 90);
const AMBER: egui::Color32 = egui::Color32::from_rgb(220, 160, 40);
const RED: egui::Color32 = egui::Color32::from_rgb(200, 70, 70);

impl eframe::App for Gui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.minimize_first_frame {
            self.minimize_first_frame = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        let close_minimizes = lock(&self.shared).cfg.close_minimizes;
        if ctx.input(|i| i.viewport().close_requested()) && close_minimizes && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        ctx.request_repaint_after(Duration::from_secs(1));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("GamePrep");
                ui.label(egui::RichText::new("otimizador de Windows para jogos").weak());
            });
            ui.horizontal(|ui| {
                for (t, label) in [
                    (Tab::Painel, "Painel"),
                    (Tab::Apps, "Apps"),
                    (Tab::Jogos, "Jogos"),
                    (Tab::Diagnostico, "Diagnóstico"),
                    (Tab::Registro, "Registro"),
                ] {
                    if ui.selectable_label(self.tab == t, label).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.tab {
                    Tab::Painel => self.painel(ui, &ctx),
                    Tab::Apps => self.apps(ui),
                    Tab::Jogos => self.jogos(ui),
                    Tab::Diagnostico => self.diagnostico(ui),
                    Tab::Registro => self.registro(ui),
                });
        });
    }
}

impl Gui {
    fn painel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (session, open, busy, n_games) = {
            let s = lock(&self.shared);
            let open: Vec<AppRow> = s
                .rows
                .iter()
                .filter(|r| s.cfg.is_app(&r.key))
                .cloned()
                .collect();
            (s.session.clone(), open, s.busy, s.cfg.games.len())
        };

        ui.add_space(4.0);
        match &session {
            Some(sess) => {
                ui.label(
                    egui::RichText::new(format!("MODO JOGO ATIVO: {}", sess.game))
                        .strong()
                        .size(18.0)
                        .color(GREEN),
                );
                ui.label(format!(
                    "{} app(s) fechados nesta sessão, {} liberados",
                    sess.closed,
                    gb(sess.freed_mb)
                ));
            }
            None => {
                ui.label(
                    egui::RichText::new("Aguardando um jogo")
                        .strong()
                        .size(18.0),
                );
                let jogos = if n_games == 1 {
                    "1 jogo".to_string()
                } else {
                    format!("{n_games} jogos")
                };
                ui.label(format!(
                    "vigiando {jogos} (aba Jogos); o modo jogo liga sozinho quando um deles abrir"
                ));
            }
        }

        ui.add_space(8.0);
        let mb: f64 = open.iter().map(|r| r.mb).sum();
        ui.horizontal(|ui| {
            let label = if open.is_empty() {
                "nenhum app da lista aberto agora".to_string()
            } else {
                format!("{} app(s) da lista abertos agora ({})", open.len(), gb(mb))
            };
            ui.label(label);
            if ui
                .add_enabled(
                    !busy && !open.is_empty(),
                    egui::Button::new("Otimizar agora"),
                )
                .clicked()
            {
                let _ = self.tx.send(Cmd::OptimizeNow);
            }
            if busy {
                ui.spinner();
            }
        });
        if !open.is_empty() {
            ui.small(
                open.iter()
                    .map(|r| r.display.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Automático").strong());
        let mut s = lock(&self.shared);
        let mut changed = false;
        changed |= ui
            .checkbox(
                &mut s.cfg.auto_close,
                "Fechar os apps da lista quando um jogo abrir",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut s.cfg.relaunch_after,
                "Reabrir esses apps quando o jogo fechar",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut s.cfg.boost_priority,
                "Prioridade \"acima do normal\" para o jogo",
            )
            .on_hover_text(
                "Raramente ajuda e pode piorar áudio/voz; deixe desligado se não notar diferença",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut s.cfg.close_minimizes,
                "Botão X só minimiza (continua vigiando)",
            )
            .changed();
        if changed {
            s.save();
        }
        drop(s);
        if ui
            .checkbox(&mut self.autostart, "Iniciar com o Windows (minimizado)")
            .changed()
        {
            let ok = tweaks::set_autostart(self.autostart);
            self.autostart = tweaks::autostart_enabled();
            lock(&self.shared).event(if ok {
                format!(
                    "iniciar com o Windows: {}",
                    if self.autostart {
                        "ligado"
                    } else {
                        "desligado"
                    }
                )
            } else {
                "não consegui mudar a inicialização automática".to_string()
            });
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Windows").strong());
        ui.horizontal(|ui| match self.dvr {
            Some(true) => {
                ui.colored_label(
                    AMBER,
                    "Gravação em segundo plano do Xbox Game Bar: LIGADA (custa FPS)",
                );
                if ui.button("Desligar").clicked() {
                    let ok = tweaks::set_game_dvr(false);
                    self.dvr = tweaks::game_dvr_enabled();
                    lock(&self.shared).event(if ok {
                        "gravação do Game Bar desligada"
                    } else {
                        "não consegui desligar a gravação do Game Bar"
                    });
                }
            }
            Some(false) => {
                ui.colored_label(
                    GREEN,
                    "Gravação em segundo plano do Xbox Game Bar: desligada",
                );
                if ui.small_button("religar").clicked() {
                    tweaks::set_game_dvr(true);
                    self.dvr = tweaks::game_dvr_enabled();
                    lock(&self.shared).event("gravação do Game Bar religada");
                }
            }
            None => {
                ui.label("Gravação do Xbox Game Bar: padrão do Windows");
                if ui.button("Desligar").clicked() {
                    tweaks::set_game_dvr(false);
                    self.dvr = tweaks::game_dvr_enabled();
                    lock(&self.shared).event("gravação do Game Bar desligada");
                }
            }
        });

        ui.add_space(12.0);
        if ui
            .button("Sair do GamePrep")
            .on_hover_text("Para de vigiar os jogos")
            .clicked()
        {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn apps(&mut self, ui: &mut egui::Ui) {
        ui.label("Marque o que pode ser fechado quando um jogo abrir. Programas do Windows, antivírus, Steam, Riot/Vanguard e drivers nunca aparecem aqui.");
        ui.add_space(4.0);
        let mut s = lock(&self.shared);
        let rows = s.rows.clone();
        let mut changed = false;
        egui::Grid::new("apps")
            .striped(true)
            .num_columns(4)
            .show(ui, |ui| {
                ui.strong("fechar");
                ui.strong("app");
                ui.strong("processos");
                ui.strong("memória");
                ui.end_row();
                for r in &rows {
                    let mut on = s.cfg.is_app(&r.key);
                    if ui.checkbox(&mut on, "").changed() {
                        s.cfg.set_app(&r.key, on);
                        changed = true;
                    }
                    ui.label(&r.display);
                    ui.label(r.count.to_string());
                    ui.label(gb(r.mb));
                    ui.end_row();
                }
            });
        let closed: Vec<String> = s
            .cfg
            .apps
            .iter()
            .filter(|a| !rows.iter().any(|r| &r.key == *a))
            .cloned()
            .collect();
        if !closed.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Na lista, mas fechados agora").strong());
            for a in closed {
                ui.horizontal(|ui| {
                    ui.label(&a);
                    if ui.small_button("remover").clicked() {
                        s.cfg.set_app(&a, false);
                        changed = true;
                    }
                });
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Adicionar pelo nome do .exe:");
            ui.text_edit_singleline(&mut self.new_app);
            if ui.button("Adicionar").clicked() && !self.new_app.trim().is_empty() {
                let n = normalize(&self.new_app);
                if protect::is_protected(&n) {
                    s.event(format!("{n} é protegido e nunca é fechado"));
                } else {
                    s.cfg.set_app(&n, true);
                    changed = true;
                }
                self.new_app.clear();
            }
        });
        if changed {
            s.save();
        }
    }

    fn jogos(&mut self, ui: &mut egui::Ui) {
        ui.label("Quando qualquer um destes estiver aberto, o modo jogo liga.");
        ui.add_space(4.0);
        let mut s = lock(&self.shared);
        let mut changed = false;
        for g in s.cfg.games.clone() {
            ui.horizontal(|ui| {
                ui.label(&g);
                if ui.small_button("remover").clicked() {
                    s.cfg.remove_game(&g);
                    changed = true;
                }
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Adicionar jogo (.exe):");
            ui.text_edit_singleline(&mut self.new_game);
            if ui.button("Adicionar").clicked() && !self.new_game.trim().is_empty() {
                s.cfg.add_game(&self.new_game);
                self.new_game.clear();
                changed = true;
            }
        });
        ui.small("Dica: com o jogo aberto, veja o nome do processo no Gerenciador de Tarefas > Detalhes.");
        if changed {
            s.save();
        }
    }

    fn diagnostico(&mut self, ui: &mut egui::Ui) {
        ui.label("Verifica rede, driver, RAM, GPU, Game Bar, energia e inicialização. Só lê, não muda nada.");
        let running = matches!(
            *self.diag.lock().unwrap_or_else(|e| e.into_inner()),
            Diag::Running
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("Rodar diagnóstico"))
                .clicked()
            {
                *self.diag.lock().unwrap_or_else(|e| e.into_inner()) = Diag::Running;
                let diag = self.diag.clone();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let result = match windiag::collect(5) {
                        Ok(snap) => Diag::Done(windiag::findings(&snap, agentlog::today_days())),
                        Err(e) => Diag::Failed(e.to_string()),
                    };
                    *diag.lock().unwrap_or_else(|e| e.into_inner()) = result;
                    ctx.request_repaint();
                });
            }
            if running {
                ui.spinner();
                ui.label("coletando (~10 s)...");
            }
        });
        ui.add_space(6.0);
        match &*self.diag.lock().unwrap_or_else(|e| e.into_inner()) {
            Diag::Idle | Diag::Running => {}
            Diag::Failed(e) => {
                ui.colored_label(RED, format!("falhou: {e}"));
            }
            Diag::Done(findings) => {
                let mut area = "";
                for f in findings {
                    if f.area != area {
                        area = f.area;
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(area).strong());
                    }
                    let (tag, color) = match f.level {
                        windiag::Level::Ok => ("OK", GREEN),
                        windiag::Level::Info => ("INFO", egui::Color32::GRAY),
                        windiag::Level::Warn => ("AVISO", AMBER),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(color, egui::RichText::new(tag).monospace());
                        ui.label(&f.text);
                    });
                }
            }
        }
    }

    fn registro(&mut self, ui: &mut egui::Ui) {
        let s = lock(&self.shared);
        if s.events.is_empty() {
            ui.label("nada ainda");
        }
        for e in &s.events {
            ui.label(egui::RichText::new(e).monospace());
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--write-icon") {
        // Used by the MSI build script (Add/Remove Programs icon).
        let path = args
            .get(i + 1)
            .map(PathBuf::from)
            .unwrap_or_else(|| "gameprep.ico".into());
        std::fs::write(&path, icon::ico(&[16, 24, 32, 48, 64, 256])).expect("write icon");
        return Ok(());
    }
    if !tweaks::claim_single_instance() {
        return Ok(());
    }
    let start_minimized = args.iter().any(|a| a == "--minimized");
    endsession::spawn_listener();

    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let _ = agentlog::init_file(
            &PathBuf::from(dir).join(r"GamePrep\gameprep.log"),
            512 * 1024,
            2,
        );
    }
    let cfg_path = config::default_path();
    let cfg = config::load(&cfg_path);
    let shared = Arc::new(Mutex::new(Shared {
        cfg,
        cfg_path,
        rows: Vec::new(),
        session: None,
        events: VecDeque::new(),
        busy: false,
    }));
    lock(&shared).event(format!("GamePrep {} iniciado", env!("CARGO_PKG_VERSION")));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tweaks::APP_NAME)
            .with_inner_size([620.0, 640.0])
            .with_min_inner_size([480.0, 420.0])
            .with_icon(egui::IconData {
                rgba: icon::rgba(64),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        tweaks::APP_NAME,
        options,
        Box::new(move |cc| {
            let (tx, rx) = mpsc::channel();
            let engine_shared = shared.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || engine_loop(engine_shared, rx, ctx));
            Ok(Box::new(Gui {
                shared,
                tx,
                tab: Tab::Painel,
                new_app: String::new(),
                new_game: String::new(),
                diag: Arc::new(Mutex::new(Diag::Idle)),
                autostart: tweaks::autostart_enabled(),
                dvr: tweaks::game_dvr_enabled(),
                minimize_first_frame: start_minimized,
                quitting: false,
            }))
        }),
    )
}
