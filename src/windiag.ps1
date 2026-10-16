# Collector for `route-agent diagnose` (src/windiag.rs embeds this file).
# Read-only. Prints ONE line of JSON. Rules for editing it:
#   - no double quotes anywhere (it's passed via -Command; single quotes only)
#   - ASCII only
#   - read only locale-independent properties (numbers, GUIDs, raw
#     registry values) - never parse localized text (see CLAUDE.md, sc.exe)
#   - every probe tolerates failure (missing data -> null, not an error)
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$r = [ordered]@{}

$os = Get-CimInstance Win32_OperatingSystem
if ($os) {
    $r.os = [ordered]@{
        caption      = [string]$os.Caption
        build        = [string]$os.BuildNumber
        uptime_hours = [math]::Round(((Get-Date) - $os.LastBootUpTime).TotalHours, 1)
    }
    $r.memory = [ordered]@{
        total_mb = [math]::Round($os.TotalVisibleMemorySize / 1KB)
        free_mb  = [math]::Round($os.FreePhysicalMemory / 1KB)
    }
}

$route = Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric, ifMetric | Select-Object -First 1
if ($route) {
    $a = Get-NetAdapter -InterfaceIndex $route.ifIndex
    $ipif = Get-NetIPInterface -InterfaceIndex $route.ifIndex -AddressFamily IPv4
    $rss = Get-NetAdapterRss -Name $a.Name
    $sig = $null
    # NdisPhysicalMedium 9 = native 802.11 (Wi-Fi), 14 = 802.3 (Ethernet).
    if ([int]$a.NdisPhysicalMedium -eq 9) {
        $w = (netsh wlan show interfaces) -join ' '
        if ($w -match '(\d{1,3})\s*%') { $sig = [int]$Matches[1] }
    }
    $r.adapter = [ordered]@{
        name            = [string]$a.Name
        description     = [string]$a.InterfaceDescription
        ndis_medium     = [int]$a.NdisPhysicalMedium
        link_speed_mbps = [math]::Round([double]$a.Speed / 1e6)
        driver_version  = [string]$a.DriverVersionString
        driver_date     = [string]$a.DriverDate
        driver_provider = [string]$a.DriverProvider
        mtu             = [int]$ipif.NlMtu
        rss_enabled     = $(if ($rss) { [bool]$rss.Enabled } else { $null })
        wifi_signal_pct = $sig
        gateway         = [string]$route.NextHop
    }
}

$c = Get-CimInstance Win32_Processor | Select-Object -First 1
if ($c) {
    $r.cpu = [ordered]@{
        name        = ([string]$c.Name).Trim()
        cores       = [int]$c.NumberOfCores
        threads     = [int]$c.NumberOfLogicalProcessors
        load_pct    = [int]$c.LoadPercentage
        max_mhz     = [int]$c.MaxClockSpeed
        current_mhz = [int]$c.CurrentClockSpeed
    }
}

# Win32_VideoController.AdapterRAM is a uint32 (caps at 4 GB); the driver's
# own registry value has the real size.
$vramByDesc = @{}
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}' |
    ForEach-Object {
        $p = Get-ItemProperty $_.PSPath
        if ($p.DriverDesc -and $p.'HardwareInformation.qwMemorySize') {
            $vramByDesc[[string]$p.DriverDesc] = [math]::Round([double]$p.'HardwareInformation.qwMemorySize' / 1MB)
        }
    }
$r.gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object {
    [ordered]@{
        name           = [string]$_.Name
        driver_version = [string]$_.DriverVersion
        driver_date    = $(if ($_.DriverDate) { $_.DriverDate.ToString('yyyy-MM-dd') } else { $null })
        vram_mb        = $vramByDesc[[string]$_.Name]
        refresh_hz     = [int]$_.CurrentRefreshRate
        active         = [bool]$_.CurrentHorizontalResolution
    }
})

$s = (Get-Counter '\GPU Process Memory(*)\Dedicated Usage').CounterSamples
if ($s) {
    $r.vram_used_mb = [math]::Round(($s | Measure-Object CookedValue -Sum).Sum / 1MB)
    $r.top_vram = @($s | Where-Object { $_.InstanceName -match 'pid_(\d+)' } |
        ForEach-Object { [void]($_.InstanceName -match 'pid_(\d+)'); [pscustomobject]@{ id = [int]$Matches[1]; mb = $_.CookedValue / 1MB } } |
        Group-Object id | ForEach-Object {
            $p = Get-Process -Id ([int]$_.Name)
            [pscustomobject]@{ name = [string]$p.ProcessName; count = 1; mb = [math]::Round(($_.Group | Measure-Object mb -Sum).Sum) }
        } | Where-Object { $_.mb -gt 0 } | Sort-Object mb -Descending | Select-Object -First 5)
}

$r.top_ram = @(Get-Process | Group-Object ProcessName | ForEach-Object {
    [pscustomobject]@{ name = $_.Name; count = $_.Count; mb = [math]::Round(($_.Group | Measure-Object WorkingSet64 -Sum).Sum / 1MB) }
} | Sort-Object mb -Descending | Select-Object -First 8)

# Usually needs admin and/or isn't implemented by the motherboard - empty
# is the common, expected result.
$r.temps_c = @(Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature |
    ForEach-Object { [math]::Round($_.CurrentTemperature / 10 - 273.15, 1) })

$gc = Get-ItemProperty 'HKCU:\System\GameConfigStore'
$gb = Get-ItemProperty 'HKCU:\Software\Microsoft\GameBar'
$gd = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\GameDVR'
$gr = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers'
$plan = $null
if (((powercfg /getactivescheme) -join ' ') -match '([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})') {
    $plan = $Matches[1].ToLower()
}
$r.game = [ordered]@{
    game_dvr       = $gc.GameDVR_Enabled
    app_capture    = $gd.AppCaptureEnabled
    auto_game_mode = $gb.AutoGameModeEnabled
    hags           = $gr.HwSchMode
    power_plan     = $plan
}

$skip = 'PSPath', 'PSParentPath', 'PSChildName', 'PSDrive', 'PSProvider'
$enabled = @()
foreach ($hive in 'HKCU', 'HKLM') {
    $run = Get-ItemProperty ($hive + ':\Software\Microsoft\Windows\CurrentVersion\Run')
    $sa = Get-ItemProperty ($hive + ':\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run')
    if (-not $run) { continue }
    foreach ($prop in $run.PSObject.Properties) {
        if ($prop.Name -in $skip) { continue }
        $v = if ($sa) { $sa.($prop.Name) } else { $null }
        # StartupApproved: odd first byte = disabled (as Task Manager sets it).
        if ($v -and ($v[0] -band 1)) { continue }
        $enabled += [string]$prop.Name
    }
}
$r.startup_enabled = @($enabled)

$d = Get-CimInstance Win32_LogicalDisk -Filter ('DeviceID=''' + $env:SystemDrive + '''')
if ($d) {
    $r.disk = [ordered]@{
        drive    = $env:SystemDrive
        free_gb  = [math]::Round($d.FreeSpace / 1GB, 1)
        total_gb = [math]::Round($d.Size / 1GB, 1)
    }
}

$r | ConvertTo-Json -Depth 5 -Compress
