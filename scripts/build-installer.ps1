<#
.SYNOPSIS
    Builds the Windows installer, Clippy_<version>_x64-setup.exe, on top of the
    same gates as the release zip.

.DESCRIPTION
    The installer is Tauri's NSIS bundle, configured by
    crates\clippy-ui\tauri.installer.conf.json and
    crates\clippy-ui\installer-hooks.nsh. It installs per user (no admin, no UAC
    prompt) into %LOCALAPPDATA%\Clippy, which is what keeps the in-app updater
    working: the updater swaps the three binaries in place by renaming them, and
    that needs a folder the user can write to.

    What goes in it is exactly what goes in the zip: clippy.exe,
    clippy-daemon.exe, clippy-ui.exe and README.txt. This script does not build
    anything itself; it runs scripts\ship-zip.ps1 -StageOnly first, so the clean
    tree, version agreement, both test suites and the version checks on the
    staged binaries all apply, and then bundles the staged files.

    After bundling it installs the result silently into a scratch folder, checks
    what landed, runs clippy.exe from there and uninstalls again, so the artifact
    is one that was seen to install and to leave nothing behind. That step is
    skipped, with a notice, when a real Clippy is installed or running: the
    uninstall entry has one fixed name per user, and the installer stops every
    clippy process, so a test must not run over somebody's real installation.

    Requires everything ship-zip.ps1 does (cargo tauri, npm), plus network on the
    first run, when the tauri CLI downloads and hash-checks NSIS.

.PARAMETER OutDir
    Where the installer and its .sha256 are written. Defaults to dist\ inside
    the repo (git-ignored), like the zip.

.PARAMETER SkipTests
    Passed to ship-zip.ps1. The installer is then named with -untested.

.PARAMETER AllowDirty
    Passed to ship-zip.ps1. The installer is then named with -dirty.

.PARAMETER SkipInstallTest
    Skips the silent install/uninstall round trip.

.EXAMPLE
    pwsh -ExecutionPolicy Bypass -File scripts\build-installer.ps1

    Run it under PowerShell 7 (pwsh) when launched from a shell that carries
    pwsh's PSModulePath: Windows PowerShell 5.1 then cannot find Get-FileHash.
#>
[CmdletBinding()]
param(
    [string]$OutDir,
    [switch]$SkipTests,
    [switch]$AllowDirty,
    [switch]$SkipInstallTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'dist' }
$UiDir = Join-Path $RepoRoot 'crates\clippy-ui'
$StageForBundle = Join-Path $RepoRoot 'target\installer-stage'
$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Clippy'
# Where the NSIS installer remembers the folder it installed to (InstallDirRegKey).
# A silent uninstall leaves it behind, and the next install by hand then
# defaults to that folder -- which, after the round trip below, was a Temp
# folder about to be deleted. Seen on 2026-11-01.
$InstallDirKey = 'HKCU:\Software\Clippy'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

# Removes whatever the install round trip left that points into $Under: the
# remembered install folder, the uninstall entry and the shortcuts. Never
# touches an entry that points anywhere else (a real install).
function Clear-TestInstall([string]$Under) {
    $prefix = $Under.TrimEnd('\') + '\'
    $remembered = (Get-ItemProperty -LiteralPath "$InstallDirKey\Clippy" -ErrorAction SilentlyContinue).'(default)'
    if ($remembered -and ($remembered + '\').StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath "$InstallDirKey\Clippy" -Recurse -Force -ErrorAction SilentlyContinue
        if ((Test-Path $InstallDirKey) -and -not (Get-ChildItem $InstallDirKey -ErrorAction SilentlyContinue) -and
            -not (Get-Item $InstallDirKey).Property) {
            Remove-Item -LiteralPath $InstallDirKey -Force -ErrorAction SilentlyContinue
        }
    }
    $location = (Get-ItemProperty -LiteralPath $UninstallKey -ErrorAction SilentlyContinue).InstallLocation
    if ($location -and ($location.Trim('"') + '\').StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $UninstallKey -Recurse -Force -ErrorAction SilentlyContinue
    }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($dir in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {
        $lnk = Join-Path $dir 'Clippy.lnk'
        if ((Test-Path -LiteralPath $lnk) -and
            $shell.CreateShortcut($lnk).TargetPath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $lnk -Force -ErrorAction SilentlyContinue
        }
    }
}

function Require {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    if ($Condition) {
        Write-Host "  [PASS] $Name" -ForegroundColor Green
    } else {
        Write-Host "  [FAIL] $Name" -ForegroundColor Red
        if ($Detail) { Write-Host "         $Detail" -ForegroundColor Red }
        throw "check failed: $Name"
    }
}

$work = Join-Path $env:TEMP ("clippy-installer-" + [guid]::NewGuid().ToString('N'))
$installStarted = $false
# The uninstaller deletes the Run\Clippy value (installer-hooks.nsh), even one a
# real install wrote; it is put back after the round trip.
$savedRun = (Get-ItemProperty -LiteralPath $RunKey -Name 'Clippy' -ErrorAction SilentlyContinue).Clippy
try {
    Write-Host ''
    Write-Host 'Step 1: gates, release build and staging (scripts\ship-zip.ps1 -StageOnly)' -ForegroundColor Cyan
    $shipArgs = @('-StageOnly', '-StageDir', $work)
    if ($SkipTests) { $shipArgs += '-SkipTests' }
    if ($AllowDirty) { $shipArgs += '-AllowDirty' }
    # The same PowerShell host this script runs in, so the child sees the same modules.
    $self = (Get-Process -Id $PID).Path
    & $self -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'ship-zip.ps1') @shipArgs
    Require -Name 'ship-zip.ps1 -StageOnly' -Condition ($LASTEXITCODE -eq 0) -Detail "exit code $LASTEXITCODE"

    $manifest = Get-Content -LiteralPath (Join-Path $work 'release.json') -Raw | ConvertFrom-Json
    $Version = $manifest.version
    $stage = Join-Path $work $manifest.name
    $suffix = ''
    if ($manifest.skippedTests) { $suffix += '-untested' }
    if ($manifest.name -like '*-dirty') { $suffix += '-dirty' }

    Write-Host ''
    Write-Host 'Step 2: bundle' -ForegroundColor Cyan
    # tauri.installer.conf.json lists these paths; the bundler packs them beside
    # clippy-ui.exe under the same names, which is the layout the updater and
    # the UI's "daemon is beside me" lookup both rely on.
    if (Test-Path -LiteralPath $StageForBundle) { Remove-Item -LiteralPath $StageForBundle -Recurse -Force }
    New-Item -ItemType Directory -Force $StageForBundle | Out-Null
    foreach ($f in @('clippy.exe', 'clippy-daemon.exe', 'README.txt')) {
        Copy-Item -LiteralPath (Join-Path $stage $f) -Destination (Join-Path $StageForBundle $f) -Force
    }

    Push-Location $UiDir
    try { cargo tauri build --bundles nsis --config tauri.installer.conf.json } finally { Pop-Location }
    Require -Name 'cargo tauri build --bundles nsis' -Condition ($LASTEXITCODE -eq 0) -Detail "exit code $LASTEXITCODE"

    $built = Join-Path $RepoRoot "target\release\bundle\nsis\Clippy_${Version}_x64-setup.exe"
    Require -Name 'the installer exists' -Condition (Test-Path -LiteralPath $built) -Detail $built
    $info = (Get-Item -LiteralPath $built).VersionInfo
    Require -Name "the installer reports $Version" -Condition ($info.ProductVersion -eq $Version) `
        -Detail "ProductVersion is '$($info.ProductVersion)'"
    $uiInfo = (Get-Item -LiteralPath (Join-Path $RepoRoot 'target\release\clippy-ui.exe')).VersionInfo
    Require -Name "the bundled clippy-ui.exe reports $Version" -Condition ($uiInfo.ProductVersion -eq $Version) `
        -Detail "ProductVersion is '$($uiInfo.ProductVersion)'"

    Write-Host ''
    Write-Host 'Step 3: install, run, uninstall' -ForegroundColor Cyan
    $installTested = $false
    if ($SkipInstallTest) {
        Write-Host '  skipped (-SkipInstallTest)' -ForegroundColor Yellow
    } elseif ((Test-Path $UninstallKey) -or (Test-Path "$InstallDirKey\Clippy") -or
              (Get-Process -Name 'clippy*' -ErrorAction SilentlyContinue)) {
        Write-Host '  skipped: Clippy is installed or running here, and the test would stop it and' -ForegroundColor Yellow
        Write-Host '  reuse its uninstall entry. Run this on a machine without it, or by hand.' -ForegroundColor Yellow
    } else {
        $target = Join-Path $work 'installed'
        $installStarted = $true
        $p = Start-Process -FilePath $built -ArgumentList '/S', "/D=$target" -Wait -PassThru
        Require -Name 'silent install exits 0' -Condition ($p.ExitCode -eq 0) -Detail "exit code $($p.ExitCode)"
        foreach ($f in @('clippy.exe', 'clippy-daemon.exe', 'clippy-ui.exe', 'README.txt', 'uninstall.exe')) {
            Require -Name "installed: $f" -Condition (Test-Path -LiteralPath (Join-Path $target $f)) -Detail $target
        }
        Require -Name 'the uninstall entry is registered' -Condition (Test-Path $UninstallKey)
        $reported = (& (Join-Path $target 'clippy.exe') --version)
        Require -Name "the installed clippy.exe runs and says clippy $Version" -Condition ($reported -eq "clippy $Version") `
            -Detail "it said '$reported'"
        $u = Start-Process -FilePath (Join-Path $target 'uninstall.exe') -ArgumentList '/S' -Wait -PassThru
        Require -Name 'silent uninstall exits 0' -Condition ($u.ExitCode -eq 0) -Detail "exit code $($u.ExitCode)"
        # The NSIS uninstaller copies itself out and finishes asynchronously.
        $deadline = (Get-Date).AddSeconds(15)
        while ((Test-Path -LiteralPath $target) -and ((Get-Date) -lt $deadline)) { Start-Sleep -Milliseconds 300 }
        Require -Name 'the install folder is gone' -Condition (-not (Test-Path -LiteralPath $target))
        Require -Name 'the uninstall entry is gone' -Condition (-not (Test-Path $UninstallKey))
        Clear-TestInstall -Under $work
        Require -Name 'no remembered install folder is left' -Condition (-not (Test-Path "$InstallDirKey\Clippy"))
        $installTested = $true
    }

    Write-Host ''
    Write-Host 'Step 4: publish' -ForegroundColor Cyan
    if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Force $OutDir | Out-Null }
    $final = Join-Path $OutDir "Clippy_${Version}_x64-setup$suffix.exe"
    Copy-Item -LiteralPath $built -Destination $final -Force
    $sha = (Get-FileHash -LiteralPath $final -Algorithm SHA256).Hash.ToLower()
    [System.IO.File]::WriteAllText("$final.sha256", "$sha  $(Split-Path -Leaf $final)`n", (New-Object System.Text.UTF8Encoding($false)))

    $size = [math]::Round((Get-Item -LiteralPath $final).Length / 1MB, 2)
    Write-Host ''
    Write-Host "  $final"
    Write-Host "  $size MB"
    Write-Host "  SHA256  $sha"
    Write-Host ''
    if ($suffix) {
        Write-Host "INSTALLER BUILT$suffix -- NOT A RELEASE ARTIFACT" -ForegroundColor Yellow
    } elseif (-not $installTested) {
        Write-Host 'INSTALLER BUILT, install round trip NOT run' -ForegroundColor Yellow
    } else {
        Write-Host 'INSTALLER BUILT and seen to install and uninstall cleanly' -ForegroundColor Green
    }
    Write-Host 'Unsigned: Windows SmartScreen will warn on first run ("More info" > "Run anyway").'
    Write-Host 'Nothing was committed, tagged or pushed.'
} finally {
    if ($installStarted) {
        Clear-TestInstall -Under $work
        if ($savedRun) {
            Set-ItemProperty -LiteralPath $RunKey -Name 'Clippy' -Value $savedRun -ErrorAction SilentlyContinue
        }
    }
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
