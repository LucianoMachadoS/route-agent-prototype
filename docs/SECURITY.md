# Security review — route-agent

Threat model and hardening status for this project, written from the
perspective of: *what could leak a key, leak traffic, or compromise this
machine or the relay, and what have we actually done about it?*

Severity is about **this project's realistic use** (a personal relay you
rent, a tunnel scoped to `10.66.0.0/24`), not a generic worst case.

## What's already right by design (don't undo these)

- **No full-tunnel routing.** `AllowedIPs = 10.66.0.0/24` on the client and
  `10.66.0.2/32` on the relay mean a compromised or malicious relay cannot
  see or redirect your general internet traffic - only the private
  client↔relay tunnel subnet exists. Don't widen this to `0.0.0.0/0`
  without re-threat-modeling.
- **Keys never touch argv.** Every private key lives only in the `.conf`
  file; route-agent never takes a key as a CLI flag, so it never shows up
  in `tasklist`/Task Manager/process listings for other local users.
- **No shell interpolation.** Every subprocess call uses
  `Command::new(...).arg(...)` with each argument as its own array entry -
  never a formatted string handed to a shell. User-supplied values (hosts,
  tunnel names, paths) cannot inject a second command.
- **Cargo.lock is committed.** Dependency versions and checksums are
  pinned and verified by Cargo on every build.

## Fixed in this review

| # | Issue | Fix |
|---|---|---|
| 1 | **CWE-427 (uncontrolled search path):** `ping.exe`, `sc.exe`, `powershell.exe` were spawned by bare name. Windows searches the current working directory *before* `System32` for a bare filename, so running `route-agent.exe` from a folder where an attacker planted a same-named executable would run theirs instead - with Administrator rights for `connect`/`disconnect`. | `src/syspath.rs` resolves these via `%SystemRoot%\System32\...` and all call sites now use the fully-qualified path. |
| 2 | **Trusting `WIREGUARD_EXE`/`--wireguard` blindly before running it elevated.** A lower-privileged process can set a user environment variable; if that's inherited by a terminal you later elevate, `route-agent connect` would run an attacker-chosen binary as Administrator. | `warn_if_untrusted_wireguard_path()` prints a loud warning whenever the resolved path didn't come from our own hardcoded, admin-writable Program Files candidates. We can't safely auto-block a legitimate custom install, so the trust decision is made visible instead of silent. |
| 3 | **Plaintext private key left with inherited/default ACLs.** `wireguard.exe /installtunnelservice` does not delete or lock down the source `.conf` file. If it lives under e.g. `C:\ProgramData\...`, any local account can typically read it. | `restrict_config_permissions()` runs `icacls` right after a successful `connect`, dropping inherited ACEs and granting access only to `SYSTEM` and `Administrators` by well-known SID (`S-1-5-18`, `S-1-5-32-544` - locale-independent, unlike the group *names*, which are localized). Failure is a loud warning, not a fatal error, since the tunnel is already up either way. |
| 4 | **`bench`'s `ping()` silently reported "100% loss" when `ping.exe` itself failed** (bad hostname, DNS failure, bad flag) - indistinguishable from a real network problem, which is exactly the kind of wrong signal that leads to a bad routing decision. | Only treated as "loss" when at least the command executed; a run with zero parsed replies *and* a failing exit status now returns a real error instead. |
| 5 | **Unbounded `--count` on `bench`** could ping flood a host you don't control for an arbitrarily long time. | Clamped to `1..=1000` via clap's `value_parser`. |
| 6 | **No safety net against committing a real key.** `.gitignore` only stops a normal `git add`; `git add -f`, a key pasted into a doc, or a renamed file would slip through. | `.githooks/pre-commit` blocks committing any non-template `.conf` file and any line that looks like a real 44-char base64 `PrivateKey`/`PresharedKey` value, anywhere in the diff. Enable with `git config core.hooksPath .githooks` (already set in this clone). |

## Known limitations - not (yet) fixable in code

These depend on infrastructure this tool doesn't control (your relay,
your Windows install, your own operational habits), so they're
recommendations, not code:

- **Authenticode verification of `wireguard.exe` is not implemented.**
  The path-trust warning (#2 above) checks *location*, not the binary's
  signature. A fuller check would shell out to
  `Get-AuthenticodeSignature '<path>'` and require `Status -eq 'Valid'`
  and a publisher match before running it elevated. Reasonable next step
  if this moves past "prototype."
- **Relay (your Linux VPS) hardening is on you, and matters more than
  anything in this repo.** WireGuard itself doesn't respond to
  unauthenticated packets (no banner, no handshake without a valid key),
  so it's not the weak point - everything *else* on a quickly-provisioned
  VPS usually is:
  - Set a default-deny firewall and only open what you need:
    `ufw default deny incoming`, then `ufw allow 51820/udp` and your SSH
    port from a restricted source if possible.
  - SSH: key-only auth, disable password auth
    (`PasswordAuthentication no`), consider `fail2ban`.
  - Enable unattended security updates (`unattended-upgrades` on
    Debian/Ubuntu). An unpatched box with a public IP gets scanned and
    probed constantly regardless of what you run on it.
  - Consider a non-default `ListenPort` instead of `51820` - doesn't add
    real cryptographic security, but cuts down on casual mass-scan noise.
- **DNS-pinned relay endpoints become a bigger risk once selective
  game-traffic routing (README's milestone 3) exists.** Today, a hijacked
  `<RELAY_PUBLIC_IPV4_OR_DNS>` DNS record would redirect at most your
  10.66.0.0/24 tunnel probe traffic - low value to an attacker. Once the
  relay actually forwards real game traffic, a DNS hijack becomes a
  traffic-redirection/MITM vector. When you implement that milestone,
  either pin the relay's IP directly or add external monitoring on that
  DNS record.
- **`docs/runs.csv` (gitignored) can contain topology hints** - relay
  region/ISP if you put them in `--note`. It never leaves your machine via
  this tool, but don't paste raw rows into a public issue/forum post for
  support without trimming that column.
- **Error output can include local file paths.** `CommandFailed`/`Spawn`
  errors echo the failing command and its stderr/stdout verbatim. Nothing
  in this project writes a key there, but if you're pasting an error
  publicly for help, skim it for your Windows username in a path first.
- **No dependency vulnerability scanning is run automatically.** Run
  `cargo install cargo-audit && cargo audit` occasionally (or `cargo
  deny`) to catch known CVEs in `clap`/`thiserror`/`tempfile` or their
  transitive dependencies.

## If you think a key has already leaked

Rotate it - there is no way to "unleak" a WireGuard key. On the relay:
`wg genkey | tee relay-private.key | wg pubkey > relay-public.key`,
update `/etc/wireguard/wg0.conf`, `wg-quick down wg0 && wg-quick up wg0`.
On the client, generate a new keypair, update your local `.conf`, and
`route-agent disconnect` / `connect` again. A leaked *public* key alone is
not sensitive (that's its purpose), but if it leaked alongside which
relay it talks to, consider that relay's IP/hostname exposed too.
