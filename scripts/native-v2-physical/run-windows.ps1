Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $repositoryRoot
& cargo.exe run --manifest-path src-tauri\Cargo.toml --bin pastey-native-v2-physical-harness -- @args
exit $LASTEXITCODE
