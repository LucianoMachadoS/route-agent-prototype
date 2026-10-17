//! Processes GamePrep never closes and never even offers to close, no
//! matter what the config says: Windows itself, security software, game
//! launchers/anti-cheat the game needs, GPU/audio drivers, terminals that
//! may be running the user's work, and GamePrep.

const EXACT: &[&str] = &[
    // Windows core / shell
    "system",
    "idle",
    "registry",
    "memory compression",
    "secure system",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "svchost.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "explorer.exe",
    "sihost.exe",
    "ctfmon.exe",
    "runtimebroker.exe",
    "searchhost.exe",
    "searchindexer.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
    "conhost.exe",
    "taskhostw.exe",
    "dllhost.exe",
    "audiodg.exe",
    "spoolsv.exe",
    "lockapp.exe",
    "applicationframehost.exe",
    "wmiprvse.exe",
    "smartscreen.exe",
    "securityhealthservice.exe",
    "securityhealthsystray.exe",
    "msmpeng.exe",
    "nissrv.exe",
    "mpdefendercoreservice.exe",
    "taskmgr.exe",
    "msedgewebview2.exe",
    "widgets.exe",
    "widgetservice.exe",
    "useroobebroker.exe",
    "systemsettings.exe",
    "dashost.exe",
    "wudfhost.exe",
    "crossdeviceservice.exe",
    "phoneexperiencehost.exe",
    // terminals / shells (could be running the user's work - or this session)
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "windowsterminal.exe",
    "openconsole.exe",
    "wsl.exe",
    "wslhost.exe",
    "claude.exe",
    // game launchers, anti-cheat, overlays the game needs
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "gameoverlayui.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "riotclientuxrender.exe",
    "riotclientcrashhandler.exe",
    "vgc.exe",
    "vgtray.exe",
    "epicgameslauncher.exe",
    "easyanticheat.exe",
    "easyanticheat_eos.exe",
    "beservice.exe",
    "battle.net.exe",
    "agent.exe",
    "eadesktop.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "faceit.exe",
    "faceitclient.exe",
    // GPU / audio drivers and their helpers
    "radeonsoftware.exe",
    "amdrsserv.exe",
    "amdrssrcext.exe",
    "atieclxx.exe",
    "atiesrxx.exe",
    "amdow.exe",
    "amdfendrsr.exe",
    "nvcontainer.exe",
    "nvdisplay.container.exe",
    "nvidia share.exe",
    "nvidia overlay.exe",
    "igfxem.exe",
    "rtkauduservice64.exe",
    "rtkaudioservice64.exe",
    // VPN / this project
    "wireguard.exe",
    "route-agent.exe",
    "route-agent-gui.exe",
    "gameprep.exe",
];

const PREFIXES: &[&str] = &[
    // Avast/AVG engines and UI (also self-protected; killing them fails anyway)
    "avast",
    "asw",
    "avg",
    "afwserv",
    "bgsvc",
    "wsc_proxy",
];

/// `name` is a normalized (lowercase, `.exe`) process name, or a bare
/// pseudo-process name like "system".
pub fn is_protected(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    EXACT.contains(&n.as_str()) || PREFIXES.iter().any(|p| n.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_windows_av_launchers_and_self() {
        for n in [
            "explorer.exe",
            "svchost.exe",
            "AvastUI.exe",
            "aswEngSrv.exe",
            "steam.exe",
            "vgc.exe",
            "RadeonSoftware.exe",
            "gameprep.exe",
            "claude.exe",
            "powershell.exe",
            "Memory Compression",
        ] {
            assert!(is_protected(n), "{n} must be protected");
        }
    }

    #[test]
    fn ordinary_apps_are_closable() {
        for n in [
            "discord.exe",
            "spotify.exe",
            "opera.exe",
            "chrome.exe",
            "lm studio.exe",
        ] {
            assert!(!is_protected(n), "{n} should be closable");
        }
    }

    #[test]
    fn list_is_normalized() {
        for n in EXACT {
            assert_eq!(*n, n.to_lowercase(), "keep EXACT lowercase");
        }
    }
}
