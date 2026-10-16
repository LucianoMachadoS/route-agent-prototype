# route-agent — WireGuard tunnel prototype

A small Rust CLI that controls the **official WireGuard for Windows**
tunnel service. It does not implement WireGuard cryptography, install a
custom driver, capture packets, or route game traffic selectively. The
roadmap: a reliable tunnel between a Windows 10/11 client and a Linux
relay you control (`connect`/`disconnect`/`status`); measuring whether a
relay's route is actually better than your direct path (`bench`); and
automatically picking the best of several relays, with failover
(`monitor`) - see [`docs/BENCHMARK_PLAN.md`](docs/BENCHMARK_PLAN.md) for
how to actually validate that last part before trusting it.

## Safety / scope

- Use only a relay and networks you are authorized to administer.
- The example routes only `10.66.0.0/24` through the tunnel. It does
  **not** redirect all internet traffic or game traffic.
- Do not commit real private keys or populated `.conf` files to source
  control (`.gitignore` already excludes `*.conf`, keeping only
  `examples/*.conf.template`).
- `connect` and `disconnect` require an elevated Administrator terminal
  because they install/remove a Windows service.
- This prototype uses the official `wireguard.exe` CLI; it does not
  reproduce proprietary ExitLag (or any other third-party) code.
- See [`docs/SECURITY.md`](docs/SECURITY.md) for the full threat model:
  what's mitigated in code (safe subprocess calls, no keys in argv,
  locked-down config permissions, no PATH-hijackable binaries) and what's
  on you (relay hardening, key rotation if something leaks). Run
  `git config core.hooksPath .githooks` once per clone to enable a
  pre-commit check that blocks committing a real key by accident.

## Architecture

```text
Rust CLI (route-agent)
  ├── connect/disconnect/status
  │    └── TunnelManager (wireguard.rs) - std::process::Command, no shell
  │         └── official wireguard.exe / sc.exe
  │              └── WireGuardTunnel$<config-stem> Windows service
  │                   └── encrypted WireGuard UDP tunnel
  │                        └── Linux relay (wg-quick / wg)
  ├── health-check
  │    └── health.rs - TunnelManager::status() + Probe::measure() combined
  ├── bench
  │    └── Probe (bench.rs) - ping.exe (direct host, then tunnel host)
  │         └── median/p95/p99/jitter/loss -> docs/runs.csv
  └── monitor (Phase 3)
       ├── relays.rs            - loads relays.toml (candidates + weights)
       ├── routing::score       - weighted, normalized score per round
       ├── routing::selector    - rolling window, hysteresis, cooldown,
       │                          hard failover -> Decision
       └── apply_decision()     - disconnect old / connect new TunnelManager
            └── docs/decisions.csv
```

`traits.rs` defines `Probe`/`TunnelManager` so `routing::selector` can be
unit-tested with fakes (`tests/failover.rs`) instead of needing a real
network or an installed WireGuard client.

## Requirements

- Windows 10/11 x64.
- Rust stable and Cargo.
- Official WireGuard for Windows installed from
  https://www.wireguard.com/install/
- A Linux relay with a public IP, WireGuard tools, and UDP port 51820
  permitted by its firewall.
- Administrator PowerShell for tunnel service management (`connect` /
  `disconnect`). `status` and `bench` don't need elevation.

## Build and test on Windows

From this directory:

```powershell
cargo fmt --check
cargo test
cargo build --release
```

The executable will be `target\release\route-agent.exe`.

## Configure the Linux relay

On a Linux server you control (Ubuntu/Debian example):

```bash
sudo apt update
sudo apt install wireguard
umask 077
wg genkey | tee relay-private.key | wg pubkey > relay-public.key
wg genkey | tee client-private.key | wg pubkey > client-public.key
```

Keep `relay-private.key` and `client-private.key` secret. Copy only the
**client public key** to the relay and only the **relay public key** to the
Windows client. Transfer private keys securely to their respective hosts;
do not paste them into chat or commit them.

Create `/etc/wireguard/wg0.conf` based on
`examples/relay-wg0.conf.template`, filling in the relay private key and
client public key:

```bash
sudo chmod 600 /etc/wireguard/wg0.conf
sudo systemctl enable --now wg-quick@wg0
sudo wg show
```

Allow inbound UDP/51820 in the provider firewall and host firewall. For
UFW, if enabled:

```bash
sudo ufw allow 51820/udp
```

This prototype does not enable IP forwarding or NAT because the goal is
only to reach the relay's tunnel address, not to use it as an internet
gateway.

## Configure the Windows client

1. Copy `examples/client.conf.template` to a private local folder, e.g.
   `C:\ProgramData\RouteAgent\route-client.conf`.
2. Fill in the client private key, relay public key, and relay public
   endpoint. Do not leave angle-bracket placeholders.
3. Keep the config private. Restrict file permissions to Administrators
   and SYSTEM.
4. In an **Administrator PowerShell** opened in this repository, run:

```powershell
$env:WIREGUARD_EXE = "C:\Program Files\WireGuard\wireguard.exe"
.\target\release\route-agent.exe connect --config "C:\ProgramData\RouteAgent\route-client.conf"
```

If WireGuard is installed elsewhere, either set `WIREGUARD_EXE` or pass
`--wireguard "<path>"` on the command line (the flag takes precedence over
the environment variable; if neither is given, `route-agent` looks in the
standard install locations and then `PATH`).

## Verify

```powershell
.\target\release\route-agent.exe status --name route-client
ping 10.66.0.1
```

Or do both at once with `health-check`, which exits non-zero if either
check fails (scriptable):

```powershell
.\target\release\route-agent.exe health-check --name route-client --tunnel-host 10.66.0.1
```

Note what this does **not** verify: there's no documented way to query a
live WireGuard handshake/peer state from the official Windows client's
CLI (no bundled `wg.exe` like Linux's `wg show`), so `health-check` is
"service running + relay address pings" - a solid practical signal, but
not a cryptographic handshake check.

On the relay:

```bash
sudo wg show
```

A successful handshake should appear after the client sends traffic. If
ping fails, verify the relay's public IP/DNS, UDP firewall rules,
public/private key placement, tunnel addresses, and that the endpoint is
reachable. ICMP may also be blocked by a host firewall even when the
WireGuard handshake works.

## Disconnect

Run from an Administrator PowerShell:

```powershell
.\target\release\route-agent.exe disconnect --name route-client
```

## Measure direct vs. tunnel latency (`bench`)

Once the tunnel is connected, compare your ISP's direct route to a game
server against the route through your relay, following the protocol in
[`docs/experiments.md`](docs/experiments.md):

```powershell
.\target\release\route-agent.exe bench `
  --direct-host <ip-ou-host-do-servidor-do-jogo> `
  --tunnel-host 10.66.0.1 `
  --count 30 `
  --note "relay=fra1,isp=vivo-fibra"
```

This pings both hosts, prints median/p95/p99/jitter/loss for each, and
appends one row to `docs/runs.csv` so you can compare runs over time while
changing one variable at a time (relay region, time of day, ISP, etc.).
`bench` does not require Administrator and does not touch the tunnel
service — run it before and after `connect` to see the difference, and
across different relays to decide if a given relay is worth using for a
given game server.

## Automatic relay selection and failover (`monitor`)

Once you have two or more relays worth comparing, `monitor` does
continuously what `bench` does once: probe every candidate, score them,
and switch the active tunnel to whichever one wins - with hysteresis (no
flapping over noise) and a fast path for outright failure.

1. Copy [`relays.toml.template`](relays.toml.template) to `relays.toml`
   and list your relays (each `config` points at a real `.conf`, same as
   `connect` takes):

   ```toml
   [[relay]]
   name = "fra1"
   config = "C:/ProgramData/RouteAgent/fra1.conf"
   tunnel_host = "10.66.0.1"
   ```

2. Run it (Administrator, since it calls `connect`/`disconnect` under the
   hood unless `--dry-run`):

   ```powershell
   .\target\release\route-agent.exe monitor `
     --relays relays.toml `
     --direct-host <ip-ou-host-do-servidor-do-jogo> `
     --interval 30
   ```

   Add `--dry-run` to only print/log what it *would* do, and `--rounds N`
   to stop after N rounds instead of running forever (useful for a quick
   check without a real relay - see the smoke test in
   [`docs/BENCHMARK_PLAN.md`](docs/BENCHMARK_PLAN.md)).

Every round's scores and whether a switch happened are appended to
`docs/decisions.csv`. `--direct-host` is scored as a candidate too, named
`direct` (reserved - a relay can't use that name): if every configured
relay is worse than just staying on the direct connection, `monitor`
disconnects any active tunnel instead of keeping a losing relay active
just because it was the best of the configured ones.

## Routing real game traffic through the relay (NAT)

By default the tunnel only reaches the relay's own tunnel address
(`AllowedIPs = 10.66.0.0/24`) - nothing else gets routed through it, by
design (see "Safety / scope"). To actually route a game's traffic
through the relay, two things both have to happen, on two different
machines:

1. **Windows client**: add the destination IP/range to the `.conf`'s
   `AllowedIPs` - this is what `add-game-route`/`watch-game` automate
   (see above).
2. **Linux relay**: allow forwarding *to that same range*, with NAT so
   the return traffic can find its way back. This part is **manual** -
   route-agent doesn't manage SSH access to a relay in code.

On the relay, for each IP range you want to allow (replace
`10.66.0.2/32` with your client's tunnel address, `205.185.192.0/19` with
the actual range, and `ens4` with the relay's real external interface -
check with `ip route show default`):

```bash
# One-time: allow forwarding at all.
echo 'net.ipv4.ip_forward=1' | sudo tee /etc/sysctl.d/99-wireguard-forward.conf
sudo sysctl -p /etc/sysctl.d/99-wireguard-forward.conf

# NAT: insert into the *nat table* at the very top of /etc/ufw/before.rules
# (before the existing *filter table), one block, ending in COMMIT:
#   *nat
#   :POSTROUTING ACCEPT [0:0]
#   -A POSTROUTING -s 10.66.0.2/32 -d 205.185.192.0/19 -o ens4 -j MASQUERADE
#   COMMIT
# Careful: a before.rules file normally has *two* "COMMIT" lines (nat and
# filter) - inserting with a naive `sed '/^COMMIT$/i ...'` matches both
# and corrupts the filter table ("No chain/target/match by that name" on
# the next `ufw enable`). Insert by line number or prepend a whole block
# instead, and always check `sudo ufw status verbose` plus
# `sudo iptables -t nat -L POSTROUTING -n -v` afterward.

# Allow the actual forwarding, scoped to exactly this client and range:
sudo ufw route allow in on wg0 out on ens4 from 10.66.0.2 to 205.185.192.0/19
sudo ufw disable && sudo ufw --force enable
```

If you add a NAT line to `before.rules` more than once without flushing
first, `iptables -t nat -L POSTROUTING` accumulates duplicate (harmless
but messy) entries on every `ufw` reload - flush and reapply cleanly with
`sudo bash -c 'iptables -t nat -F POSTROUTING && iptables-restore -n -T nat < /etc/ufw/before.rules'`.

**Finding the right range isn't obvious.** A game server's IP changes
per match, and (for CS2 at least) isn't even the real game server - it's
a Valve SDR relay point-of-presence (check the game's own console: CS2's
`status`/`net_graph 1` shows lines like `Selecting gru#1
(155.133.227.35:27015) as primary` - `155.133.0.0/16` is Valve's own
relay network, a *different* range than their older, pre-SDR matchmaking
IP ranges that are documented on GitHub). Verify with a live match
(`detect-remote`) rather than trusting a community IP list blindly - see
`docs/SECURITY.md` for why a found-online range should be treated as a
starting hypothesis, not ground truth.

## CLI reference

```text
route-agent connect    --config <PATH> [--wireguard <PATH>]
route-agent disconnect --name <TUNNEL_NAME> [--wireguard <PATH>]
route-agent status     --name <TUNNEL_NAME>
route-agent health-check --name <TUNNEL_NAME> --tunnel-host <HOST> [--count <N>]
route-agent bench      --direct-host <HOST> [--tunnel-host <HOST>] [--count <N>] [--note <TEXT>] [--csv <PATH>]
route-agent monitor    --relays <PATH> --direct-host <HOST> [--count <N>] [--interval <SECS>] [--rounds <N>] [--dry-run] [--csv <PATH>] [--wireguard <PATH>]
route-agent detect-remote   --process <NAME>
route-agent add-game-route  --config <PATH> --name <TUNNEL_NAME> --process <NAME> [--cidr <PREFIX>] [--wireguard <PATH>]
route-agent watch-game      --config <PATH> --name <TUNNEL_NAME> --process <NAME> [--cidr <PREFIX>] [--interval <SECS>] [--rounds <N>] [--wireguard <PATH>]
route-agent watch-games     --games <PATH> [--interval <SECS>] [--rounds <N>] [--wireguard <PATH>]
route-agent diagnose        [--gateway-pings <N>] [--json]
```

`monitor`, `watch-game` and `watch-games` also take `--log <FILE>
[--log-max-kb <KB>]`: timestamped lines go to that file instead of
stdout, rotated to `<FILE>.1`/`<FILE>.2` once it passes the size (default
1024 KB). Every ~5 minutes they also log their own CPU/RAM
(`self: cpu 0.01% ram 5.2 MB ...`). `scripts\install.ps1` migrates old
`.bat` files that used `>> file 2>&1` to this automatically.

`diagnose` is read-only and needs no admin: network adapter (cable vs
Wi-Fi, link speed, driver - including flagging Microsoft's generic inbox
driver on third-party NICs), latency/jitter/loss to your own router,
CPU/RAM/disk, GPU/VRAM, temperature when Windows exposes it, Game Bar
background capture, power plan and startup apps - each with an OK / INFO
/ AVISO verdict. `--json` prints the raw data instead.

Options:
- `--wireguard <PATH>` selects `wireguard.exe` for `connect`/`disconnect`/`monitor`/`add-game-route`/`watch-game`/`watch-games`.
- `health-check --count` defaults to 5 probes.
- Alternatively set the `WIREGUARD_EXE` environment variable. The flag
  wins if both are set; if neither is set, the standard install locations
  and then `PATH` are searched.
- `bench --tunnel-host` defaults to `10.66.0.1` (the relay's tunnel
  address from `examples/relay-wg0.conf.template`).
- `bench --count` / `monitor --count` default to 20 / 10 probes per host.
- `bench --csv` defaults to `docs/runs.csv`; `monitor --csv` defaults to
  `docs/decisions.csv`.
- `monitor --interval` defaults to 30 seconds between rounds;
  `watch-game --interval` defaults to 20.
- `add-game-route`/`watch-game --cidr <N>` expands each detected server
  IP to its containing `/N` instead of an exact `/32`.

## Automatic selective routing (`detect-remote` / `add-game-route` / `watch-game`)

README milestone 3 used to mean manually finding a game's current server
IP (`Get-NetUDPEndpoint`), editing `AllowedIPs` by hand, and reconnecting.
route-agent now does this itself:

```powershell
# Just see what IP(s) a game process is currently talking to (read-only,
# no admin needed) - only meaningful while a match is in progress:
.\target\release\route-agent.exe detect-remote --process cs2.exe

# Detect + add to AllowedIPs + reconnect, once:
.\target\release\route-agent.exe add-game-route --config saopaulo-gcp.conf --name saopaulo-gcp --process cs2.exe --cidr 24

# Or keep doing that automatically while you play, instead of re-running
# it by hand every time you're matched into a different server:
.\target\release\route-agent.exe watch-game --config saopaulo-gcp.conf --name saopaulo-gcp --process cs2.exe --cidr 24
```

**For more than one game**, copy [`games.toml.template`](games.toml.template)
to `games.toml` and list each `[[game]]` once (process, config, tunnel
name, optional CIDR). Adding a future game/app is then just adding one
more entry there - no code changes, no new Scheduled Task:

```powershell
.\target\release\route-agent.exe watch-games --games games.toml
```

This watches every registered game in one loop, in the order listed,
each round.

`add-game-route`/`watch-game` need the same Administrator elevation as
`connect` (they reconnect the tunnel). To avoid opening an elevated
terminal every single time, see
[`scripts/scheduled-tasks/README.md`](scripts/scheduled-tasks/README.md) -
a Scheduled Task created once with `/rl HIGHEST` can be triggered and
stopped from a completely unprivileged session afterward, with no further
UAC prompts, while still only ever running the exact fixed command it was
registered with.

**Not automated**: the relay's NAT rules for whatever IP gets detected -
this project doesn't manage SSH access to relays in code. If a detected
server falls outside the ranges you've already allowed on the relay
(see "Routing real game traffic" below), the packet just goes nowhere
useful - no error, just no benefit, until you add a matching NAT rule on
the relay by hand.

## Desktop window (`route-agent-gui`)

```powershell
cargo build --release
# elevated: installs both exes to Program Files, games.toml to ProgramData,
# locks down ACLs, adds a "RouteAgent" Start menu shortcut. Re-run after
# every rebuild - the Scheduled Tasks only ever run the installed copy.
.\scripts\install.ps1
```

Shows the tunnel's service state plus RTT/jitter/loss to `10.66.0.1`
(override with `--tunnel-host`), the games in `games.toml` (found next to
the exe or up to four folders above it, or pass `--games`), the
RouteAgent Scheduled Tasks' status, and the tail of
`%LOCALAPPDATA%\RouteAgent\watch-game.log`. Refreshes every 5 s.

It never runs elevated: the Connect/Disconnect/detection buttons only
`schtasks /run` (or `/end`) the fixed-name tasks from
`scripts/scheduled-tasks/` - register those first, or the buttons just
report that the task doesn't exist. A task shown as "Desabilitado"/
"Disabled" refuses `/run` until it's re-enabled in Task Scheduler.

## Current limitations

- No relay discovery, game/process detection, UI, or multipath - all
  still deferred per the project roadmap.
- `status`/`health-check` report Windows service state + ICMP
  reachability, not a cryptographic handshake check - there is no
  documented way to query that from the official Windows client's CLI.
- No full-tunnel `AllowedIPs = 0.0.0.0/0`; this is intentional to avoid
  unexpectedly redirecting all network traffic.
- `bench`/`monitor` measure ICMP latency/jitter/loss, which correlates
  with but is not identical to a given game's actual (often UDP) traffic
  performance.
- The scoring weights in `relays.toml.template` are starting defaults, not
  tuned values - see `docs/BENCHMARK_PLAN.md` before trusting a specific
  relay choice.
- The CLI checks input paths and tunnel names, but this is a learning
  prototype, not a production security boundary.
- Windows service state can take a moment to settle after install/uninstall.

## Next milestones

1. ~~Add a `health-check` that tests the tunnel endpoint~~ done - see
   above (it's a practical proxy, not a real handshake check, since the
   Windows client doesn't expose one via CLI). A broader structured-
   logging pass (e.g. the `tracing` crate) across every subcommand is
   still open if that becomes worth it.
2. ~~Let `monitor` treat "no tunnel" as a selectable candidate~~ done -
   the direct host is scored as a `direct` candidate; see "Automatic
   relay selection and failover" above.
3. ~~Add explicit consent/configuration for selective application
   routing~~ half done - `detect-remote`/`add-game-route`/`watch-game`
   automate the Windows side (see "Automatic selective routing" above).
   The relay side (NAT/forwarding for the detected IP) is still a manual
   procedure (see "Routing real game traffic through the relay"); making
   route-agent manage that too would mean it starts handling SSH access
   to relays, which is new scope, not yet decided.
4. Only then explore multipath in a controlled test environment, and a
   native (egui) GUI once Phase 3 is validated per
   `docs/BENCHMARK_PLAN.md`.
