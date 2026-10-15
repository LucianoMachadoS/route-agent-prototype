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
`docs/decisions.csv`. The direct host is only printed for reference each
round - it isn't (yet) something the selector can pick, so if every relay
is worse than your direct path that only shows up by reading the numbers
yourself, not automatically.

## CLI reference

```text
route-agent connect    --config <PATH> [--wireguard <PATH>]
route-agent disconnect --name <TUNNEL_NAME> [--wireguard <PATH>]
route-agent status     --name <TUNNEL_NAME>
route-agent health-check --name <TUNNEL_NAME> --tunnel-host <HOST> [--count <N>]
route-agent bench      --direct-host <HOST> [--tunnel-host <HOST>] [--count <N>] [--note <TEXT>] [--csv <PATH>]
route-agent monitor    --relays <PATH> --direct-host <HOST> [--count <N>] [--interval <SECS>] [--rounds <N>] [--dry-run] [--csv <PATH>] [--wireguard <PATH>]
```

Options:
- `--wireguard <PATH>` selects `wireguard.exe` for `connect`/`disconnect`/`monitor`.
- `health-check --count` defaults to 5 probes.
- Alternatively set the `WIREGUARD_EXE` environment variable. The flag
  wins if both are set; if neither is set, the standard install locations
  and then `PATH` are searched.
- `bench --tunnel-host` defaults to `10.66.0.1` (the relay's tunnel
  address from `examples/relay-wg0.conf.template`).
- `bench --count` / `monitor --count` default to 20 / 10 probes per host.
- `bench --csv` defaults to `docs/runs.csv`; `monitor --csv` defaults to
  `docs/decisions.csv`.
- `monitor --interval` defaults to 30 seconds between rounds.

## Current limitations

- `monitor` only scores the relays in `relays.toml` against each other -
  it doesn't compare against "no tunnel at all", so a direct connection
  that beats every relay isn't detected automatically (see
  `docs/BENCHMARK_PLAN.md`).
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
2. Let `monitor` treat "no tunnel" as a selectable candidate, so a relay
   that's worse than the direct path doesn't get used just because it's
   the best of the configured relays.
3. Add explicit consent/configuration for selective application routing,
   using documented Windows networking mechanisms (this needs NAT/
   forwarding on the relay for the game server's IP, not just the client's
   tunnel `/32`, which the relay config does not yet allow).
4. Only then explore multipath in a controlled test environment, and a
   native (egui) GUI once Phase 3 is validated per
   `docs/BENCHMARK_PLAN.md`.
