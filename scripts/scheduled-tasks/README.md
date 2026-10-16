# Running route-agent without re-elevating every time

`connect`/`disconnect`/`add-game-route`/`watch-game` all need Administrator
(installing/removing a Windows service). Windows's UAC means being an
admin account doesn't make every process you run elevated - you get a
non-elevated "filtered" token by default, and need an explicit consent
prompt to get the elevated one. That means a normal terminal has to be
opened with "Run as Administrator" *every time* you want to run one of
these commands - real friction if you want to, say, trigger `watch-game`
from a script, a hotkey, or anything that isn't you manually opening an
elevated window.

## The trick: a Scheduled Task with `/rl HIGHEST`

A Windows Scheduled Task created with `/rl HIGHEST` runs with full
Administrator rights **without a UAC prompt each time it fires** - the
one-time consent happens at *creation* (which does need an elevated
session), not at every run. After that, triggering it
(`schtasks /run /tn "<name>"`) and stopping it (`schtasks /end /tn
"<name>"`) work from a completely unprivileged session - the Task
Scheduler service (which already runs as SYSTEM) handles the elevation
internally.

## Why this is safe (and how to not make it unsafe)

**The task's action is a fixed string set at creation time** (a specific
`.bat` file with hardcoded arguments) - creating or editing that
definition still requires admin. A non-elevated process can only *start*
or *stop* the exact thing that was already authorized; it cannot change
*what* runs.

**Do not** build a "generic" version of this that reads its command from
a file an unprivileged process can write to (e.g., "write the command you
want to `pending.txt`, then trigger the task to run whatever's in it").
That turns a convenience feature into a privilege-escalation path: any
unprivileged process (including malware) could then get arbitrary code
run as Administrator. See docs/SECURITY.md's note on `WIREGUARD_EXE` for
the same pattern in a different place. One task per fixed action, always.

## Setup (one-time, needs an elevated terminal)

1. Copy a template below to `C:\ProgramData\RouteAgent\<name>.bat` and
   fill in your actual paths/relay/process name.
2. Register it (`/sc ONCE /st 00:00 /sd 01/01/2020` schedules it for a
   date in the past, purely so it never fires on its own - it only ever
   runs when you explicitly `/run` it):

   ```powershell
   schtasks /create /tn "RouteAgent-<Name>" /tr "C:\ProgramData\RouteAgent\<name>.bat" /sc ONCE /st 00:00 /sd 01/01/2020 /rl HIGHEST /f
   ```

## Day-to-day use (no elevation needed from here on)

```powershell
schtasks /run /tn "RouteAgent-<Name>"                       # start it
schtasks /query /tn "RouteAgent-<Name>" /v /fo list          # check status
schtasks /end /tn "RouteAgent-<Name>"                        # stop it (for a looping task like watch-game)
Get-Content "$env:LOCALAPPDATA\RouteAgent\<name>.log" -Tail 20   # see what it's doing
```

Output doesn't show up anywhere visible - the `.bat` templates redirect
stdout/stderr to a log file under `%LOCALAPPDATA%\RouteAgent\`, since a
Scheduled Task has no console. Check the log, not the task's own output.

## Templates

- [`connect.bat.template`](connect.bat.template) - bring up a specific tunnel
- [`disconnect.bat.template`](disconnect.bat.template) - tear it down
- [`watch-game.bat.template`](watch-game.bat.template) - continuously
  detect **one** game's current server and keep `AllowedIPs` (and the
  tunnel) up to date while you play
- [`watch-games.bat.template`](watch-games.bat.template) - same, but for
  every game listed in `games.toml` at once (see
  `../../games.toml.template`). **Prefer this one** once you have more
  than one game/app to watch: adding a future game is then just a new
  `[[game]]` entry in `games.toml`, not a new `.bat` + a new task.

Make one `.bat` + one Scheduled Task per relay you actually use (or one
`watch-games` task covering your whole `games.toml`) - don't try to
parameterize a single task at runtime (see the security note above for
why).
