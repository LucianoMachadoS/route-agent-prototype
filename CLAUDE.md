# CLAUDE.md — route-agent

Context for working on this project in future sessions. Don't duplicate
`README.md` (setup/usage) or `docs/SECURITY.md` (threat model) here -
this is for things that bit us and aren't obvious from reading the code.

## Current state (check this first in a new session)

`brasil-instance` (the GCP relay, `southamerica-east1-a`) was stopped at
the end of the previous session to avoid paying for idle time - status
`TERMINATED`. The Windows tunnel was disconnected to match (no point
leaving it "connected" to a dead endpoint). Last stopped 2026-10-16.
`gcloud` isn't on PATH in a fresh session - it's at
`%LOCALAPPDATA%\Google\google-cloud-sdk\bin\gcloud.cmd`. Before resuming:

1. `gcloud compute instances start brasil-instance --zone southamerica-east1-a`
2. **Check the external IP didn't change** - it's ephemeral, not
   reserved, so a stop/start cycle can reassign it:
   `gcloud compute instances describe brasil-instance --zone southamerica-east1-a --format="value(networkInterfaces[0].accessConfigs[0].natIP)"`.
   If it's still `34.151.232.106`, nothing else to do (it *did* change on
   the 2026-10-16 restart, from `34.95.142.255` - updated in both
   `write-saopaulo-conf.ps1` and `saopaulo-gcp.conf` via one elevated
   `-EncodedCommand` string replace). If it changed,
   `write-saopaulo-conf.ps1` (behind the `RouteAgent-Connect-SaoPaulo`
   Scheduled Task - see below) has that IP hardcoded in its `Endpoint =`
   line and needs updating before reconnecting.
3. Then `schtasks /run /tn "RouteAgent-Connect-SaoPaulo"` (no elevation
   needed - see the Scheduled Task section below) brings the tunnel back
   up with the known-good `AllowedIPs` (legacy CS2 ranges +
   `155.133.0.0/16` SDR).

## Measured result: the relay makes CS2 *worse* from this location (2026-10-16)

One ~10 min CS2 match each way, pinging Valve SDR `gru#1`
(`155.133.227.35`) once per second during the match:

| | direct | via relay |
|---|---|---|
| min / p50 / p95 | 10 / 11 / 13 ms | 15 / 17 / 20 ms |
| jitter / loss | 1.0 ms / 0% | 1.2 ms / 0% |

The **minimum** rose by 5 ms too, so it's path length, not congestion:
the ISP already hands off to Valve at IX.br São Paulo (`187.16.218.161`
in the traceroute, 9 hops, 11 ms) and the relay just adds a detour.
Don't spend more effort tuning the CS2 path through this relay; it can
only win if the ISP route degrades (peak hours) or for far-away servers.
`cs2.exe`'s SDR sockets are unconnected, so `Get-NetUDPEndpoint` never
shows the 155.133.x peer - measure a known PoP instead of waiting for
detection. Valorant was removed from `games.toml` (the relay has no Riot
NAT, so a detected Riot IP would have black-holed Valorant).

## Real cause of tasks flipping to Disabled: Avast Cleanup "Sleep Mode"

Not the antivirus engine - Avast One's Cleanup module (`AvastCleanupSvc`
/ `TuneupSvc.exe`) catalogued all three RouteAgent tasks *and* the
`WireGuardManager` and `WireGuardTunnel$saopaulo-gcp` services in
`C:\ProgramData\Avast Software\Cleanup\TUProgMan.10.tudb` (readable only
with `FileShare.ReadWrite`; it's locked) and kept disabling the tasks
within ~10 min of re-enabling. Fixed by the user marking those five
items "never sleep" in Avast; tasks stayed enabled 20+ min afterwards.
No Application event log entry is written when it does this.

## Installed layout (since 2026-10-16) - the tasks no longer run the repo build

The Scheduled Tasks run `C:\Program Files\RouteAgent\route-agent.exe`
and read `C:\ProgramData\RouteAgent\games.toml`, both admin-only, put
there by `scripts\install.ps1` (see docs/SECURITY.md for why: the old
layout was a local privilege-escalation path). Consequences:

- **`cargo build --release` alone changes nothing the tasks run.**
  Re-run `install.ps1` elevated afterwards (from a non-elevated session:
  `Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','"<repo>\scripts\install.ps1"' -Wait`
  - the user clicks one UAC prompt; output goes to
  `%LOCALAPPDATA%\RouteAgent\install.log`, since the elevated window closes).
- Same for edits to the repo's `games.toml` - the installed copy wins.
- Anything under `C:\ProgramData\RouteAgent\` now needs elevation to
  edit (including the `.bat` files - the "edit the .bat without
  re-registering" trick below still works, just from an elevated session).
- `install.ps1` must stay ASCII-only, and can't use `$PSScriptRoot` in a
  `param()` default - Windows PowerShell 5.1 leaves it empty there under
  `-File` (hit live: the first elevated run died before even starting
  its transcript).
- Avast exception, if wanted, belongs on the *installed* exe path only -
  never on the repo folder (that's the user-writable copy).

## Real bug: `detect-remote` crashed on every round during an actual match

`gamedetect::detect_remote_ips` used `Select-Object -ExpandProperty
RemoteAddress`, which *throws* the moment any object in the pipeline
lacks that property - and a UDP socket that never called `connect()`
(extremely common; UDP is connectionless, and plenty of game traffic
goes out via `sendto()` on an unconnected socket) has no `RemoteAddress`
at all. Confirmed live: 100% reproducible, every single round of
`watch-games` while CS2 was genuinely running in a real match (never
once succeeded at detecting anything - the whole point of the feature
never got validated end-to-end on the first real attempt). Fixed by
switching to plain `$_.RemoteAddress` property access, which evaluates
to `$null` instead of throwing when the property's absent - `$null`
lines are already skipped by the Rust-side `.parse::<IpAddr>()` filter.
**Lesson: this class of PowerShell object-shape assumption needs a live
test against a real app with real traffic, not just a synthetic/offline
one** - `cargo test` only covers `is_plausible_remote_peer` (pure), not
the actual `Get-NetUDPEndpoint` pipeline shape, and testing against
`steam.exe`'s idle UDP endpoints earlier in the same session didn't
trigger this (they happened to all have resolvable addresses or none at
all, apparently) - only an app in an active match reliably has the mix
of connected/unconnected sockets that exposes it.

## A Scheduled Task's `.bat` content can be edited without re-registering

Once a task is created with `/rl HIGHEST` (see below), its *action* is
just "run this file path" - the `.bat` file's own content isn't part of
what required elevation to set, so it can be edited freely afterward
without touching the task again. Used this live: `connect-saopaulo.bat`
was rewritten to rewrite the `.conf`'s `AllowedIPs` (via a small
`write-saopaulo-conf.ps1`) *before* calling `connect`, making the task
idempotent/self-correcting on every future run instead of connecting
with whatever the file happened to contain; separately,
`watch-cs2.bat` (behind the already-registered
`RouteAgent-WatchGame-CS2` task) was repointed from the single-game
`watch-game` to the multi-game `watch-games --games games.toml` without
creating a new task at all - the task's name is now a bit stale
(it covers Valorant too), but renaming it would need one more elevated
`schtasks /create`, so it was left as-is; only the file it runs changed.

## Running elevated commands without a UAC prompt every time

`connect`/`disconnect`/`add-game-route`/`watch-game` need Administrator.
Being an admin account doesn't elevate every process by default (UAC's
split-token model) - confirmed repeatedly this session via
`([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)`,
which was `False` in multiple "but I'm an admin!" sessions until a
terminal was *explicitly* opened with "Run as Administrator". A real
pitfall discovered live: `cmd.exe` vs `powershell.exe` makes zero
difference here - both inherit the same (non-elevated) process token, so
switching shells never fixes an "Acesso negado" from a missing
elevation. Only a genuinely elevated session does.

**Fix for repeated UAC friction**: a Scheduled Task created once with
`/rl HIGHEST` (needs an elevated session to *create*) can be `schtasks
/run`/`/end` from a completely unprivileged session afterward, with no
further prompts - the Task Scheduler service (already SYSTEM) handles
elevation internally. Documented properly, with the security reasoning
(fixed action string per task, never a generic "run whatever's in this
file" task - that would be a real privilege-escalation path) in
`scripts/scheduled-tasks/README.md`. Verified live: created
`RouteAgent-WatchGame-CS2` from an elevated window once, then
`schtasks /run`/`/query`/`/end` all worked from this (non-elevated)
session with zero prompts.

Also worth remembering: after a successful `connect`, the config file's
ACL gets hardened to SYSTEM+Administrators only
(`wireguard::restrict_config_permissions`) - so editing `AllowedIPs` by
hand *also* needs the same elevated session, not just `connect`/
`disconnect` themselves. `disconnect` does **not** undo that ACL change.

## ufw's `before.rules` has two "COMMIT" lines - don't sed-match on that alone

Inserting a new NAT rule via `sed -i '/^COMMIT$/i <rule>' before.rules`
corrupted the file: `before.rules` has a `*nat` table AND a `*filter`
table, each ending in its own "COMMIT" line, and the pattern matches
*both*, inserting a `-A POSTROUTING ... -j MASQUERADE` line into the
filter table too - where `POSTROUTING` isn't a valid chain, breaking
`ufw enable` with `iptables-restore: ... No chain/target/match by that
name`. SSH stayed up throughout (ufw's own chains kept running even
while the custom nat block failed to reload), so this wasn't a lockout,
just a mess to clean up (delete the one bad line by exact line number,
found with `grep -n`, not another global pattern match). If you add more
NAT rules later, insert by exact line number or rebuild the whole `*nat`
block, never a bare `/COMMIT/` pattern. Also: toggling `ufw disable` /
`ufw --force enable` repeatedly accumulates duplicate entries in
`iptables -t nat -L POSTROUTING` (harmless but messy) - flush with
`iptables -t nat -F POSTROUTING` before reapplying from `before.rules` if
you want a clean counter/rule list.

## CS2 uses Valve's SDR (Steam Datagram Relay), not direct server IPs

Learned from a real match's console log: CS2 no longer connects straight
to a game server. It connects to a nearby Valve-operated relay PoP
(`gru#1` = São Paulo, in this case `155.133.227.35:27015`,
`155.133.0.0/16` more broadly - confirmed against a second, unrelated
community IP list that independently used a `155.133.x.x` address for a
*different* US city, so this range is Valve's own relay network, not
specific to São Paulo) and Valve routes internally from there. The
**older, pre-SDR matchmaking IP ranges** documented in community GitHub
repos (`205.185.192.0/19`, `209.197.29.0/24`, `209.197.25.0/24`) are a
different, mostly-legacy address space - a NAT rule scoped only to those
had zero effect on a real SDR-era match, since the traffic never went
there. Don't assume a found-online IP range is what the game actually
uses now; verify with `detect-remote`/a live match's own diagnostics
first. Valorant's actual server range is still unknown - Riot doesn't
publish one and no community list for it was found reliable enough to
use.

## Avast's Ransomware Shield blocks writes inside this Desktop folder

`bench`/`monitor` failed with `error: Acesso negado. (os error 5)` right
after printing a correct comparison table - the ping/scoring worked, only
the CSV write failed. Not a code bug: this project lives under
`...\Desktop\route-agent-prototype\`, which **Avast's Ransomware Shield**
protects by default, and it blocks a freshly-compiled, unsigned
`route-agent.exe` from writing there even though the NTFS ACL (checked
with `icacls`) grants full control. Confirmed by writing the exact same
binary + CSV path to `%TEMP%` instead, which worked immediately - same
code, same user, different folder. `PowerShell`'s own `Add-Content` to
the *same* blocked file also succeeded, because PowerShell is an
allow-listed trusted app and our own exe isn't.

**Fix**: either add an exclusion for `route-agent.exe` (or the whole
project folder) in Avast's Ransomware Shield settings, or pass
`--csv <path outside Desktop/Documents/Pictures>` (e.g.
`%LOCALAPPDATA%\RouteAgent\runs.csv`). Didn't change the code's
*default* `docs/runs.csv`/`docs/decisions.csv` paths over this - it's an
environment-specific AV behavior (Windows' own Controlled Folder Access
does the same thing to protected folders on other machines), not
something to design around by default, and `docs/runs.csv` already has
real history from before this was discovered.

## Rust toolchain on this machine (Windows 11, pt-BR)

- Rust was **not** installed as of 2026-10-15. Installed via the official
  `rustup-init.exe` from `static.rust-lang.org` (not winget, per the
  user's preference).
- **The GNU toolchain (`stable-x86_64-pc-windows-gnu`) does not work out
  of the box**: compiling fails with `error calling dlltool 'dlltool.exe':
  program not found`. Fixing this needs a separate MinGW-w64 binutils
  install, which is about as much work as the alternative below - don't
  go down this path again expecting it to be quick.
- **The MSVC toolchain works, but needs Visual C++ Build Tools** (the
  linker, `link.exe`, isn't part of a bare Windows install). Installed via
  `vs_buildtools.exe` from `aka.ms/vs/17/release/vs_buildtools.exe`
  (direct from Microsoft, not winget), with:
  `--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended`
  Takes several minutes; ran it with `run_in_background: true`.
- After both installs: `rustup default stable-x86_64-pc-windows-msvc`,
  then `cargo build` works. `cargo`/`rustc` live in
  `%USERPROFILE%\.cargo\bin`, which isn't on PATH in a *new* PowerShell
  session until you open a fresh shell (or re-add it manually with
  `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"`).
- `clap`'s `env` feature (used for `WIREGUARD_EXE`) must be enabled
  explicitly: `features = ["derive", "env"]`. It's easy to add the `env`
  attribute to a clap arg and forget the crate feature - compiles fine
  without it being wired up correctly? No - it fails to build without the
  feature. Check Cargo.toml first if `env =` attributes stop working.

## Real bug found and fixed: `ping.exe`'s trailing stats line

`parse_ping_output` in `src/bench.rs` used to count the trailing
`Minimum = Xms, Maximum = Yms, Average = Zms` summary line as an extra
reply (observed live as "9/8 received/sent" during a smoke test). Fixed
by requiring `bytes=` on the line before attempting to extract a `...ms`
value - only real per-probe reply lines contain that. There's a
regression test for this (`ignores_the_trailing_statistics_line`) - if
you touch the parser, make sure that test still exists and still runs
against a *realistic* multi-line `ping.exe` transcript, not just a single
reply line, or this regresses silently again.

## Security hardening already applied (see docs/SECURITY.md for the full list)

Don't undo these while refactoring:

- `ping.exe`, `sc.exe`, `icacls.exe`, `powershell.exe` are invoked by
  fully-qualified `%SystemRoot%\System32\...` path (`src/syspath.rs`),
  never bare name. A bare name is searched-for in the current working
  directory before `System32` on Windows (CWE-427) - a real local
  priv-esc path for `connect`/`disconnect`, which run elevated.
- `icacls` ACL grants use the **well-known SIDs** `*S-1-5-18` (SYSTEM) and
  `*S-1-5-32-544` (Administrators), not the group names "SYSTEM"/
  "Administrators" - those names are localized (e.g. "Administradores"
  on pt-BR Windows, which is what this user runs) and would silently fail
  or target the wrong principal on a non-English install.
- `WIREGUARD_EXE`/`--wireguard` overrides get a loud stderr warning if
  they don't resolve to a known Program Files install path, since that
  binary then runs elevated.

## Original README recovery incident (context, not a recurring risk)

Early in this project's history the assistant overwrote `README.md`
without reading it first, with no git/backup to recover it. The user
later supplied the original project zip; it was extracted to `antigo/`
(gitignored, safe to delete - everything useful from it was already
merged into `README.md`: the original `connect`/`disconnect` naming and
the `--wireguard`/`WIREGUARD_EXE` design, which the CLI was renamed to
match). Lesson that generalizes: always `Read` a file before overwriting
it with `Write`, even when asked to work fast.

## Build/test loop

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"   # if cargo isn't found
cargo fmt
cargo test            # 80 tests as of 2026-10-16 (72 lib + 6 + 2 integration); a few Windows-only ones spawn cmd/ping/kernel32 locally, none touch the network
cargo build --release
```

`cargo test` doesn't touch the network or need WireGuard/admin - it's all
unit tests on the parsers/stats/CSV writer/scoring/selector, plus fakes
for `Probe`/`TunnelManager`. Actually exercising `connect`/`disconnect`/
`status`/`bench`/`monitor` end-to-end needs: WireGuard for Windows
installed, a real relay, and (for `connect`/`disconnect`/`monitor`
without `--dry-run`) an elevated terminal. `bench`/`monitor --dry-run`
can both be smoke-tested against public IPs without any of that (e.g.
`bench --direct-host 1.1.1.1 --tunnel-host 8.8.8.8`, or a `relays.toml`
whose `tunnel_host`s are public IPs) - that's how the ping-parser bug
above was actually found, and how `monitor`'s cooldown/hysteresis was
confirmed to behave correctly end-to-end before trusting the unit tests
alone.

## Phase 3 (routing/selector) - why it's a lib+bin crate now

This crate was binary-only (`src/main.rs` only) through Phase 2.
`tests/route_scoring.rs` and `tests/failover.rs` need to import
`routing::selector`, `traits::TunnelManager`, etc. from *outside* the
crate (integration tests are their own crate) - that's only possible
through a library target. So there's now a `src/lib.rs` that declares
every module `pub`, and `src/main.rs` does
`use route_agent::{bench, monitor, wireguard};` instead of its own `mod`
declarations. If you add a new module, declare it in `lib.rs`, not (only)
with a `mod` statement in `main.rs` - the build error if you forget
(`unresolved import` from `main.rs` or from a file under `tests/`) is
usually this.

Design choices in `routing/` worth knowing before changing them:

- `Selector::evaluate` is pure (no I/O) on purpose - `apply_decision` is
  the one place that touches a real (or faked) `TunnelManager`. Keep it
  that way; it's what makes `src/routing/selector.rs`'s own `#[cfg(test)]
  mod tests` possible without mocks for most of the logic.
- Scoring normalizes RTT/jitter/loss **min-max within the current
  round**, not against history - no baseline/calibration step needed,
  but it does mean a round with only one relay always scores it `0.0`
  (nothing to normalize against). That's expected, not a bug.
- `relays.toml`'s `tunnel_host` can repeat the same IP (e.g.
  `10.66.0.1`) across every relay - only one tunnel is ever active at a
  time, so there's no real collision; don't "fix" this into unique
  per-relay subnets unless you're also building concurrent multi-tunnel
  support (explicitly out of scope for now).
- `monitor --direct-host` is scored as a candidate named
  `relays::DIRECT_CANDIDATE` ("direct", reserved - `relays::load()`
  rejects a real relay using that name). `apply_decision` special-cases
  it: disconnect whatever's active, connect nothing.

## Real bug found and fixed: `ping()` turned "target down" into a hard error

While smoke-testing the direct-candidate feature above, two relays
pointed at unreachable IPs (203.0.113.1/.2, RFC 5737 TEST-NET) **vanished
from `monitor`'s round entirely** instead of showing up with
`loss_pct: 100.0`. Root cause: an earlier session's fix for "a bad
hostname shouldn't silently look like 100% loss" was too broad - it
treated *any* "zero replies + non-zero exit" as a hard error, but
`ping.exe` also exits non-zero for a **real, fully-timed-out** target,
which is exactly the measurement `monitor`'s hard-failover and
`direct`-candidate logic depend on. The two cases need the output
examined, not just the exit code, to tell apart: `ping()` now uses
`looks_like_a_real_attempt()` (counts non-empty lines - more than one
means it actually sent probes, one means an immediate DNS/usage
failure) instead of exit status. **If you touch `ping()` again, smoke-
test it against both a real public IP AND an RFC 5737 TEST-NET address
(203.0.113.0/24) in the same `monitor` run** - the unit tests use
synthetic transcripts for this exact case, but the bug only showed up
end-to-end, not in `cargo test`.

## GUI (`src/bin/route-agent-gui.rs`) - things that bit us

- **eframe 0.37 changed the `App` trait**: the required method is now
  `fn ui(&mut self, ui: &mut egui::Ui, frame)`, not `update(ctx, frame)`,
  and `CentralPanel::show` takes a `&mut Ui`, not a `&Context`. Most
  examples/docs online still show the old shape - check
  `eframe-0.37.0/src/epi.rs` in `~/.cargo/registry` instead of guessing.
- **Use `glow`, not eframe's default `wgpu`** - measured live: ~202 MB
  working set with wgpu vs ~117 MB with glow for the same window.
  `Cargo.toml` turns off default features; `winit/default` can't be
  listed from our crate (a dependency's dependency's feature), and isn't
  needed on Windows anyway.
- **A windowless (release) GUI flashes a console for every child
  process** it spawns (`sc.exe`, `ping.exe`, `schtasks.exe`,
  `powershell.exe`) unless they're created with `CREATE_NO_WINDOW` - that's
  what `syspath::hidden_command` is for. Use it for any new spawn the GUI
  can reach; harmless for the CLI since every call site captures via pipes.
- **Console output is CP850 on this pt-BR machine**, not UTF-8 -
  `from_utf8_lossy` turned "Não é possível" into "N�o � poss�vel" *in
  watch-game.log itself*. `console::decode` handles it; use it instead of
  `from_utf8_lossy` for stderr/stdout of Windows console tools.
- Refreshes run on a worker thread (`ping.exe` takes seconds; doing it in
  `ui()` froze the window), results come back over an `mpsc` channel.
- Task status is shown as schtasks' raw localized text (CSV column 3,
  positional) - deliberately not parsed, same lesson as `sc.exe`'s "ESTADO".
- **Seen live 2026-10-16, cause unknown**: all three RouteAgent tasks
  flipped between Ready and Disabled within minutes, while nothing in
  the session disabled them (`/end` doesn't disable). Suspects: Avast
  reacting to the freshly rebuilt, unsigned `route-agent.exe` that the
  tasks run elevated, or a manual change in Task Scheduler. If the
  buttons start failing with "não pôde ser executada porque está
  desabilitada", check `(Get-ScheduledTask -TaskName ...).Settings.Enabled`
  first.

## `watch-games` no longer logs a stack trace when a game isn't running

`detect_remote_ips` used `Get-Process -ErrorAction Stop`, so every idle
round (game closed, the normal state) wrote a multi-line PowerShell
exception per game into watch-game.log. Now the script exits with code 3
when no process matches, which maps to `AgentError::ProcessNotRunning`
and a one-line `"<game>: not running"`. Verified live against `cs2.exe`
(not running) and `steam.exe` (running) in the same session.

## Agent efficiency pass + `diagnose` (2026-10-16) - measured, not assumed

- `ping.exe -w 1000` (`bench::PROBE_TIMEOUT_MS`): 5 probes to a dead
  target took **23.8 s** by default vs **9.0 s** with it (live targets
  4.1 s either way). Windows still spaces probes ~1 s apart, so a dead
  target costs ~1.8 s/probe, not 1 s. Smoke-tested per the rule above:
  `monitor` with 1.1.1.1 + 203.0.113.1 in one run - the dead relay still
  shows up scored (penalty ~1000), it doesn't vanish.
- `bench::utc_timestamp` no longer spawns PowerShell (pure Rust in
  `agentlog`, Hinnant's civil-date algorithm). Self-report shows the
  long-running `watch-games` at ~5 MB working set / <1 MB private.
- `syspath::output_with_timeout` drains stdout/stderr on threads - a
  child blocked on a full pipe would otherwise never exit and get "timed
  out" while healthy. Used for detect-remote (20 s) and diagnose (60 s).
- `windiag.ps1` is embedded with `include_str!` and passed via
  `-Command` (not `-EncodedCommand` - that's the classic malware pattern
  AV heuristics watch for). A unit test enforces it stays ASCII and has
  no double quotes, which would need escaping on the command line.
- First real `diagnose` finding on this machine: the Realtek NIC runs
  Microsoft's generic inbox driver (v1.0.0.14, 2017). Temperature is not
  readable without admin here (`MSAcpi_ThermalZoneTemperature` empty).
- First `diagnose` run took 29 s with CS2 open (CPU 57%); steady state
  is ~5.5 s collection + ~1 s per gateway ping.
