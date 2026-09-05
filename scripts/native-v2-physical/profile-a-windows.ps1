[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$BridgeId,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$RunId,

    [string]$AppDataDir = "C:\pastey-physical\windows-app-data",
    [string]$ReportDir = "C:\pastey-physical\reports",

    [switch]$SelfCheck
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$manifestPath = Join-Path $repositoryRoot "src-tauri\Cargo.toml"
$harnessName = "pastey-native-v2-physical-harness"
$attemptId = "physical-native-v2-attempt-$RunId"
$windowsEvidence = Join-Path $ReportDir "native-v2-physical-windows-host-$attemptId.json"
$hostProcess = $null
$physicalHostReadyPattern = 'PHYSICAL_HOST_READY\s+git_commit=\S+\s+host_ref=(\S+)\s+bridge_id='

function Assert-CleanWorktree {
    Set-Location $repositoryRoot
    $status = & git status --porcelain=v1 --untracked-files=all
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to determine repository worktree status."
    }
    if (-not [string]::IsNullOrWhiteSpace(($status | Out-String))) {
        throw "Profile A requires a clean repository worktree before launch."
    }
}

function Invoke-HarnessCollect {
    param([string]$HarnessPath)

    & $HarnessPath collect --profile a --role windows-host `
        --app-data-dir $AppDataDir --attempt-id $attemptId --report-dir $ReportDir |
        ForEach-Object { Write-Host $_ }
    return $LASTEXITCODE
}

function ConvertTo-WindowsCommandLine {
    param([string[]]$Arguments)

    $quote = [string][char]34
    $quoted = foreach ($argument in $Arguments) {
        if ($argument.Contains($quote)) {
            throw "Double quotes are not supported in physical harness arguments."
        }
        $quote + ($argument -replace '(\\*)$', '$1$1') + $quote
    }
    return ($quoted -join ' ')
}

function Get-WindowsHostRef {
    param([AllowEmptyString()][string]$Output)

    $ready = [regex]::Match($Output, $physicalHostReadyPattern)
    if ($ready.Success) {
        return $ready.Groups[1].Value
    }
    return $null
}

function Invoke-WrapperSelfCheck {
    $quote = [string][char]34
    $literalBackslashQuote = ([string][char]92) + $quote
    $hostArguments = ConvertTo-WindowsCommandLine @(
        "host", "--app-data-dir", "C:\Program Files\Pastey", "--bridge-id", "bridge-self-check"
    )
    $expectedHostArguments = @(
        "${quote}host${quote}", "${quote}--app-data-dir${quote}",
        "${quote}C:\Program Files\Pastey${quote}", "${quote}--bridge-id${quote}",
        "${quote}bridge-self-check${quote}"
    ) -join ' '
    if ($hostArguments -ne $expectedHostArguments -or $hostArguments.Contains($literalBackslashQuote)) {
        throw "Windows command-line quoting self-check failed."
    }
    $trailingBackslashArguments = ConvertTo-WindowsCommandLine @("C:\physical\")
    if ($trailingBackslashArguments -ne ("${quote}C:\physical\\${quote}")) {
        throw "Windows trailing-backslash quoting self-check failed."
    }
    $hostRef = Get-WindowsHostRef -Output "PHYSICAL_HOST_READY git_commit=abc123 host_ref=windows-host-self-check bridge_id=bridge-self-check"
    if ($hostRef -ne "windows-host-self-check") {
        throw "Windows HostRef readiness parsing self-check failed."
    }
    Write-Host "PROFILE_A_WINDOWS_SELF_CHECK_PASS"
}

function Stop-HostProcess {
    if ($null -ne $hostProcess -and -not $hostProcess.HasExited) {
        Stop-Process -Id $hostProcess.Id -ErrorAction SilentlyContinue
        $hostProcess.WaitForExit(5000)
    }
}

if ($SelfCheck) {
    Invoke-WrapperSelfCheck
    return
}

try {
    Assert-CleanWorktree
    if ($RunId.Length -gt 64 -or $RunId -notmatch '^[A-Za-z0-9_-]+$') {
        throw "RunId must be 1-64 ASCII letters, digits, hyphens, or underscores."
    }
    $null = New-Item -ItemType Directory -Force -Path $AppDataDir
    $null = New-Item -ItemType Directory -Force -Path $ReportDir
    if (Test-Path -LiteralPath $windowsEvidence) {
        throw "Windows evidence already exists; choose a fresh RunId."
    }

    & cargo.exe build --manifest-path $manifestPath --bin $harnessName
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to build the physical harness."
    }
    $metadata = & cargo.exe metadata --manifest-path $manifestPath --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to locate the Cargo target directory."
    }
    $harnessPath = Join-Path $metadata.target_directory "debug\$harnessName.exe"
    if (-not (Test-Path -LiteralPath $harnessPath)) {
        throw "Physical harness binary was not produced at $harnessPath"
    }

    $stdoutPath = Join-Path $ReportDir "profile-a-windows-$RunId-host.stdout.log"
    $stderrPath = Join-Path $ReportDir "profile-a-windows-$RunId-host.stderr.log"
    Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
    $hostProcess = Start-Process -FilePath $harnessPath `
        -ArgumentList (ConvertTo-WindowsCommandLine @("host", "--app-data-dir", $AppDataDir, "--bridge-id", $BridgeId)) `
        -WorkingDirectory $repositoryRoot -NoNewWindow -PassThru `
        -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath

    $hostDeadline = [DateTime]::UtcNow.AddSeconds(60)
    $windowsHostRef = $null
    while ([DateTime]::UtcNow -lt $hostDeadline) {
        if ($hostProcess.HasExited) {
            throw "Windows physical Host exited before readiness. See $stderrPath"
        }
        if (Test-Path -LiteralPath $stdoutPath) {
            $windowsHostRef = Get-WindowsHostRef -Output (Get-Content -LiteralPath $stdoutPath -Raw)
            if (-not [string]::IsNullOrWhiteSpace($windowsHostRef)) {
                break
            }
        }
        Start-Sleep -Milliseconds 250
    }
    if ([string]::IsNullOrWhiteSpace($windowsHostRef)) {
        throw "Windows physical Host did not report an exact HostRef within 60 seconds. See $stdoutPath and $stderrPath"
    }

    Write-Host "WINDOWS_HOST_REF=$windowsHostRef"
    Write-Host "Run the Mac command now with this exact WindowsHostRef."

    $evidenceDeadline = [DateTime]::UtcNow.AddSeconds(120)
    $attemptObserved = $false
    $receiverEvidenceReady = $false
    while ([DateTime]::UtcNow -lt $evidenceDeadline) {
        if ($hostProcess.HasExited) {
            throw "Windows physical Host exited before evidence collection. See $stderrPath"
        }
        $collectExit = Invoke-HarnessCollect -HarnessPath $harnessPath
        if ($collectExit -eq 0 -and (Test-Path -LiteralPath $windowsEvidence)) {
            $report = Get-Content -LiteralPath $windowsEvidence -Raw | ConvertFrom-Json
            if ($report.attempt.attemptId -eq $attemptId) {
                if (-not $attemptObserved) {
                    Write-Host "WINDOWS_ATTEMPT_OBSERVED=$attemptId"
                    $attemptObserved = $true
                }
                $receiverRecords = @($report.attempt.receiverRecords | Where-Object {
                    $_.attemptId -eq $attemptId
                })
                $matchingReceipts = @($report.transferReceipts | Where-Object {
                    $_.stepId -eq "transfer-mac-windows" -and
                    -not [string]::IsNullOrWhiteSpace([string]$_.contentDigest)
                })
                $matchingStepCommits = @($report.stepCommits | Where-Object {
                    $_.stepId -eq "transfer-mac-windows" -and $_.state -eq "committed"
                })
                if ($receiverRecords.Count -eq 1 -and $matchingReceipts.Count -eq 1 -and $matchingStepCommits.Count -eq 1) {
                    $receiverEvidenceReady = $true
                    Write-Host "WINDOWS_TRANSFER_COMMIT_OBSERVED=transfer-mac-windows"
                    break
                }
            }
        }
        Start-Sleep -Milliseconds 500
    }
    if (-not $attemptObserved) {
        throw "Windows did not observe $attemptId within 120 seconds."
    }
    if (-not $receiverEvidenceReady) {
        throw "Windows did not reach receipt-bearing receiver state for $attemptId within 120 seconds."
    }

    $finalCollectExit = Invoke-HarnessCollect -HarnessPath $harnessPath
    if ($finalCollectExit -ne 0) {
        throw "Final Windows evidence collection failed."
    }
    Write-Host "WINDOWS_EVIDENCE_JSON=$windowsEvidence"
}
finally {
    Stop-HostProcess
}
