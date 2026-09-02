Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Invoke-PasteyAcceptanceStage -StageNumber 1 -Body {
    param($Context)

    Assert-PasteyWindows
    Assert-PasteyNonElevated
    $git = Get-PasteyRequiredCommand "git.exe"
    $rustc = Get-PasteyRequiredCommand "rustc.exe"
    $cargo = Get-PasteyRequiredCommand "cargo.exe"
    $node = Get-PasteyRequiredCommand "node.exe"
    $npm = Get-PasteyRequiredCommand "npm.cmd"

    $gitCommit = Invoke-PasteyCommand -FilePath $git -ArgumentList @("rev-parse", "HEAD")
    if ($gitCommit.ExitCode -ne 0) {
        Stop-PasteyAcceptanceFailed "Could not capture the Git commit."
    }
    $gitStatus = Invoke-PasteyCommand -FilePath $git -ArgumentList @("status", "--short")
    if ($gitStatus.ExitCode -ne 0) {
        Stop-PasteyAcceptanceFailed "Could not capture Git status."
    }
    $rustVersion = Invoke-PasteyCommand -FilePath $rustc -ArgumentList @("-vV")
    if ($rustVersion.ExitCode -ne 0) {
        Stop-PasteyAcceptanceFailed "rustc -vV failed."
    }
    $hostMatch = [regex]::Match($rustVersion.Output, '(?m)^host:\s*(\S+)\s*$')
    if (-not $hostMatch.Success) {
        Stop-PasteyAcceptanceFailed "Could not derive the Rust host triple."
    }
    $hostTriple = $hostMatch.Groups[1].Value
    Write-Host "RUST_HOST_TRIPLE=$hostTriple"
    $expectedHelpers = @(
        (Join-Path $Context.RepositoryRoot "src-tauri\binaries\codex-command-runner-$hostTriple.exe"),
        (Join-Path $Context.RepositoryRoot "src-tauri\binaries\codex-windows-sandbox-setup-$hostTriple.exe")
    )
    foreach ($helperPath in $expectedHelpers) {
        Write-Host "REQUIRED_STAGED_HELPER=$helperPath"
    }
    $cargoVersion = Invoke-PasteyCommand -FilePath $cargo -ArgumentList @("-vV")
    if ($cargoVersion.ExitCode -ne 0) {
        Stop-PasteyAcceptanceFailed "cargo -vV failed."
    }
    foreach ($versionCommand in @(
        @{ File = $node; Arguments = @("--version"); Label = "node" },
        @{ File = $npm; Arguments = @("--version"); Label = "npm" }
    )) {
        $version = Invoke-PasteyCommand -FilePath $versionCommand.File -ArgumentList $versionCommand.Arguments
        if ($version.ExitCode -ne 0) {
            Stop-PasteyAcceptanceFailed "$($versionCommand.Label) version query failed."
        }
    }

    $npmCi = Invoke-PasteyCommand -FilePath $npm -ArgumentList @("ci")
    if ($npmCi.ExitCode -ne 0) {
        Stop-PasteyAcceptanceFailed "npm ci failed."
    }
    $tauriBuild = Invoke-PasteyCommand -FilePath $npm -ArgumentList @("run", "tauri:build:windows")
    foreach ($helperPath in $expectedHelpers) {
        Write-Host "STAGED_HELPER_PATH=$helperPath"
        Write-Host "STAGED_HELPER_PRESENT=$(Test-Path -LiteralPath $helperPath -PathType Leaf)"
    }
    $bundleRoot = Join-Path $Context.RepositoryRoot "src-tauri\target\release\bundle"
    if (Test-Path -LiteralPath $bundleRoot -PathType Container) {
        foreach ($artifact in Get-ChildItem -LiteralPath $bundleRoot -Recurse -File | Where-Object {
            $_.Extension -ieq ".msi" -or $_.Name -ilike "*-setup.exe"
        }) {
            Write-Host "BUNDLE_PATH_AFTER_BUILD=$($artifact.FullName)"
            Write-Host "BUNDLE_LAST_WRITE_UTC=$($artifact.LastWriteTimeUtc.ToString('o'))"
        }
    }
    if ($tauriBuild.ExitCode -ne 0) {
        Stop-PasteyAcceptanceFailed "The production Windows Tauri build failed."
    }

    foreach ($helperPath in $expectedHelpers) {
        if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
            Stop-PasteyAcceptanceFailed "The production build did not stage required helper $helperPath."
        }
        Write-Host "STAGED_HELPER=$((Resolve-Path -LiteralPath $helperPath).Path)"
    }

    if (-not (Test-Path -LiteralPath $bundleRoot -PathType Container)) {
        Stop-PasteyAcceptanceFailed "The production build produced no Windows bundle directory."
    }
    $bundles = @(Get-ChildItem -LiteralPath $bundleRoot -Recurse -File | Where-Object {
        $_.Extension -ieq ".msi" -or $_.Name -ilike "*-setup.exe"
    })
    if ($bundles.Count -eq 0) {
        Stop-PasteyAcceptanceFailed "The production build produced no Windows installer."
    }
    foreach ($bundle in $bundles) {
        Write-Host "PRODUCED_BUNDLE=$($bundle.FullName)"
    }

    $package = Get-Content -LiteralPath (Join-Path $Context.RepositoryRoot "package.json") -Raw | ConvertFrom-Json
    $expectedInstaller = "pastey_$($package.version)_x64-setup.exe"
    $installers = @($bundles | Where-Object { $_.Name -ieq $expectedInstaller })
    if ($installers.Count -ne 1) {
        Stop-PasteyAcceptanceFailed "Expected exactly one NSIS installer named $expectedInstaller; found $($installers.Count)."
    }
    $installerExit = Invoke-PasteyInstaller -InstallerPath $installers[0].FullName
    if ($installerExit -ne 0) {
        Stop-PasteyAcceptanceFailed "The Windows installer failed with exit code $installerExit."
    }

    $installation = $null
    for ($attempt = 0; $attempt -lt 20 -and $null -eq $installation; $attempt++) {
        $installation = Resolve-PasteyInstallation
        if ($null -eq $installation) {
            Start-Sleep -Milliseconds 250
        }
    }
    if ($null -eq $installation) {
        Stop-PasteyAcceptanceFailed "The installer succeeded but the current-user Pastey installation could not be resolved."
    }
    Write-Host "INSTALLED_PASTEY=$($installation.Executable)"
    Write-Host "INSTALL_STATE_SOURCE=$($installation.Source)"
    foreach ($helperName in @("codex-command-runner.exe", "codex-windows-sandbox-setup.exe")) {
        $helper = Resolve-PasteyInstalledHelper -Installation $installation -FileName $helperName
        if ($null -eq $helper) {
            Stop-PasteyAcceptanceFailed "Installed Pastey cannot resolve required helper $helperName."
        }
        Write-Host "INSTALLED_HELPER=$helper"
    }
}
