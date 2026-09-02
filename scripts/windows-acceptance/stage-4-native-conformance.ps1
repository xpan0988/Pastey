Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Invoke-PasteyAcceptanceStage -StageNumber 4 -Body {
    param($Context)

    Assert-PasteyWindows
    Assert-PasteyNonElevated
    $cargo = Get-PasteyRequiredCommand "cargo.exe"
    $installation = Get-PasteyInstalledProduct
    $runner = Get-PasteyInstalledHelper -Installation $installation -FileName "codex-command-runner.exe"
    Assert-PasteySetupPrerequisite -RepositoryRoot $Context.RepositoryRoot

    $originalPath = $env:PATH
    try {
        $runnerDirectory = Split-Path -Parent $runner
        $env:PATH = "$runnerDirectory;$originalPath"
        $resolvedRunner = Get-Command "codex-command-runner.exe" -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -eq $resolvedRunner -or (Resolve-Path -LiteralPath $resolvedRunner.Path).Path -ne (Resolve-Path -LiteralPath $runner).Path) {
            Stop-PasteyAcceptanceBlocked "The installed Codex command runner did not resolve from the process-local PATH."
        }
        Write-Host "RESOLVED_CODEX_COMMAND_RUNNER=$($resolvedRunner.Path)"
        $null = Invoke-PasteyPackagedVerifier -Installation $installation -FailureClassification "BLOCKED"

        $testName = "native_windows_codex_execution_world_conformance"
        $test = Invoke-PasteyCommand -FilePath $cargo -ArgumentList @(
            "test", "--manifest-path", "src-tauri/Cargo.toml", "--test", "windows_execution_world",
            $testName, "--", "--exact", "--ignored", "--nocapture"
        )
        if ($test.ExitCode -ne 0 -or
            -not $test.Output.Contains("PASTEY_WINDOWS_CODEX_SANDBOX_VERIFIED") -or
            -not $test.Output.Contains("test $testName ... ok") -or
            -not $test.Output.Contains("test result: ok. 1 passed; 0 failed")) {
            Stop-PasteyAcceptanceFailed "The exact native Windows conformance test did not pass."
        }
    }
    finally {
        $env:PATH = $originalPath
        Copy-PasteySandboxDiagnostics -Context $Context
    }
}
