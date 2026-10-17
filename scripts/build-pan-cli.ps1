param(
    [string]$BuildRoot = 'D:\project\cc-switch-pan-build',
    [string]$Cargo = 'D:\dev\rust\cargo\bin\cargo.exe',
    [switch]$Test,
    [string]$Filter = 'pan_',
    [switch]$Gui
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$env:RUSTUP_HOME = Join-Path $BuildRoot 'rustup'
$env:CARGO_HOME = Join-Path $BuildRoot 'cargo-home'
$env:CARGO_TARGET_DIR = Join-Path $BuildRoot 'target'
$env:RUSTUP_TOOLCHAIN = '1.95.0-x86_64-pc-windows-gnu'
$env:TEMP = $env:TMP = Join-Path $BuildRoot 'tmp'
$env:PATH = (Join-Path $BuildRoot 'target/debug') + ';D:\software\mingw64\bin;' + $env:PATH
if ($Test) {
    $artifacts = & $Cargo test --manifest-path (Join-Path $repo 'src-tauri/Cargo.toml') --lib --no-run --message-format=json
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    $binary = $artifacts | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } | Select-Object -Last 1 -ExpandProperty executable
    if (!$binary) { throw 'Test executable not found' }
    python (Join-Path $PSScriptRoot 'embed-test-manifest.py') $binary (Join-Path $repo 'src-tauri/common-controls.manifest')
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    & $binary $Filter --test-threads=1
} elseif ($Gui) {
    & $Cargo build --manifest-path (Join-Path $repo 'src-tauri/Cargo.toml') --features custom-protocol --bin cc-switch --bin pan-cc-switch
} else {
    & $Cargo build --manifest-path (Join-Path $repo 'src-tauri/Cargo.toml') --bin pan-cc-switch
}
exit $LASTEXITCODE
