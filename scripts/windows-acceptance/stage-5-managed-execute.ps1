Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Invoke-PasteyAcceptanceStage -StageNumber 5 -Body {
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

        $build = Invoke-PasteyCommand -FilePath $cargo -ArgumentList @(
            "build", "--manifest-path", "src-tauri/Cargo.toml", "--features", "native-windows-acceptance",
            "--bin", "pastey", "--bin", "pastey-managed-execute-probe"
        )
        if ($build.ExitCode -ne 0) {
            Stop-PasteyAcceptanceFailed "The opt-in Managed Execute acceptance binaries failed to build."
        }

        $testName = "managed_worker_coordinator::tests::native_windows_managed_execute_through_codex_backend"
        $test = Invoke-PasteyCommand -FilePath $cargo -ArgumentList @(
            "test", "--manifest-path", "src-tauri/Cargo.toml", "--features", "native-windows-acceptance",
            "--bin", "pastey", $testName, "--", "--exact", "--ignored", "--nocapture"
        )
        if ($test.ExitCode -ne 0 -or
            -not $test.Output.Contains("test $testName ... ok") -or
            -not $test.Output.Contains("test result: ok. 1 passed; 0 failed")) {
            Stop-PasteyAcceptanceFailed "The exact production-path Managed Execute acceptance test did not pass."
        }
    }
    finally {
        $env:PATH = $originalPath
        Copy-PasteySandboxDiagnostics -Context $Context
    }
}
