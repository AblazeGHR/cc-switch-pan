param(
    [string]$BuildRoot = 'D:\project\cc-switch-pan-build',
    [string]$InstallRoot = 'D:\openTools\cc-switch-pan',
    [switch]$RetireLegacy
)
$ErrorActionPreference = 'Stop'
$console = Join-Path $BuildRoot 'target/debug/pan-cc-switch.exe'
$gui = Join-Path $BuildRoot 'target/debug/cc-switch.exe'
$contract = & $console --version | ConvertFrom-Json
if ($LASTEXITCODE -or $contract.contract -ne 'pan-cc-switch-gui/v1' -or $contract.guiVersion -ne '4.0.15') { throw 'Build the verified GUI Fork first' }
if (!(Test-Path -LiteralPath $gui)) { throw 'GUI executable is missing' }
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
Copy-Item -LiteralPath $console -Destination (Join-Path $InstallRoot 'cc-switch.exe')
Copy-Item -LiteralPath $gui -Destination (Join-Path $InstallRoot 'cc-switch-gui.exe')
Copy-Item -LiteralPath (Join-Path $BuildRoot 'target/debug/WebView2Loader.dll') -Destination $InstallRoot
Copy-Item -LiteralPath (Join-Path $repo 'scripts/cc-switch-launcher.vbs') -Destination $InstallRoot
Copy-Item -LiteralPath (Join-Path $repo 'cc-switch-launcher.ini') -Destination $InstallRoot
# Strip only debug symbols from the install copies; preserve source build artifacts.
$strip = 'D:\software\mingw64\bin\strip.exe'
if (Test-Path -LiteralPath $strip) {
    & $strip --strip-debug (Join-Path $InstallRoot 'cc-switch.exe') (Join-Path $InstallRoot 'cc-switch-gui.exe')
    if ($LASTEXITCODE) { throw 'Could not strip install copies' }
}
'@echo off', 'wscript.exe "%~dp0cc-switch-launcher.vbs" %*' | Set-Content -LiteralPath (Join-Path $InstallRoot 'cc-switch-app.cmd') -Encoding ascii
$installed = & (Join-Path $InstallRoot 'cc-switch.exe') --version | ConvertFrom-Json
if ($LASTEXITCODE -or $installed.contract -ne 'pan-cc-switch-gui/v1') { throw 'Installed console smoke test failed; PATH untouched' }
if ($RetireLegacy) {
    $legacyDirs = @('D:\openTools\cc-switch-cli-src\src-tauri\target\release', 'D:\openTools\cc-switch-cli')
    $legacyExecutables = $legacyDirs | ForEach-Object { Join-Path $_ 'cc-switch.exe' }
    # Never stop the original GUI or any unverified process.
    Get-CimInstance Win32_Process -Filter "Name='cc-switch.exe'" | Where-Object { $_.ExecutablePath -in $legacyExecutables } | ForEach-Object {
        $listeners = @(Get-NetTCPConnection -State Listen -OwningProcess $_.ProcessId -ErrorAction SilentlyContinue)
        if ($listeners.Count) { throw 'Legacy CLI owns a listener; inspect it before retiring this process' }
        Stop-Process -Id $_.ProcessId -Force
    }
    $oldUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    # Rollback metadata stays private, outside either source repository.
    $backup = Join-Path $BuildRoot 'install-path-before.json'
    if (!(Test-Path -LiteralPath $backup)) { @{userPath=$oldUserPath} | ConvertTo-Json | Set-Content -LiteralPath $backup -Encoding utf8 }
    $remaining = @($oldUserPath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -notin ($legacyDirs + $InstallRoot) })
    $newUserPath = (@($InstallRoot) + $remaining) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
    $env:PATH = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + $newUserPath
    Add-Type @'
using System; using System.Runtime.InteropServices;
public static class PanEnvironmentRefresh {
 [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
 public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, UIntPtr wParam, string text, uint flags, uint timeout, out UIntPtr result);
}
'@
    $sent = [UIntPtr]::Zero
    [void][PanEnvironmentRefresh]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 2000, [ref]$sent)
}
@{installed=$InstallRoot;guiVersion=$installed.guiVersion;legacyRetired=[bool]$RetireLegacy;realAccountSwitches=0;guiStarted=$false} | ConvertTo-Json
