#Requires -Version 5.1
<#
.SYNOPSIS
  Installs route-agent where the elevated Scheduled Tasks can safely run it.

.DESCRIPTION
  The RouteAgent-* Scheduled Tasks run with /rl HIGHEST. Whatever they
  execute - the .bat/.ps1 under %ProgramData%\RouteAgent, route-agent.exe,
  and the games.toml watch-games reads - must therefore NOT be writable by
  a non-elevated process, or any program running as the normal user could
  edit it and get Administrator on the next task run (local privilege
  escalation). Before this script, all of them were: the exe/games.toml
  lived under the user's Desktop, and the ProgramData files were owned by
  (and full-control for) the user.

  This script (run elevated, re-runnable after every `cargo build --release`):
    1. copies route-agent.exe + route-agent-gui.exe to %ProgramFiles%\RouteAgent
    2. copies the repo's games.toml to %ProgramData%\RouteAgent
    3. makes Administrators the owner of all of it; SYSTEM/Administrators
       full control, Users read-only - and *.conf / *.ps1 (they contain
       WireGuard private keys) SYSTEM/Administrators only
    4. repoints the task .bat files at the installed copies (no task
       re-registration needed - a task's action is just the .bat path)
    5. verifies no non-admin principal has write access, and fails if so

  Group names are localized ("Administradores" on pt-BR), so every ACL
  uses well-known SIDs. Keep this file ASCII-only: Windows PowerShell 5.1
  misreads a BOM-less UTF-8 script.

  This script itself lives in the (user-writable) repo and runs elevated:
  check `git diff` / `git status` before running it, as with any script
  you run as Administrator.
#>
[CmdletBinding()]
param(
    [string]$RepoRoot
)

$ErrorActionPreference = 'Stop'
# Not a param default: Windows PowerShell 5.1 leaves $PSScriptRoot empty
# inside param() when run via -File.
if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
}

$logDir = Join-Path $env:LOCALAPPDATA 'RouteAgent'
New-Item -ItemType Directory -Force $logDir | Out-Null
Start-Transcript -Path (Join-Path $logDir 'install.log') -Force | Out-Null

try {
    $principal = New-Object Security.Principal.WindowsPrincipal(
        [Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run this script from an elevated (Run as Administrator) session.'
    }

    $icacls = Join-Path $env:SystemRoot 'System32\icacls.exe'
    $powershellExe = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $sidSystem = 'S-1-5-18'
    $sidAdmins = 'S-1-5-32-544'
    $sidUsers = 'S-1-5-32-545'
    $sidTrustedInstaller = 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464'

    $installDir = Join-Path $env:ProgramFiles 'RouteAgent'
    $dataDir = Join-Path $env:ProgramData 'RouteAgent'

    function Invoke-Icacls {
        param([Parameter(ValueFromRemainingArguments = $true)][string[]]$IcaclsArgs)
        $out = & $icacls @IcaclsArgs 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "icacls $($IcaclsArgs -join ' ') failed ($LASTEXITCODE): $out"
        }
    }

    # Directory: owner Administrators; SYSTEM/Admins full, Users read, all
    # inherited by children. Drops everything else (incl. the user's own
    # full-control ACE and Users' create-files right on ProgramData).
    function Protect-Directory([string]$dir) {
        Invoke-Icacls $dir /setowner "*$sidAdmins" /T /C
        Invoke-Icacls $dir /inheritance:r /grant:r `
            "*${sidSystem}:(OI)(CI)F" "*${sidAdmins}:(OI)(CI)F" "*${sidUsers}:(OI)(CI)RX"
        Get-ChildItem -LiteralPath $dir -File | ForEach-Object {
            Invoke-Icacls $_.FullName /reset
        }
    }

    # Files holding private keys: SYSTEM/Admins only, nothing inherited.
    function Protect-Secret([string]$file) {
        Invoke-Icacls $file /inheritance:r /grant:r "*${sidSystem}:F" "*${sidAdmins}:F"
    }

    # --- 0. stop anything holding the installed exe open ------------------
    Get-ScheduledTask -TaskName 'RouteAgent-*' -ErrorAction SilentlyContinue |
        Where-Object { $_.State -eq 'Running' } |
        ForEach-Object {
            Write-Host "Stopping running task $($_.TaskName) (start it again afterwards)"
            Stop-ScheduledTask -TaskName $_.TaskName
        }
    Get-Process -Name 'route-agent-gui' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -like "$installDir\*" } |
        Stop-Process -Force

    # --- 1. binaries -------------------------------------------------------
    $exes = 'route-agent.exe', 'route-agent-gui.exe'
    foreach ($exe in $exes) {
        $src = Join-Path $RepoRoot "target\release\$exe"
        if (-not (Test-Path -LiteralPath $src)) {
            throw "Missing $src - run 'cargo build --release' first."
        }
    }
    New-Item -ItemType Directory -Force $installDir | Out-Null
    foreach ($exe in $exes) {
        Copy-Item -LiteralPath (Join-Path $RepoRoot "target\release\$exe") `
            -Destination (Join-Path $installDir $exe) -Force
        Write-Host "Installed $installDir\$exe"
    }
    Protect-Directory $installDir

    # --- 2+3. data dir -----------------------------------------------------
    if (-not (Test-Path -LiteralPath $dataDir)) {
        throw "$dataDir does not exist - nothing to repoint (set up the tasks first, see scripts/scheduled-tasks/README.md)."
    }
    $repoGames = Join-Path $RepoRoot 'games.toml'
    $dataGames = Join-Path $dataDir 'games.toml'
    if (Test-Path -LiteralPath $repoGames) {
        Copy-Item -LiteralPath $repoGames -Destination $dataGames -Force
        Write-Host "Copied games.toml -> $dataGames"
    }
    Protect-Directory $dataDir
    Get-ChildItem -LiteralPath $dataDir -File |
        Where-Object { $_.Extension -in '.conf', '.ps1' } |
        ForEach-Object {
            Protect-Secret $_.FullName
            Write-Host "Restricted to SYSTEM/Administrators: $($_.Name)"
        }

    # --- 4. repoint the task .bat files -----------------------------------
    $oldExe = Join-Path $RepoRoot 'target\release\route-agent.exe'
    $newExe = Join-Path $installDir 'route-agent.exe'
    foreach ($bat in Get-ChildItem -LiteralPath $dataDir -Filter '*.bat' -File) {
        $text = [IO.File]::ReadAllText($bat.FullName)
        $new = [regex]::Replace($text, [regex]::Escape($oldExe), $newExe.Replace('$', '$$'), 'IgnoreCase')
        $new = [regex]::Replace($new, [regex]::Escape($repoGames), $dataGames.Replace('$', '$$'), 'IgnoreCase')
        # Bare "powershell" is resolved via PATH/cwd - same CWE-427 issue
        # syspath.rs fixes on the Rust side.
        $new = [regex]::Replace($new, '(?im)^powershell(\.exe)?(?=\s)', "`"$powershellExe`"")
        # Long-running loops used to log via `>> "<file>" 2>&1` - no
        # timestamps, no size limit. Switch them to the exe's own rotating
        # --log; stderr (only used if the log itself can't be written, or on
        # a crash) goes to <file>.stderr. Lines that already use --log are
        # left alone, so this is idempotent.
        $new = [regex]::Replace($new,
            '(?im)^(?<cmd>.*route-agent\.exe"?\s+(?:watch-games?|monitor)\s(?:(?!--log).)*?)\s*>>\s*"(?<log>[^"]+)"\s*2>&1[ \t]*$',
            '${cmd} --log "${log}" >nul 2>> "${log}.stderr"')
        if ($new -ne $text) {
            [IO.File]::WriteAllText($bat.FullName, $new, [Text.Encoding]::ASCII)
            Write-Host "Repointed $($bat.Name)"
        }
        if ($new -match [regex]::Escape($RepoRoot)) {
            Write-Warning "$($bat.Name) still references $RepoRoot - edit it by hand."
        }
    }

    # --- start menu shortcut for the GUI ----------------------------------
    $lnk = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\RouteAgent.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $sc = $shell.CreateShortcut($lnk)
    $sc.TargetPath = Join-Path $installDir 'route-agent-gui.exe'
    $sc.WorkingDirectory = $installDir
    $sc.Save()
    Write-Host "Start menu shortcut: $lnk"

    # --- 5. verify -------------------------------------------------------
    $trusted = @($sidSystem, $sidAdmins, $sidTrustedInstaller)
    $writeMask = [int]([Security.AccessControl.FileSystemRights]'WriteData, AppendData, WriteExtendedAttributes, WriteAttributes, DeleteSubdirectoriesAndFiles, Delete, ChangePermissions, TakeOwnership')
    $problems = 0
    $targets = @(Get-Item -LiteralPath $installDir, $dataDir) +
        @(Get-ChildItem -LiteralPath $installDir, $dataDir -File)
    foreach ($item in $targets) {
        $acl = Get-Acl -LiteralPath $item.FullName
        $ownerSid = (New-Object Security.Principal.NTAccount($acl.Owner)).Translate(
            [Security.Principal.SecurityIdentifier]).Value
        if ($ownerSid -notin $trusted) {
            Write-Warning "$($item.FullName): owner is $($acl.Owner)"
            $problems++
        }
        $isSecret = $item.Extension -in '.conf', '.ps1'
        foreach ($rule in $acl.Access) {
            if ($rule.AccessControlType -ne 'Allow') { continue }
            $sid = $rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
            if ($sid -in $trusted) { continue }
            if ($isSecret -or (([int]$rule.FileSystemRights) -band $writeMask)) {
                Write-Warning "$($item.FullName): $($rule.IdentityReference) has $($rule.FileSystemRights)"
                $problems++
            }
        }
    }
    if ($problems -gt 0) { throw "$problems ACL problem(s) left - see warnings above." }
    Write-Host 'OK: nothing the elevated tasks run is writable (or, for keys, readable) by non-admins.'
}
catch {
    Write-Host "FAILED: $_"
    Stop-Transcript | Out-Null
    exit 1
}
Stop-Transcript | Out-Null
exit 0
