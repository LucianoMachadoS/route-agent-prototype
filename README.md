# route-agent — WireGuard tunnel prototype

A small Rust CLI that controls the **official WireGuard for Windows**
tunnel service. It does not implement WireGuard cryptography, install a
custom driver, capture packets, or route game traffic selectively. The
first milestone is a reliable tunnel between a Windows 10/11 client and a
Linux relay you control; the second is measuring whether that relay's route
is actually better than your direct path.

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

## Architecture

```text
Rust CLI (route-agent)
  ├── connect/disconnect/status
  │    └── std::process::Command (no shell)
  │         └── official wireguard.exe / sc.exe
  │              └── WireGuardTunnel$<config-stem> Windows service
  │                   └── encrypted WireGuard UDP tunnel
  │                        └── Linux relay (wg-quick / wg)
  └── bench
       └── ping.exe (direct host, then tunnel host)
            └── median/p95/p99/jitter/loss -> docs/runs.csv
```

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

## CLI reference

```text
route-agent connect    --config <PATH> [--wireguard <PATH>]
route-agent disconnect --name <TUNNEL_NAME> [--wireguard <PATH>]
route-agent status     --name <TUNNEL_NAME>
route-agent bench      --direct-host <HOST> [--tunnel-host <HOST>] [--count <N>] [--note <TEXT>] [--csv <PATH>]
```

Options:
- `--wireguard <PATH>` selects `wireguard.exe` for `connect`/`disconnect`.
- Alternatively set the `WIREGUARD_EXE` environment variable. The flag
  wins if both are set; if neither is set, the standard install locations
  and then `PATH` are searched.
- `bench --tunnel-host` defaults to `10.66.0.1` (the relay's tunnel
  address from `examples/relay-wg0.conf.template`).
- `bench --count` defaults to 20 probes per host.
- `bench --csv` defaults to `docs/runs.csv`.

## Current limitations

- No automatic route scoring, relay discovery, game/process detection, UI,
  auto-reconnect, or multipath.
- `status` reports Windows service state, not a cryptographic handshake or
  end-to-end health check.
- No full-tunnel `AllowedIPs = 0.0.0.0/0`; this is intentional to avoid
  unexpectedly redirecting all network traffic.
- `bench` measures ICMP latency/jitter/loss, which correlates with but is
  not identical to a given game's actual (often UDP) traffic performance.
- The CLI checks input paths and tunnel names, but this is a learning
  prototype, not a production security boundary.
- Windows service state can take a moment to settle after install/uninstall.

## Next milestones

1. Add structured logs and a `health-check` that tests the tunnel endpoint
   (a real WireGuard handshake check, not just service state).
2. ~~Add relay telemetry (latency/jitter/loss)~~ done via `bench`; still
   missing: a route scoring library that picks the best relay automatically
   from a set of candidates.
3. Add explicit consent/configuration for selective application routing,
   using documented Windows networking mechanisms (this needs NAT/
   forwarding on the relay for the game server's IP, not just the client's
   tunnel `/32`, which the relay config does not yet allow).
4. Only then explore failover and multipath in a controlled test
   environment.
