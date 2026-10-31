<#
.SYNOPSIS
Builds Sidecast for Windows from this repository and installs it for the
current user (no administrator rights needed).

.DESCRIPTION
Checks Rust and the Microsoft C++ build tools, builds the release binaries,
installs them into %LOCALAPPDATA%\Programs\Sidecast (quitting a running copy
first), adds a Start-menu shortcut, optionally starts Sidecast at login, and
says whether the Virtual Display Driver (for an extra screen) and a virtual
audio cable (for the phone's microphone) are installed.

.PARAMETER InstallDir
Where to install (default: %LOCALAPPDATA%\Programs\Sidecast).

.PARAMETER Autostart
Start Sidecast when you log in (also in its menu: Start at login).

.PARAMETER NoLaunch
Do not start Sidecast afterwards.

.PARAMETER NoShortcut
Do not create the Start-menu shortcut.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1 -Autostart
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Sidecast'),
    [switch]$Autostart,
    [switch]$NoLaunch,
    [switch]$NoShortcut
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$RepoDir = Split-Path -Parent $PSScriptRoot
$VddReleases = 'https://github.com/VirtualDrivers/Virtual-Display-Driver/releases'
$CableUrl = 'https://vb-audio.com/Cable/'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

function Step([string]$Text) { Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Info([string]$Text) { Write-Host "    $Text" }
function Fail([string]$Text) {
    Write-Host "error: $Text" -ForegroundColor Red
    exit 1
}

Step 'Checking the build tools'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Fail ("Rust is not installed. Install it with:`n" +
        "      winget install Rustlang.Rustup`n" +
        "    (or from https://rustup.rs), then open a new terminal and run this again.")
}
$rustInfo = & rustc -vV
$hostTriple = ($rustInfo | Select-String '^host:').ToString().Split(' ')[1]
Info ("Rust: " + ($rustInfo | Select-Object -First 1))
if ($hostTriple -notlike '*-pc-windows-msvc') {
    Fail "Rust's default toolchain targets $hostTriple; Sidecast needs the MSVC one:  rustup default stable-msvc"
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = $null
if (Test-Path $vswhere) {
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if (-not $vs) {
    Fail ("The Microsoft C++ build tools are missing (Rust links with them, and they compile OpenH264 and ring).`n" +
        "    Install them with:`n" +
        "      winget install Microsoft.VisualStudio.2022.BuildTools --override `"--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended`"`n" +
        "    or pick 'Desktop development with C++' in the Visual Studio Installer, then run this again.")
}
Info "C++ build tools: $vs"

Step 'Building Sidecast (release; the first build takes a few minutes)'
Push-Location (Join-Path $RepoDir 'rust')
try {
    & cargo build --release -p sidecast-host
    if ($LASTEXITCODE -ne 0) { Fail 'the build failed (see above)' }
} finally {
    Pop-Location
}
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoDir 'rust\target' }
$built = Join-Path $targetDir 'release'

Step "Installing into $InstallDir"
if (Get-Process -Name 'sidecast-tray' -ErrorAction SilentlyContinue) {
    # A normal quit: the running copy says goodbye to a connected phone and frees its files.
    Info 'Quitting the running Sidecast'
    & (Join-Path $built 'sidecast-tray.exe') --quit
    for ($i = 0; $i -lt 50 -and (Get-Process -Name 'sidecast-tray' -ErrorAction SilentlyContinue); $i++) {
        Start-Sleep -Milliseconds 200
    }
    if (Get-Process -Name 'sidecast-tray' -ErrorAction SilentlyContinue) {
        Fail 'Sidecast is still running: quit it from its icon in the notification area, then run this again.'
    }
}
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
foreach ($file in 'sidecast-tray.exe', 'sidecast-host.exe') {
    Copy-Item -Force (Join-Path $built $file) $InstallDir
}
foreach ($file in 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md') {
    Copy-Item -Force (Join-Path $RepoDir $file) $InstallDir
}
$exe = Join-Path $InstallDir 'sidecast-tray.exe'
Info "Installed $exe"

if (-not $NoShortcut) {
    $link = Join-Path ([Environment]::GetFolderPath('Programs')) 'Sidecast.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($link)
    $shortcut.TargetPath = $exe
    $shortcut.WorkingDirectory = $InstallDir
    $shortcut.Description = 'Use your phone as a second screen, touchpad, keyboard and microphone'
    $shortcut.Save()
    Info "Start menu: Sidecast ($link)"
}

$existing = Get-ItemProperty -Path $RunKey -Name 'Sidecast' -ErrorAction SilentlyContinue
if ($Autostart -or $existing) {
    # Also refreshes an older entry, which may point to another folder.
    Set-ItemProperty -Path $RunKey -Name 'Sidecast' -Value "`"$exe`""
    Info 'Starts at login (turn it off in its menu, or in Settings > Apps > Startup)'
}

Step 'Virtual Display Driver (an extra screen for the phone)'
$pipes = @()
try { $pipes = [System.IO.Directory]::GetFiles('\\.\pipe\') } catch { }
$vddPipe = [bool]($pipes | Where-Object { $_ -like '*MTTVirtualDisplayPipe' })
$vddDevice = Get-PnpDevice -Class Display -ErrorAction SilentlyContinue |
    Where-Object { $_.FriendlyName -like '*Virtual Display Driver*' -or ($_.HardwareID -join ' ') -like '*MttVDD*' }
if ($vddPipe) {
    Info 'Installed and running: each phone gets its own extra screen at its resolution.'
} elseif ($vddDevice) {
    Info 'Installed but not running (disabled in Device Manager?). Enable it, or phones mirror an existing monitor.'
} else {
    Info 'Not installed: phones mirror your main monitor (or extend onto a second one).'
    Info 'For an extra screen, install the signed driver from its official release:'
    Info "  $VddReleases"
    Info "(its 'Virtual Driver Control' app installs it), then connect the phone again."
}

Step 'Phone microphone (optional)'
$cable = Get-PnpDevice -Class AudioEndpoint -ErrorAction SilentlyContinue |
    Where-Object { $_.FriendlyName -like 'CABLE Input*' -and $_.Status -eq 'OK' }
if ($cable) {
    Info "Virtual audio cable found: apps can record the phone's microphone as 'CABLE Output'."
} else {
    Info "To use the phone's microphone in Windows apps, install a virtual audio cable such as VB-CABLE"
    Info "($CableUrl). Sidecast plays the phone into 'CABLE Input'; apps record 'CABLE Output'."
}

Step 'Windows Firewall'
Info 'The first time Sidecast runs, Windows Defender Firewall asks whether it may accept connections:'
Info 'allow Private networks (your home Wi-Fi) so phones can connect; leave Public networks unchecked.'
Info 'Your Wi-Fi must be a Private network (Settings > Network & internet > Wi-Fi > your network).'
Info 'Missed it? Windows Security > Firewall & network protection > Allow an app through firewall.'

if (-not $NoLaunch) {
    Start-Process -FilePath $exe
    Step 'Sidecast is running: its icon is in the notification area (click ^ if it is hidden).'
    Info 'On the phone, pick this computer in the list, or Add by IP (the address is in the menu).'
}
