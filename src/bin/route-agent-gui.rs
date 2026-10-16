//! `route-agent-gui`: a small native window over the same pieces the CLI
//! uses. Read-only state (tunnel service, reachability, games.toml, the
//! watch-games log) is gathered in-process - none of it needs elevation.
//! Anything that *does* need Administrator (connect/disconnect/watch) is
//! done by `schtasks /run`/`/end` on the Scheduled Tasks registered once
//! from an elevated session (see scripts/scheduled-tasks/README.md), so
//! this window never runs elevated itself and never asks for UAC.
//!
//! Security boundary, same as the tasks themselves: the task names below
//! are fixed constants. This binary never builds a task name or command
//! line from user input or from a file.

// No console window in release builds; keep it in debug for eprintln!.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use clap::Parser;
use eframe::egui;
use route_agent::bench::PingProbe;
use route_agent::games::{self, GameProfile};
use route_agent::traits::{Measurement, Probe};
use route_agent::{console, syspath, wireguard};

const TASK_CONNECT: &str = "RouteAgent-Connect-SaoPaulo";
const TASK_DISCONNECT: &str = "RouteAgent-Disconnect-SaoPaulo";
const TASK_WATCH: &str = "RouteAgent-WatchGame-CS2";

const REFRESH_EVERY: Duration = Duration::from_secs(5);
const LOG_TAIL_BYTES: u64 = 8 * 1024;

#[derive(Parser, Debug)]
#[command(name = "route-agent-gui", about = "Janela de status do route-agent")]
struct Args {
    /// games.toml to list. Default: %ProgramData%\RouteAgent\games.toml
    /// (the installed copy), then ./games.toml, then next to the exe and
    /// up to three parent folders (so target\release\ finds the repo's).
    #[arg(long)]
    games: Option<PathBuf>,

    /// Address inside the tunnel to ping for the reachability check.
    #[arg(long, default_value = "10.66.0.1")]
    tunnel_host: String,

    /// Tunnel (service) name, used if games.toml can't be read.
    #[arg(long, default_value = "saopaulo-gcp")]
    tunnel_name: String,
}

/// Everything one refresh round produces. Errors are kept as display
/// strings - the window shows them, it doesn't act on them.
#[derive(Default)]
struct Snapshot {
    tunnel_name: String,
    service: Option<Result<wireguard::ServiceState, String>>,
    reachability: Option<Result<Measurement, String>>,
    tasks: Vec<(&'static str, String)>,
    games: Option<Result<Vec<GameProfile>, String>>,
    games_path: Option<PathBuf>,
    log_tail: String,
}

enum Msg {
    Snapshot(Snapshot),
    ActionDone(String),
}

struct GuiApp {
    args: Args,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    snapshot: Snapshot,
    refreshing: bool,
    last_refresh: Option<Instant>,
    last_action: String,
}

impl GuiApp {
    fn new(args: Args) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            args,
            tx,
            rx,
            snapshot: Snapshot::default(),
            refreshing: false,
            last_refresh: None,
            last_action: String::new(),
        }
    }

    fn start_refresh(&mut self, ctx: &egui::Context) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.last_refresh = Some(Instant::now());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let games_arg = self.args.games.clone();
        let tunnel_host = self.args.tunnel_host.clone();
        let fallback_name = self.args.tunnel_name.clone();
        std::thread::spawn(move || {
            let snap = gather(games_arg.as_deref(), &tunnel_host, &fallback_name);
            let _ = tx.send(Msg::Snapshot(snap));
            ctx.request_repaint();
        });
    }

    fn run_task(&mut self, ctx: &egui::Context, verb: &'static str, task: &'static str) {
        self.last_action = format!("{task}: enviando...");
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match schtasks(&[verb, "/tn", task]) {
                Ok(_) => format!("{task}: ok ({verb})"),
                Err(e) => format!("{task}: falhou - {e}"),
            };
            let _ = tx.send(Msg::ActionDone(msg));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Snapshot(s) => {
                    self.snapshot = s;
                    self.refreshing = false;
                }
                Msg::ActionDone(text) => {
                    self.last_action = text;
                    // Show the effect of the action without waiting a full cycle.
                    self.last_refresh = None;
                }
            }
        }

        let due = self
            .last_refresh
            .map_or(true, |t| t.elapsed() >= REFRESH_EVERY);
        if due {
            self.start_refresh(ctx);
        }
        // egui only repaints on input by default; keep the timer ticking.
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("route-agent");
            ui.add_space(4.0);
            self.tunnel_section(ui, ctx);
            ui.separator();
            self.games_section(ui, ctx);
            ui.separator();
            self.log_section(ui);
        });
    }
}

impl GuiApp {
    fn tunnel_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let snap = &self.snapshot;
        ui.label(egui::RichText::new("Túnel").strong().size(16.0));

        let name = if snap.tunnel_name.is_empty() {
            self.args.tunnel_name.as_str()
        } else {
            snap.tunnel_name.as_str()
        };
        let (text, color) = match &snap.service {
            None => ("verificando...".to_string(), egui::Color32::GRAY),
            Some(Ok(wireguard::ServiceState::Running)) => (
                "conectado".to_string(),
                egui::Color32::from_rgb(60, 170, 90),
            ),
            Some(Ok(state)) => (state.to_string(), egui::Color32::from_rgb(220, 160, 40)),
            // sc.exe reports nothing parseable once `disconnect` has
            // uninstalled the service - that's the normal disconnected state.
            Some(Err(_)) => (
                "desconectado".to_string(),
                egui::Color32::from_rgb(200, 70, 70),
            ),
        };
        ui.horizontal(|ui| {
            ui.label(format!("{name}:"));
            ui.colored_label(color, text);
        });

        match &snap.reachability {
            None => {}
            Some(Ok(m)) => {
                let rtt = m
                    .rtt_p95_ms
                    .map_or("-".to_string(), |v| format!("{v:.0} ms"));
                let jitter = m
                    .jitter_ms
                    .map_or("-".to_string(), |v| format!("{v:.1} ms"));
                ui.label(format!(
                    "{} - RTT p95 {rtt}, jitter {jitter}, perda {:.0}%",
                    self.args.tunnel_host, m.loss_pct
                ));
            }
            Some(Err(e)) => {
                ui.label(format!("{}: {e}", self.args.tunnel_host));
            }
        }

        ui.horizontal(|ui| {
            if ui.button("Conectar").clicked() {
                self.run_task(ctx, "/run", TASK_CONNECT);
            }
            if ui.button("Desconectar").clicked() {
                self.run_task(ctx, "/run", TASK_DISCONNECT);
            }
            if ui
                .add_enabled(!self.refreshing, egui::Button::new("Atualizar"))
                .clicked()
            {
                self.start_refresh(ctx);
            }
            if self.refreshing {
                ui.spinner();
            }
        });
        if !self.last_action.is_empty() {
            ui.small(&self.last_action);
        }
    }

    fn games_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let snap = &self.snapshot;
        ui.label(egui::RichText::new("Jogos").strong().size(16.0));

        match &snap.games {
            None => {
                ui.label("carregando...");
            }
            Some(Ok(list)) => {
                if let Some(p) = &snap.games_path {
                    ui.small(p.display().to_string());
                }
                egui::Grid::new("games").striped(true).show(ui, |ui| {
                    ui.strong("nome");
                    ui.strong("processo");
                    ui.strong("túnel");
                    ui.end_row();
                    for g in list {
                        ui.label(&g.name);
                        ui.label(&g.process);
                        ui.label(&g.tunnel_name);
                        ui.end_row();
                    }
                });
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::from_rgb(200, 70, 70), e);
            }
        }

        ui.add_space(4.0);
        for (task, status) in &snap.tasks {
            // Status is schtasks' own (localized) text, shown as-is -
            // parsing it would repeat the sc.exe "ESTADO" locale bug.
            ui.small(format!("{task}: {status}"));
        }
        ui.horizontal(|ui| {
            if ui.button("Iniciar detecção").clicked() {
                self.run_task(ctx, "/run", TASK_WATCH);
            }
            if ui.button("Parar detecção").clicked() {
                self.run_task(ctx, "/end", TASK_WATCH);
            }
        });
    }

    fn log_section(&self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Log (watch-game.log)")
                .strong()
                .size(16.0),
        );
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&self.snapshot.log_tail).monospace())
                        .wrap(),
                );
            });
    }
}

/// One refresh round. Runs on a worker thread: `ping.exe` alone takes a
/// couple of seconds, which would freeze the window if done in `update()`.
fn gather(games_arg: Option<&Path>, tunnel_host: &str, fallback_name: &str) -> Snapshot {
    let games_path = games_arg.map(Path::to_path_buf).or_else(find_games_toml);
    let games = games_path
        .as_deref()
        .map(|p| games::load(p).map(|f| f.games).map_err(|e| e.to_string()));

    let tunnel_name = games
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .and_then(|list| list.first())
        .map_or_else(|| fallback_name.to_string(), |g| g.tunnel_name.clone());

    let service = wireguard::status(&tunnel_name).map_err(|e| e.to_string());
    let reachability = match service {
        Ok(wireguard::ServiceState::Running) => {
            Some(PingProbe.measure(tunnel_host, 2).map_err(|e| e.to_string()))
        }
        _ => None,
    };

    let tasks = [TASK_CONNECT, TASK_DISCONNECT, TASK_WATCH]
        .into_iter()
        .map(|t| (t, task_status(t)))
        .collect();

    Snapshot {
        tunnel_name,
        service: Some(service),
        reachability,
        tasks,
        games: Some(games.unwrap_or_else(|| Err("games.toml não encontrado".to_string()))),
        games_path,
        log_tail: read_log_tail(),
    }
}

fn find_games_toml() -> Option<PathBuf> {
    // The installed copy (scripts/install.ps1) comes first: it's the one
    // the elevated watch-games task actually reads.
    let mut candidates: Vec<PathBuf> = std::env::var_os("ProgramData")
        .map(|d| PathBuf::from(d).join(r"RouteAgent\games.toml"))
        .into_iter()
        .collect();
    candidates.push(PathBuf::from("games.toml"));
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(
            exe.ancestors()
                .skip(1)
                .take(4)
                .map(|d| d.join("games.toml")),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// `schtasks /query /fo csv /nh` prints `"<name>","<next run>","<status>"`.
/// Columns are positional, so this doesn't depend on the header language;
/// the status value itself is localized ("Pronto", "Em execução", ...)
/// and returned as-is for display.
fn task_status(task: &str) -> String {
    match schtasks(&["/query", "/tn", task, "/fo", "csv", "/nh"]) {
        Ok(out) => out
            .lines()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| l.rsplit("\",\"").next())
            .map(|s| s.trim().trim_matches('"').to_string())
            .unwrap_or_else(|| "?".to_string()),
        Err(_) => "não registrada".to_string(),
    }
}

fn schtasks(args: &[&str]) -> Result<String, String> {
    let output = syspath::hidden_command(syspath::system32("schtasks.exe"))
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = console::decode(&output.stdout);
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = console::decode(&output.stderr);
        Err(format!("{} {}", stderr.trim(), stdout.trim())
            .trim()
            .to_string())
    }
}

fn read_log_tail() -> String {
    let Some(dir) = std::env::var_os("LOCALAPPDATA") else {
        return "(LOCALAPPDATA não definido)".to_string();
    };
    let path = PathBuf::from(dir).join(r"RouteAgent\watch-game.log");
    let Ok(mut file) = std::fs::File::open(&path) else {
        return format!("(sem log ainda em {})", path.display());
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(LOG_TAIL_BYTES);
    let mut buf = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut buf).is_err() {
        return format!("(não foi possível ler {})", path.display());
    }
    let text = console::decode(&buf);
    // Drop the first, probably cut-in-half line when we started mid-file.
    if start > 0 {
        text.split_once('\n')
            .map_or(text.clone(), |(_, rest)| rest.to_string())
    } else {
        text
    }
}

fn main() -> eframe::Result<()> {
    let args = Args::parse();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("route-agent")
            .with_inner_size([560.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "route-agent",
        options,
        Box::new(|_cc| Ok(Box::new(GuiApp::new(args)))),
    )
}
