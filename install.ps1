# RustCraft installer for Windows.
# Detects the architecture and offers install, update and uninstall actions.
# Messages are ASCII only so Windows PowerShell 5.1 renders them correctly.
param(
    [Parameter(Position = 0)][string]$Action = "",
    [switch]$Purge,
    [switch]$Yes
)

$ErrorActionPreference = "Stop"

$Repo = "jephersonRD/RustCraft"
$Bin = "rustcraft.exe"
$Version = if ($env:RUSTCRAFT_VERSION) { $env:RUSTCRAFT_VERSION } else { "latest" }
$LocalRoot = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $env:USERPROFILE "AppData\Local" }
$DataRoot = if ($env:APPDATA) { $env:APPDATA } else { Join-Path $env:USERPROFILE "AppData\Roaming" }
$RootDir = Join-Path $LocalRoot "RustCraft"
$InstallDir = Join-Path $RootDir "bin"
$DataDir = Join-Path $DataRoot "RustCraft"
$ExePath = Join-Path $InstallDir $Bin

$Arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if (-not $Arch) { $Arch = "AMD64" }

function Write-Say($Message) {
    Write-Host "RustCraft " -NoNewline -ForegroundColor Yellow
    Write-Host $Message
}

function Write-Ok($Message) {
    Write-Host "OK " -NoNewline -ForegroundColor Green
    Write-Host $Message
}

function Write-Warn($Message) {
    Write-Host "! " -NoNewline -ForegroundColor Yellow
    Write-Host $Message
}

function Fail($Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Show-Usage {
    Write-Host "RustCraft installer"
    Write-Host ""
    Write-Host "Usage:"
    Write-Host "  .\install.ps1 [install|update|uninstall] [options]"
    Write-Host ""
    Write-Host "Actions:"
    Write-Host "  install       Install the latest release (or RUSTCRAFT_VERSION)."
    Write-Host "  update        Download and install the latest release."
    Write-Host "  uninstall     Remove the binary. Asks before deleting data."
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  -Purge        On uninstall, also delete data and instances."
    Write-Host "  -Yes          Do not prompt (data is only deleted with -Purge)."
    Write-Host "  -Help         Show this help."
    Write-Host ""
    Write-Host "Environment:"
    Write-Host "  RUSTCRAFT_VERSION   Release tag, for example v0.1.0. Default: latest."
    Write-Host ""
    Write-Host "Install location: $ExePath"
    Write-Host "Data location:    $DataDir"
}

function Get-AssetName {
    switch ($Arch.ToUpperInvariant()) {
        "ARM64" { return "rustcraft-windows-arm64.exe" }
        "X86" { return "rustcraft-windows-x86.exe" }
        default { return "rustcraft-windows-x64.exe" }
    }
}

function Get-DownloadUrl($Asset) {
    if ($Version -eq "latest") {
        return "https://github.com/$Repo/releases/latest/download/$Asset"
    }
    return "https://github.com/$Repo/releases/download/$Version/$Asset"
}

function Invoke-Download($Url, $Dest) {
    try {
        Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing
        return $true
    }
    catch {
        return $false
    }
}

function Build-FromSource {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Fail "No prebuilt binary for Windows/$Arch and Cargo is missing. Install Rust from https://rustup.rs"
    }
    Write-Say "building from source (this may take a few minutes)..."
    & cargo install --git "https://github.com/$Repo" --locked --root $RootDir --force
    if ($LASTEXITCODE -ne 0) {
        Fail "Cargo could not build RustCraft."
    }
}

function Add-ToPath {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $userPath) { $userPath = "" }
    $parts = @($userPath -split ";" | Where-Object { $_ -ne "" })
    if ($parts -notcontains $InstallDir) {
        $newPath = (@($parts) + $InstallDir) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        Write-Warn "Added $InstallDir to PATH; open a new terminal to use it."
    }
    $env:Path = "$InstallDir;$env:Path"
}

function Remove-FromPath {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $userPath) { return }
    $parts = @($userPath -split ";" | Where-Object { $_ -ne "" })
    $kept = @($parts | Where-Object { $_ -ne $InstallDir })
    if ($kept.Count -ne $parts.Count) {
        [Environment]::SetEnvironmentVariable("Path", ($kept -join ";"), "User")
    }
}

function Test-Java {
    try {
        $line = (& java -version 2>&1 | Select-Object -First 1)
        if ($line -match '"(\d+)') {
            $major = [int]$Matches[1]
            if ($major -ge 25) {
                Write-Ok "Java $major detected"
                return
            }
        }
    }
    catch { }
    Write-Warn "Java 25 is not installed (horizon-prepare needs it)."
    Write-Host "  Install it with: winget install --id EclipseAdoptium.Temurin.25.JRE -e"
}

function Install-App {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $asset = Get-AssetName
    $url = Get-DownloadUrl $asset
    $tmp = Join-Path $env:TEMP ("rustcraft-" + [guid]::NewGuid().ToString() + ".exe")

    $downloaded = Invoke-Download $url $tmp
    if ($downloaded) {
        Move-Item -Force $tmp $ExePath
    }
    else {
        if (Test-Path $tmp) { Remove-Item $tmp -Force }
        Write-Warn "No prebuilt binary for Windows/$Arch."
        Build-FromSource
    }

    Add-ToPath
    Write-Ok "RustCraft installed at $ExePath"
    Test-Java
    Write-Host ""
    Write-Host "Run: rustcraft"
}

function Uninstall-App {
    if (Test-Path $ExePath) {
        Remove-Item $ExePath -Force
        Write-Ok "Removed $ExePath"
    }
    else {
        Write-Warn "RustCraft was not installed at $InstallDir"
    }

    $deleteData = $Purge.IsPresent
    if (-not $deleteData -and -not $Yes.IsPresent -and (Test-Path $DataDir)) {
        $answer = Read-Host "Delete data and instances in $DataDir too? [y/N]"
        if ($answer -match '^(y|yes|s|si)$') { $deleteData = $true }
    }

    if ($deleteData -and (Test-Path $DataDir)) {
        Remove-Item $DataDir -Recurse -Force
        Write-Ok "Deleted data at $DataDir"
    }
    else {
        Write-Host "Data kept at $DataDir"
    }

    Remove-FromPath
    Write-Ok "RustCraft uninstalled"
}

function Show-Menu {
    Write-Host "RustCraft - installer"
    Write-Host "System: Windows ($Arch)"
    Write-Host ""
    Write-Host "  1) Install"
    Write-Host "  2) Update"
    Write-Host "  3) Uninstall"
    Write-Host "  4) Exit"
    Write-Host ""
    $option = Read-Host "Choose an option [1-4]"
    switch ($option) {
        "1" { return "install" }
        "2" { return "update" }
        "3" { return "uninstall" }
        "4" { exit 0 }
        default { Fail "invalid option: $option" }
    }
}

$Action = $Action.Trim().TrimStart("-").ToLowerInvariant()
if ($Action -eq "help" -or $Action -eq "h") {
    Show-Usage
    exit 0
}
if ([string]::IsNullOrWhiteSpace($Action)) {
    $Action = Show-Menu
}

switch ($Action) {
    "install" { Install-App }
    "update" { Write-Say "looking for the latest release ($Version)..."; Install-App }
    "uninstall" { Uninstall-App }
    default { Fail "unknown action: $Action (use -Help)" }
}
