Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Invoke-PasteyAcceptanceStage -StageNumber 2 -Body {
    param($Context)

    Assert-PasteyWindows
    Assert-PasteyElevated
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    Write-Host "CURRENT_USERNAME=$($identity.Name)"
    Write-Host "ELEVATION_STATE=administrator"
    $installation = Get-PasteyInstalledProduct
    $null = Get-PasteyInstalledHelper -Installation $installation -FileName "codex-windows-sandbox-setup.exe"

    $setup = Invoke-PasteyCommand -FilePath $installation.Executable -ArgumentList @("--pastey-setup-windows-codex-sandbox-v1")
    Write-Host "SETUP_EXIT_CODE=$($setup.ExitCode)"
    Copy-PasteySandboxDiagnostics -Context $Context
    if ($setup.ExitCode -ne 0 -or -not $setup.Output.Contains("PASTEY_WINDOWS_CODEX_SANDBOX_SETUP_OK")) {
        Stop-PasteyAcceptanceFailed "Production sandbox setup did not emit its success token with exit code 0."
    }
    Assert-PasteySetupPrerequisite -RepositoryRoot $Context.RepositoryRoot
}
