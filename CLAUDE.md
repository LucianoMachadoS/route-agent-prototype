# CLAUDE.md — route-agent

Context for working on this project in future sessions. Don't duplicate
`README.md` (setup/usage) or `docs/SECURITY.md` (threat model) here -
this is for things that bit us and aren't obvious from reading the code.

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
cargo test            # 15 tests as of this writing, all pure/offline
cargo build --release
```

`cargo test` doesn't touch the network or need WireGuard/admin - it's all
unit tests on the parsers/stats/CSV writer. Actually exercising
`connect`/`disconnect`/`status`/`bench` end-to-end needs: WireGuard for
Windows installed, a real relay, and (for `connect`/`disconnect`) an
elevated terminal. `bench` alone can be smoke-tested against any two
public IPs without any of that (e.g. `--direct-host 1.1.1.1 --tunnel-host
8.8.8.8`) - that's how the ping-parser bug above was actually found.
