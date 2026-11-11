<#
.SYNOPSIS
  Measures RustCord's memory the way Task Manager shows it, plus frame time.

.DESCRIPTION
  Starts the given rustcord.exe and samples it at 10 s, 30 s and 120 s, then optionally once more after you browse around
  (press Enter when done). Each sample records:
    - Memory  : private working set (Task Manager's "Memory" column)
    - WS      : working set
    - Private : private bytes (commit)
  and the frame-time p95 (ms) of the frames since the previous mark. Frame
  times come from the app itself (RUSTCORD_FRAMETIMES=<file>, set here).
  (-Profile is an alias of -ModuleProfile; $Profile is a PowerShell
  automatic variable.)

  One markdown table row per run is appended to scripts/memprobe-results.md.

  Demo runs (-Demo, -Big) never touch your real config or login: with
  RUSTCORD_MODULES set the app starts from defaults and saves nothing.
  Live runs (-Live <user id>, or -Live bot for the saved bot account named
  "RustCord") set RUSTCORD_TEST_ACCOUNT: the app takes that saved account's
  token from the OS credential store itself, under its own storage id
  ("RustCord-Test"), never saves or removes accounts and is read-only
  (no sends, acks or typing). This script never reads, passes or logs a
  token; -Live bot only reads account ids from the config. -Profile is
  ignored in live runs (Modules start from the defaults there).

  Every launch sets RUSTCORD_START_OFFSCREEN=1: the window opens far off
  screen without taking focus (only its taskbar button shows), so it never
  pops up over other work but still draws and loads like a normal window.
  (A minimised window doesn't run its UI in eframe, so its numbers would
  be far too low.) -Visible opens it normally instead, to compare.

  Windows PowerShell 5.1 compatible. Nothing here trims the working set.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\memprobe.ps1 -Exe target\release\rustcord.exe -Demo -Big -Profile allon -Label baseline

.EXAMPLE
  scripts\memprobe.ps1 -Exe target\release\rustcord.exe -Label "real account" -Browse
#>
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    # RUSTCORD_DEMO=1 (offline fake account).
    [switch]$Demo,
    # RUSTCORD_DEMO_BIG=1 (~100 servers, ~3000 channels, ~200 DMs). Implies -Demo.
    [switch]$Big,
    # Open the window normally (on screen) instead of off screen.
    [switch]$Visible,
    # Real account, read-only: a saved account's user id, or "bot".
    [string]$Live = '',
    # leo = Leo's module list (-LeoModules), allon = every module on.
    # (Named ModuleProfile because $Profile is a PowerShell automatic variable.)
    [Alias('Profile')][ValidateSet('leo', 'allon')][string]$ModuleProfile = 'leo',
    # RUSTCORD_MODULES spec for -Profile leo: "all", "none", "default" or a
    # list like "none,images,member_list" / "all,-embeds". Until Leo sends
    # his list this is "all".
    [string]$LeoModules = 'all',
    # After the last mark, wait for Enter while you browse, then sample again.
    [switch]$Browse,
    # Short name for this run in the table (e.g. "baseline", "lto fat").
    [string]$Label = '',
    # Extra environment variables for the app, e.g. @{ RUSTCORD_DEMO_LAYOUT = 'alt' }.
    [hashtable]$Env = @{},
    # Where to append the result row (default: memprobe-results.md beside
    # this script).
    [string]$Results = ''
)

$ErrorActionPreference = 'Stop'
# ($PSScriptRoot is empty in param defaults on PowerShell 5.1.)
if ($Results -eq '') {
    $Results = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) 'memprobe-results.md'
}
# Seconds after start to sample (the table has one column for each).
$Marks = @(10, 30, 120)
if ($Big) { $Demo = $true }

$exePath = (Resolve-Path $Exe).Path
$frameFile = Join-Path ([System.IO.Path]::GetTempPath()) ("rustcord-frames-{0}.csv" -f ([guid]::NewGuid().ToString('N')))

# --- start the app with its own environment (nothing leaks into this shell)
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $exePath
$psi.WorkingDirectory = Split-Path $exePath
$psi.UseShellExecute = $false
foreach ($k in @('RUSTCORD_DEMO', 'RUSTCORD_DEMO_BIG', 'RUSTCORD_MODULES', 'RUSTCORD_FRAMETIMES', 'RUSTCORD_TEST_ACCOUNT', 'RUSTCORD_START_MINIMIZED', 'RUSTCORD_START_OFFSCREEN', 'DISCORD_TOKEN')) {
    if ($psi.EnvironmentVariables.ContainsKey($k)) { $psi.EnvironmentVariables.Remove($k) }
}
$psi.EnvironmentVariables['RUSTCORD_FRAMETIMES'] = $frameFile
if (-not $Visible) { $psi.EnvironmentVariables['RUSTCORD_START_OFFSCREEN'] = '1' }
if ($Live -ne '' -and $Demo) { throw '-Live and -Demo/-Big are separate runs' }
if ($Live -eq 'bot') {
    # The saved bot account named "RustCord" in the real config. Only ids
    # and flags are matched; nothing else from the file is kept or shown.
    $cfg = Join-Path $env:APPDATA 'RustCord\data\app.ron'
    $text = [System.IO.File]::ReadAllText($cfg)
    $ids = [regex]::Matches($text, 'user_id\\*"?\s*:\s*\\*"(\d{15,21})')
    $Live = ''
    for ($i = 0; $i -lt $ids.Count; $i++) {
        $start = $ids[$i].Index
        if ($i + 1 -lt $ids.Count) { $end = $ids[$i + 1].Index } else { $end = [Math]::Min($text.Length, $start + 600) }
        $chunk = $text.Substring($start, $end - $start)
        if (($chunk -match 'is_bot\\*"?\s*:\s*true') -and ($chunk -match '(username|display_name)\\*"?\s*:\s*\\*"RustCord\\*"')) {
            $Live = $ids[$i].Groups[1].Value
            break
        }
    }
    $text = $null
    if ($Live -eq '') { throw 'No saved bot account named RustCord found' }
    Write-Host "memprobe: bot account id $Live"
}
if ($Live -ne '') {
    if ($Live -notmatch '^\d{15,21}$') { throw '-Live takes a user id (digits) or "bot"' }
    $psi.EnvironmentVariables['RUSTCORD_TEST_ACCOUNT'] = $Live
}
$modules = '(own settings)'
if ($Demo) {
    # Demo runs never need a token; don't hand one down.
    if ($psi.EnvironmentVariables.ContainsKey('DISCORD_TOKEN')) { $psi.EnvironmentVariables.Remove('DISCORD_TOKEN') }
    $psi.EnvironmentVariables['RUSTCORD_DEMO'] = '1'
    if ($Big) { $psi.EnvironmentVariables['RUSTCORD_DEMO_BIG'] = '1' }
    if ($ModuleProfile -eq 'allon') { $modules = 'all' } else { $modules = $LeoModules }
    $psi.EnvironmentVariables['RUSTCORD_MODULES'] = $modules
}
foreach ($k in $Env.Keys) { $psi.EnvironmentVariables[[string]$k] = [string]$Env[$k] }

$mode = 'real'
if ($Live -ne '') { $mode = "live $Live" }
if ($Big) { $mode = 'demo-big' } elseif ($Demo) { $mode = 'demo' }
if ($Visible) { $mode = "$mode (visible)" }

function Get-Mem([int]$procId) {
    # WMI class names are not localised (Get-Counter paths are).
    $perf = Get-CimInstance -ClassName Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess = $procId" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
    if ($null -eq $p) { return $null }
    $p.Refresh()
    $private = $null
    if ($null -ne $perf) { $private = [double]$perf.WorkingSetPrivate }
    if ($null -eq $private) {
        try {
            $c = Get-Counter ("\Process({0})\Working Set - Private" -f $p.ProcessName) -ErrorAction Stop
            $private = [double]$c.CounterSamples[0].CookedValue
        } catch { $private = [double]::NaN }
    }
    [pscustomobject]@{
        Memory  = $private / 1MB
        WS      = $p.WorkingSet64 / 1MB
        Private = $p.PrivateMemorySize64 / 1MB
    }
}

function Get-P95([double]$fromMs, [double]$toMs) {
    if (-not (Test-Path $frameFile)) { return $null }
    $vals = New-Object System.Collections.Generic.List[double]
    foreach ($line in [System.IO.File]::ReadAllLines($frameFile)) {
        $parts = $line.Split(',')
        if ($parts.Count -ne 2) { continue }
        $at = [double]$parts[0]
        if ($at -gt $fromMs -and $at -le $toMs) {
            $vals.Add([double]::Parse($parts[1], [System.Globalization.CultureInfo]::InvariantCulture))
        }
    }
    if ($vals.Count -eq 0) { return $null }
    $sorted = $vals.ToArray()
    [array]::Sort($sorted)
    $rank = [math]::Ceiling($sorted.Length * 0.95)
    if ($rank -lt 1) { $rank = 1 }
    return [pscustomobject]@{ P95 = $sorted[$rank - 1]; Frames = $sorted.Length }
}

function Now-Ms { [double]([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()) }

function Fmt-Mem($m) {
    if ($null -eq $m) { return 'exited' }
    return ('{0:N1} / {1:N1} / {2:N1}' -f $m.Memory, $m.WS, $m.Private)
}

function Fmt-P95($f) {
    if ($null -eq $f) { return '-' }
    return ('{0:N1}' -f $f.P95)
}

Write-Host "memprobe: $mode, modules=$modules, $exePath"
$proc = [System.Diagnostics.Process]::Start($psi)
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$lastMs = Now-Ms
$cells = @()
$p95s = @()

try {
    foreach ($mark in $Marks) {
        $wait = $mark * 1000 - $sw.ElapsedMilliseconds
        if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
        if ($proc.HasExited) { throw "RustCord exited early (code $($proc.ExitCode))" }
        # Let the app flush its frame log (every 2 s) before reading it.
        $m = Get-Mem $proc.Id
        $now = Now-Ms
        Start-Sleep -Milliseconds 2200
        $f = Get-P95 $lastMs $now
        $lastMs = $now
        $cells += (Fmt-Mem $m)
        $p95s += (Fmt-P95 $f)
        Write-Host ("{0,4} s  Memory {1}  p95 {2} ms" -f $mark, (Fmt-Mem $m), (Fmt-P95 $f))
    }
    if ($Browse) {
        Read-Host 'Browse now (switch ~20 channels, scroll), then press Enter to sample'
        $m = Get-Mem $proc.Id
        $now = Now-Ms
        Start-Sleep -Milliseconds 2200
        $f = Get-P95 $lastMs $now
        $cells += (Fmt-Mem $m)
        $p95s += (Fmt-P95 $f)
        Write-Host ("browsed  Memory {0}  p95 {1} ms" -f (Fmt-Mem $m), (Fmt-P95 $f))
    } else {
        $cells += '-'
    }
}
finally {
    if (-not $proc.HasExited) {
        [void]$proc.CloseMainWindow()
        if (-not $proc.WaitForExit(5000)) { $proc.Kill() }
    }
    Remove-Item $frameFile -ErrorAction SilentlyContinue
}

# --- append the row
if (-not (Test-Path $Results)) {
    @(
        '# memprobe results',
        '',
        'Cells: Memory (private working set, what Task Manager shows) / working set / private bytes, in MB.',
        'p95: frame CPU time in ms, 95th percentile of the frames between marks ("-" = no frames drawn).',
        '',
        '| Date | Label | Mode | Modules | 10 s | 30 s | 120 s | Browsed | p95 (ms) |',
        '|---|---|---|---|---|---|---|---|---|'
    ) | Set-Content -Path $Results -Encoding utf8
}
$row = '| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} |' -f `
    (Get-Date -Format 'yyyy-MM-dd HH:mm'), $Label, $mode, $modules,
    $cells[0], $cells[1], $cells[2], $cells[3], ($p95s -join ' / ')
Add-Content -Path $Results -Value $row -Encoding utf8
Write-Host "appended to $Results"
