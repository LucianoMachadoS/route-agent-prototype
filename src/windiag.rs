//! `diagnose`: read-only health check of the Windows PC for gaming.
//!
//! Collection is one PowerShell run of `windiag.ps1` (embedded at build
//! time) that prints a single JSON object, plus a gateway ping done here
//! with `bench::ping` so the local network leg is measured with the same
//! parser everything else uses. The judgement - what's fine, what's
//! worth fixing - is `findings()`, a pure function over the parsed
//! `Snapshot`, so every rule is unit-tested against fixtures instead of
//! only being checked by eye on this one machine.
//!
//! It changes nothing and needs no admin. Things that need admin to even
//! read (CPU temperature on most boards) are reported as unavailable,
//! not guessed.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{AgentError, Result};

const SCRIPT: &str = include_str!("windiag.ps1");
const COLLECT_TIMEOUT: Duration = Duration::from_secs(60);

const PLAN_BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const PLAN_POWER_SAVER: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";
const MEDIUM_WIFI: i64 = 9;
const MEDIUM_ETHERNET: i64 = 14;

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Snapshot {
    pub os: Option<OsInfo>,
    pub memory: Option<MemoryInfo>,
    pub adapter: Option<AdapterInfo>,
    pub cpu: Option<CpuInfo>,
    #[serde(deserialize_with = "lenient_vec")]
    pub gpus: Vec<GpuInfo>,
    pub vram_used_mb: Option<f64>,
    #[serde(deserialize_with = "lenient_vec")]
    pub top_vram: Vec<ProcUsage>,
    #[serde(deserialize_with = "lenient_vec")]
    pub top_ram: Vec<ProcUsage>,
    #[serde(deserialize_with = "lenient_vec")]
    pub temps_c: Vec<f64>,
    pub game: Option<GameSettings>,
    #[serde(deserialize_with = "lenient_vec")]
    pub startup_enabled: Vec<String>,
    pub disk: Option<DiskInfo>,
    /// Filled in after collection, not by the script.
    pub gateway_ping: Option<GatewayPing>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OsInfo {
    pub caption: Option<String>,
    pub build: Option<String>,
    pub uptime_hours: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MemoryInfo {
    pub total_mb: Option<f64>,
    pub free_mb: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AdapterInfo {
    pub name: Option<String>,
    pub description: Option<String>,
    pub ndis_medium: Option<i64>,
    pub link_speed_mbps: Option<f64>,
    pub driver_version: Option<String>,
    pub driver_date: Option<String>,
    pub driver_provider: Option<String>,
    pub mtu: Option<i64>,
    pub rss_enabled: Option<bool>,
    pub wifi_signal_pct: Option<f64>,
    pub gateway: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CpuInfo {
    pub name: Option<String>,
    pub cores: Option<i64>,
    pub threads: Option<i64>,
    pub load_pct: Option<f64>,
    pub max_mhz: Option<f64>,
    pub current_mhz: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GpuInfo {
    pub name: Option<String>,
    pub driver_version: Option<String>,
    pub driver_date: Option<String>,
    pub vram_mb: Option<f64>,
    pub refresh_hz: Option<f64>,
    pub active: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProcUsage {
    pub name: String,
    pub count: Option<f64>,
    pub mb: Option<f64>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GameSettings {
    pub game_dvr: Option<i64>,
    pub app_capture: Option<i64>,
    pub auto_game_mode: Option<i64>,
    pub hags: Option<i64>,
    pub power_plan: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DiskInfo {
    pub drive: Option<String>,
    pub free_gb: Option<f64>,
    pub total_gb: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayPing {
    pub host: String,
    pub sent: u32,
    pub loss_pct: f64,
    pub median_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

/// Windows PowerShell 5.1's ConvertTo-Json turns a one-element array into
/// a bare object and an absent one into null; accept all three shapes.
fn lenient_vec<'de, D, T>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        Many(Vec<T>),
        One(T),
    }
    Ok(match Option::<OneOrMany<T>>::deserialize(d)? {
        None => Vec::new(),
        Some(OneOrMany::Many(v)) => v,
        Some(OneOrMany::One(x)) => vec![x],
    })
}

/// Runs the collector and (if `gateway_pings > 0`) pings the router.
pub fn collect(gateway_pings: u32) -> Result<Snapshot> {
    let mut cmd = crate::syspath::hidden_command(crate::syspath::windows_powershell());
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]);
    let output = crate::syspath::output_with_timeout(cmd, COLLECT_TIMEOUT).map_err(|source| {
        if source.kind() == std::io::ErrorKind::TimedOut {
            AgentError::Timeout {
                command: "powershell.exe (diagnose)".to_string(),
                secs: COLLECT_TIMEOUT.as_secs(),
            }
        } else {
            AgentError::Spawn {
                command: "powershell.exe (diagnose)".to_string(),
                source,
            }
        }
    })?;
    let stdout = crate::console::decode(&output.stdout);
    let mut snap = parse_snapshot(&stdout).map_err(|reason| AgentError::CommandFailed {
        command: "powershell.exe (diagnose)".to_string(),
        status: output.status.code().unwrap_or(-1),
        stderr: format!(
            "{reason}; stderr: {}",
            crate::console::decode(&output.stderr).trim()
        ),
    })?;

    if gateway_pings > 0 {
        if let Some(gw) = snap
            .adapter
            .as_ref()
            .and_then(|a| nonempty(&a.gateway))
            .map(str::to_string)
        {
            if let Ok(stats) = crate::bench::ping(&gw, gateway_pings) {
                snap.gateway_ping = Some(GatewayPing {
                    host: gw,
                    sent: stats.sent,
                    loss_pct: stats.loss_pct(),
                    median_ms: stats.median(),
                    jitter_ms: stats.jitter(),
                    max_ms: stats.percentile(100.0),
                });
            }
        }
    }
    Ok(snap)
}

/// Parses the collector's output, tolerating anything printed around the
/// JSON object (PowerShell can emit warnings/progress on stdout).
pub fn parse_snapshot(stdout: &str) -> std::result::Result<Snapshot, String> {
    let start = stdout.find('{').ok_or("no JSON object in output")?;
    let end = stdout.rfind('}').ok_or("unterminated JSON object")?;
    serde_json::from_str(&stdout[start..=end]).map_err(|e| format!("invalid JSON: {e}"))
}

pub fn to_json(snap: &Snapshot) -> String {
    serde_json::to_string_pretty(snap).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub area: &'static str,
    pub text: String,
}

const AREAS: [&str; 5] = ["Rede", "Sistema", "GPU", "Jogos", "Inicialização"];

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn age_years(date: Option<&str>, today_days: i64) -> Option<f64> {
    let days = crate::agentlog::parse_ymd(date?)?;
    Some((today_days - days) as f64 / 365.25)
}

fn gb(mb: f64) -> String {
    format!("{:.1} GB", mb / 1024.0)
}

/// The rules. Pure: same snapshot + same day in, same findings out.
pub fn findings(s: &Snapshot, today_days: i64) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut push = |level, area, text: String| out.push(Finding { level, area, text });

    // --- Rede ---------------------------------------------------------
    let wifi = s.adapter.as_ref().and_then(|a| a.ndis_medium) == Some(MEDIUM_WIFI);
    match &s.adapter {
        None => push(
            Level::Warn,
            "Rede",
            "não encontrei o adaptador com a rota padrão (sem internet?)".to_string(),
        ),
        Some(a) => {
            let desc = nonempty(&a.description).unwrap_or("adaptador desconhecido");
            let speed = a.link_speed_mbps.unwrap_or(0.0);
            match a.ndis_medium {
                Some(MEDIUM_WIFI) => {
                    push(
                        Level::Warn,
                        "Rede",
                        format!("Wi-Fi ({desc}): para jogar, um cabo Ethernet reduz jitter e perda de pacotes"),
                    );
                    if let Some(sig) = a.wifi_signal_pct {
                        if sig < 60.0 {
                            push(Level::Warn, "Rede", format!("sinal do Wi-Fi fraco: {sig:.0}%"));
                        } else {
                            push(Level::Info, "Rede", format!("sinal do Wi-Fi: {sig:.0}%"));
                        }
                    }
                }
                Some(MEDIUM_ETHERNET) if speed > 0.0 && speed < 100.0 => push(
                    Level::Warn,
                    "Rede",
                    format!(
                        "Ethernet ({desc}) negociou só {speed:.0} Mbps - cabo ou porta com defeito/mal encaixado (o normal é 1000)"
                    ),
                ),
                Some(MEDIUM_ETHERNET) if speed > 0.0 && speed < 1000.0 => push(
                    Level::Info,
                    "Rede",
                    format!("Ethernet ({desc}) a {speed:.0} Mbps (placas gigabit normalmente negociam 1000)"),
                ),
                Some(MEDIUM_ETHERNET) => push(
                    Level::Ok,
                    "Rede",
                    format!("Ethernet ({desc}) a {speed:.0} Mbps"),
                ),
                _ => push(Level::Info, "Rede", format!("adaptador: {desc}")),
            }

            // A Microsoft-provided driver on third-party hardware is the
            // generic inbox one - works, but the vendor's is usually newer
            // and tuned (interrupt moderation, offloads, power saving).
            let vendor = desc.split_whitespace().next().unwrap_or("");
            let generic = nonempty(&a.driver_provider) == Some("Microsoft")
                && !vendor.is_empty()
                && !["Microsoft", "Hyper-V", "WAN"].contains(&vendor);
            let date = nonempty(&a.driver_date);
            let years = age_years(date, today_days);
            let ver = nonempty(&a.driver_version).unwrap_or("?");
            let date_txt = date
                .map(|d| d.get(..10).unwrap_or(d))
                .unwrap_or("data desconhecida");
            if generic {
                push(
                    Level::Warn,
                    "Rede",
                    format!(
                        "driver de rede genérico da Microsoft (v{ver}, {date_txt}) - instale o driver oficial da {vendor} pelo site do fabricante"
                    ),
                );
            } else if years.is_some_and(|y| y > 3.0) {
                push(
                    Level::Info,
                    "Rede",
                    format!("driver de rede antigo (v{ver}, {date_txt}) - vale procurar uma versão nova"),
                );
            } else {
                push(
                    Level::Ok,
                    "Rede",
                    format!("driver de rede v{ver} ({date_txt})"),
                );
            }

            if let Some(mtu) = a.mtu.filter(|m| *m > 0 && *m != 1500) {
                push(
                    Level::Info,
                    "Rede",
                    format!("MTU {mtu} (o padrão Ethernet é 1500) - se não foi alterado de propósito, vale verificar"),
                );
            }
            if a.rss_enabled == Some(false) {
                push(
                    Level::Info,
                    "Rede",
                    "RSS desativado no adaptador (distribui o processamento de rede entre núcleos)"
                        .to_string(),
                );
            }
        }
    }
    if let Some(g) = &s.gateway_ping {
        let jitter_limit = if wifi { 5.0 } else { 2.0 };
        let med = g
            .median_ms
            .map(|m| format!("{m:.0} ms"))
            .unwrap_or("-".into());
        let jit = g.jitter_ms.unwrap_or(0.0);
        if g.loss_pct > 0.0 {
            push(
                Level::Warn,
                "Rede",
                format!(
                    "perda de {:.0}% até o roteador ({}) - o problema está na rede de casa (cabo, Wi-Fi ou roteador), não na internet",
                    g.loss_pct, g.host
                ),
            );
        } else if jit > jitter_limit {
            push(
                Level::Warn,
                "Rede",
                format!(
                    "jitter de {jit:.1} ms até o roteador ({}) - rede local instável",
                    g.host
                ),
            );
        } else {
            push(
                Level::Ok,
                "Rede",
                format!(
                    "roteador ({}): mediana {med}, jitter {jit:.1} ms, 0% perda",
                    g.host
                ),
            );
        }
    }

    // --- Sistema --------------------------------------------------------
    if let Some(c) = &s.cpu {
        let name = nonempty(&c.name).unwrap_or("CPU");
        match c.load_pct {
            Some(l) if l > 85.0 => push(
                Level::Warn,
                "Sistema",
                format!("{name}: {l:.0}% em uso agora"),
            ),
            Some(l) => push(
                Level::Ok,
                "Sistema",
                format!("{name}: {l:.0}% em uso agora"),
            ),
            None => push(Level::Info, "Sistema", name.to_string()),
        }
    }
    if let Some(m) = &s.memory {
        if let (Some(total), Some(free)) = (m.total_mb, m.free_mb) {
            let pct = if total > 0.0 {
                free / total * 100.0
            } else {
                0.0
            };
            let text = format!("RAM: {} livres de {}", gb(free), gb(total));
            if free < 2048.0 || pct < 15.0 {
                push(
                    Level::Warn,
                    "Sistema",
                    format!("{text} - feche programas antes de jogar"),
                );
            } else {
                push(Level::Ok, "Sistema", text);
            }
        }
    }
    const SYSTEM_PROCS: [&str; 6] = [
        "System",
        "svchost",
        "Memory Compression",
        "Idle",
        "Registry",
        "dwm",
    ];
    let heavy: Vec<String> = s
        .top_ram
        .iter()
        .filter(|p| !SYSTEM_PROCS.contains(&p.name.as_str()))
        .filter(|p| p.mb.unwrap_or(0.0) >= 300.0)
        .take(5)
        .map(|p| {
            let n = p.count.unwrap_or(1.0);
            let procs = if n > 1.0 {
                format!(" ({n:.0} proc.)")
            } else {
                String::new()
            };
            format!("{} {}{procs}", p.name, gb(p.mb.unwrap_or(0.0)))
        })
        .collect();
    if !heavy.is_empty() {
        push(
            Level::Info,
            "Sistema",
            format!("mais memória agora: {}", heavy.join(", ")),
        );
    }
    if s.temps_c.is_empty() {
        push(
            Level::Info,
            "Sistema",
            "temperatura indisponível (o Windows só expõe o sensor com admin e suporte da placa-mãe) - veja no software do fabricante (ex.: AMD Software)".to_string(),
        );
    } else {
        let max = s.temps_c.iter().cloned().fold(f64::MIN, f64::max);
        let level = if max > 85.0 { Level::Warn } else { Level::Ok };
        push(
            level,
            "Sistema",
            format!("temperatura (zona ACPI): {max:.0} C"),
        );
    }
    if let Some(h) = s.os.as_ref().and_then(|o| o.uptime_hours) {
        if h > 168.0 {
            push(
                Level::Info,
                "Sistema",
                format!("ligado há {:.0} dias sem reiniciar", h / 24.0),
            );
        }
    }
    if let Some(d) = &s.disk {
        if let (Some(free), Some(total)) = (d.free_gb, d.total_gb) {
            let drive = nonempty(&d.drive).unwrap_or("C:");
            let text = format!("disco {drive} {free:.0} GB livres de {total:.0} GB");
            if free < 20.0 || (total > 0.0 && free / total < 0.10) {
                push(Level::Warn, "Sistema", text);
            } else {
                push(Level::Ok, "Sistema", text);
            }
        }
    }

    // --- GPU ------------------------------------------------------------
    let active: Vec<&GpuInfo> = s
        .gpus
        .iter()
        .filter(|g| g.active == Some(true) || g.refresh_hz.unwrap_or(0.0) > 0.0)
        .collect();
    for g in &active {
        let name = nonempty(&g.name).unwrap_or("GPU");
        let vram = g
            .vram_mb
            .map(|v| format!(", {}", gb(v)))
            .unwrap_or_default();
        let hz = g
            .refresh_hz
            .filter(|h| *h > 0.0)
            .map(|h| format!(", monitor a {h:.0} Hz"))
            .unwrap_or_default();
        push(Level::Ok, "GPU", format!("{name}{vram}{hz}"));
        let date = nonempty(&g.driver_date);
        if age_years(date, today_days).is_some_and(|y| y > 1.0) {
            push(
                Level::Info,
                "GPU",
                format!(
                    "driver de vídeo de {} - atualize pelo site da fabricante",
                    date.unwrap_or("?")
                ),
            );
        }
    }
    let total_vram: f64 = active.iter().filter_map(|g| g.vram_mb).sum();
    if let Some(used) = s.vram_used_mb {
        if total_vram > 0.0 {
            let pct = used / total_vram * 100.0;
            let top: Vec<String> = s
                .top_vram
                .iter()
                .take(3)
                .map(|p| format!("{} {}", p.name, gb(p.mb.unwrap_or(0.0))))
                .collect();
            let text = format!(
                "VRAM: {} de {} em uso ({pct:.0}%){}",
                gb(used),
                gb(total_vram),
                if top.is_empty() {
                    String::new()
                } else {
                    format!(" - {}", top.join(", "))
                }
            );
            if pct > 90.0 {
                push(
                    Level::Warn,
                    "GPU",
                    format!(
                        "{text}; perto do limite causa travadas - baixe a qualidade das texturas"
                    ),
                );
            } else {
                push(Level::Ok, "GPU", text);
            }
        }
    }

    // --- Jogos ----------------------------------------------------------
    if let Some(g) = &s.game {
        if g.game_dvr == Some(1) || g.app_capture == Some(1) {
            push(
                Level::Warn,
                "Jogos",
                "gravação em segundo plano do Xbox Game Bar ligada - custa FPS (o GamePrep desliga em um clique)".to_string(),
            );
        } else {
            push(
                Level::Ok,
                "Jogos",
                "gravação em segundo plano do Game Bar desligada".to_string(),
            );
        }
        if g.auto_game_mode == Some(0) {
            push(
                Level::Warn,
                "Jogos",
                "Modo de Jogo do Windows desligado".to_string(),
            );
        }
        match g.power_plan.as_deref() {
            Some(PLAN_POWER_SAVER) => push(
                Level::Warn,
                "Jogos",
                "plano de energia Economia - limita o clock da CPU".to_string(),
            ),
            Some(PLAN_BALANCED) => push(
                Level::Info,
                "Jogos",
                "plano de energia Equilibrado - num desktop, Alto desempenho evita quedas de clock"
                    .to_string(),
            ),
            Some(_) => push(
                Level::Ok,
                "Jogos",
                "plano de energia de alto desempenho".to_string(),
            ),
            None => {}
        }
    }

    // --- Inicializacao --------------------------------------------------
    let n = s.startup_enabled.len();
    let list = s.startup_enabled.join(", ");
    if n > 8 {
        push(
            Level::Info,
            "Inicialização",
            format!("{n} apps iniciam com o Windows: {list} (desative os que você não usa em Gerenciador de Tarefas > Aplicativos de inicialização)"),
        );
    } else {
        push(
            Level::Ok,
            "Inicialização",
            format!("{n} apps iniciam com o Windows: {list}"),
        );
    }

    out
}

pub fn print_report(s: &Snapshot, findings: &[Finding]) {
    let os =
        s.os.as_ref()
            .and_then(|o| {
                nonempty(&o.caption)
                    .map(|c| format!("{c} (build {})", o.build.as_deref().unwrap_or("?")))
            })
            .unwrap_or_else(|| "Windows".to_string());
    println!("Diagnóstico do PC para jogos - {os}");
    for area in AREAS {
        let items: Vec<&Finding> = findings.iter().filter(|f| f.area == area).collect();
        if items.is_empty() {
            continue;
        }
        println!();
        println!("[{area}]");
        for f in items {
            let tag = match f.level {
                Level::Ok => "  OK   ",
                Level::Info => "  INFO ",
                Level::Warn => "  AVISO",
            };
            println!("{tag} {}", f.text);
        }
    }
    let warns = findings.iter().filter(|f| f.level == Level::Warn).count();
    let infos = findings.iter().filter(|f| f.level == Level::Info).count();
    println!();
    println!("Resumo: {warns} aviso(s), {infos} informação(ões). Nada foi alterado.");
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: i64 = 20_742; // 2026-10-16

    // Trimmed from a real run on the dev machine (Ryzen 5 3600 / RX 580 /
    // Realtek on the Microsoft inbox driver).
    const REAL: &str = r#"{"os":{"caption":"Microsoft Windows 11 Pro","build":"26200","uptime_hours":13.5},"memory":{"total_mb":16318,"free_mb":3706},"adapter":{"name":"Ethernet","description":"Realtek PCIe GbE Family Controller","ndis_medium":14,"link_speed_mbps":1000,"driver_version":"1.0.0.14","driver_date":"2017-08-10","driver_provider":"Microsoft","mtu":1500,"rss_enabled":null,"wifi_signal_pct":null,"gateway":"10.0.1.1"},"cpu":{"name":"AMD Ryzen 5 3600 6-Core Processor","cores":6,"threads":12,"load_pct":25,"max_mhz":3593,"current_mhz":3593},"gpus":[{"name":"Parsec Virtual Display Adapter","driver_version":"0.45.0.0","driver_date":"2024-01-24","vram_mb":null,"refresh_hz":0,"active":false},{"name":"AMD Radeon RX 580 2048SP","driver_version":"31.0.21925.1001","driver_date":"2026-05-19","vram_mb":8192,"refresh_hz":165,"active":true}],"vram_used_mb":3963,"top_vram":[{"name":"cs2","count":1,"mb":3003}],"top_ram":[{"name":"cs2","count":1,"mb":2146},{"name":"chrome","count":14,"mb":2131},{"name":"svchost","count":89,"mb":1472}],"temps_c":[],"game":{"game_dvr":0,"app_capture":0,"auto_game_mode":null,"hags":null,"power_plan":"60dcbe48-48ef-499f-a6ee-281cd6f006fa"},"startup_enabled":["AMDNoiseSuppression","Steam","RiotClient"],"disk":{"drive":"C:","free_gb":165.8,"total_gb":475.9}}"#;

    fn has(f: &[Finding], level: Level, needle: &str) -> bool {
        f.iter()
            .any(|x| x.level == level && x.text.contains(needle))
    }

    #[test]
    fn real_machine_flags_generic_realtek_driver_and_nothing_alarming_else() {
        let s = parse_snapshot(REAL).unwrap();
        let f = findings(&s, TODAY);
        assert!(has(&f, Level::Warn, "driver oficial da Realtek"));
        assert!(has(
            &f,
            Level::Ok,
            "Ethernet (Realtek PCIe GbE Family Controller) a 1000 Mbps"
        ));
        assert!(has(
            &f,
            Level::Ok,
            "AMD Radeon RX 580 2048SP, 8.0 GB, monitor a 165 Hz"
        ));
        assert!(has(&f, Level::Ok, "VRAM: 3.9 GB de 8.0 GB"));
        assert!(has(&f, Level::Ok, "Game Bar desligada"));
        assert!(has(&f, Level::Ok, "alto desempenho"));
        assert!(has(&f, Level::Info, "chrome 2.1 GB (14 proc.)"));
        // svchost is system noise, never listed as "heavy".
        assert!(!f.iter().any(|x| x.text.contains("svchost")));
        // The inactive Parsec virtual adapter isn't reported as a GPU.
        assert!(!f.iter().any(|x| x.text.contains("Parsec")));
        let warns = f.iter().filter(|x| x.level == Level::Warn).count();
        assert_eq!(warns, 1, "{f:#?}");
    }

    #[test]
    fn wifi_dvr_low_ram_balanced_plan_and_lossy_router_are_warned() {
        let mut s = parse_snapshot(
            r#"{"memory":{"total_mb":8192,"free_mb":900},
                "adapter":{"description":"Intel Wi-Fi 6 AX201","ndis_medium":9,"wifi_signal_pct":42,
                           "driver_provider":"Intel","driver_date":"2025-01-01","gateway":"192.168.0.1"},
                "game":{"game_dvr":1,"power_plan":"381b4222-f694-41f0-9685-ff5bb260df2e"}}"#,
        )
        .unwrap();
        s.gateway_ping = Some(GatewayPing {
            host: "192.168.0.1".into(),
            sent: 10,
            loss_pct: 10.0,
            median_ms: Some(3.0),
            jitter_ms: Some(1.0),
            max_ms: Some(9.0),
        });
        let f = findings(&s, TODAY);
        assert!(has(&f, Level::Warn, "cabo Ethernet"));
        assert!(has(&f, Level::Warn, "sinal do Wi-Fi fraco: 42%"));
        assert!(has(&f, Level::Warn, "RAM: 0.9 GB livres"));
        assert!(has(&f, Level::Warn, "Game Bar ligada"));
        assert!(has(&f, Level::Info, "Equilibrado"));
        assert!(has(&f, Level::Warn, "perda de 10% até o roteador"));
        // Vendor driver, recent: not flagged.
        assert!(has(&f, Level::Ok, "driver de rede"));
    }

    #[test]
    fn slow_ethernet_link_and_full_vram_are_warned() {
        let s = parse_snapshot(
            r#"{"adapter":{"description":"Realtek PCIe GbE","ndis_medium":14,"link_speed_mbps":10,
                           "driver_provider":"Realtek","driver_date":"2025-06-01"},
                "gpus":{"name":"GTX 1050","vram_mb":2048,"active":true},
                "vram_used_mb":1990}"#,
        )
        .unwrap();
        let f = findings(&s, TODAY);
        assert!(has(&f, Level::Warn, "negociou só 10 Mbps"));
        assert!(has(&f, Level::Warn, "baixe a qualidade das texturas"));
        // A single GPU serialized as a bare object still counts.
        assert!(has(&f, Level::Ok, "GTX 1050"));
    }

    #[test]
    fn empty_or_noisy_output_never_panics() {
        let s = parse_snapshot("WARNING: something\n{}\n").unwrap();
        let f = findings(&s, TODAY);
        assert!(has(&f, Level::Warn, "rota padrão"));
        assert!(parse_snapshot("no json here").is_err());
        let nulls = parse_snapshot(
            r#"{"gpus":null,"top_ram":null,"temps_c":null,"startup_enabled":"OnlyOne"}"#,
        )
        .unwrap();
        assert_eq!(nulls.startup_enabled, vec!["OnlyOne".to_string()]);
        assert!(nulls.gpus.is_empty());
    }

    #[test]
    fn embedded_script_follows_its_own_rules() {
        assert!(SCRIPT.is_ascii(), "windiag.ps1 must stay ASCII");
        assert!(
            !SCRIPT.contains('"'),
            "windiag.ps1 is passed via -Command: no double quotes"
        );
    }
}
