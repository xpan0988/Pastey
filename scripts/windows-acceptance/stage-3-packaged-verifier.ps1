Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Invoke-PasteyAcceptanceStage -StageNumber 3 -Body {
    param($Context)

    Assert-PasteyWindows
    Assert-PasteyNonElevated
    $installation = Get-PasteyInstalledProduct
    $null = Get-PasteyInstalledHelper -Installation $installation -FileName "codex-command-runner.exe"
    Assert-PasteySetupPrerequisite -RepositoryRoot $Context.RepositoryRoot
    try {
        $null = Invoke-PasteyPackagedVerifier -Installation $installation -FailureClassification "FAIL"
    }
    finally {
        Copy-PasteySandboxDiagnostics -Context $Context
    }
}
