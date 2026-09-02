Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$script:PasteyAcceptanceCommonRoot = $PSScriptRoot

function Get-PasteyRepositoryRoot {
    $root = (Resolve-Path (Join-Path $script:PasteyAcceptanceCommonRoot "..\..")).Path
    if (-not (Test-Path -LiteralPath (Join-Path $root "package.json") -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $root "src-tauri\Cargo.toml") -PathType Leaf)) {
        throw "Could not resolve the Pastey repository root from $($script:PasteyAcceptanceCommonRoot)."
    }
    return $root
}

function Test-PasteyWindows {
    return $env:OS -eq "Windows_NT"
}

function Test-PasteyAdministrator {
    if (-not (Test-PasteyWindows)) {
        return $false
    }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-PasteyPrivilegeLabel {
    if (-not (Test-PasteyWindows)) {
        return "not-windows"
    }
    if (Test-PasteyAdministrator) {
        return "administrator"
    }
    return "standard-user"
}

function Get-PasteyWindowsVersion {
    if (-not (Test-PasteyWindows)) {
        return [Environment]::OSVersion.VersionString
    }
    try {
        $os = Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction Stop
        return ("{0} {1} (build {2})" -f $os.Caption, $os.Version, $os.BuildNumber)
    }
    catch {
        return [Environment]::OSVersion.VersionString
    }
}

function Get-PasteyGitCommit {
    param([Parameter(Mandatory = $true)][string]$RepositoryRoot)

    $git = Get-Command git -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $git) {
        return "unavailable"
    }
    $commit = & $git.Path -C $RepositoryRoot rev-parse HEAD 2>$null
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace(($commit -join ""))) {
        return "unavailable"
    }
    return ($commit -join "").Trim()
}

function Stop-PasteyAcceptanceBlocked {
    param([Parameter(Mandatory = $true)][string]$Message)

    $exception = New-Object System.InvalidOperationException($Message)
    $exception.Data["PasteyAcceptanceStatus"] = "BLOCKED"
    throw $exception
}

function Stop-PasteyAcceptanceFailed {
    param([Parameter(Mandatory = $true)][string]$Message)

    throw $Message
}

function Assert-PasteyWindows {
    if (-not (Test-PasteyWindows)) {
        Stop-PasteyAcceptanceBlocked "This acceptance stage requires native Windows."
    }
}

function Assert-PasteyNonElevated {
    if (Test-PasteyAdministrator) {
        Stop-PasteyAcceptanceBlocked "This stage must run from a non-elevated PowerShell."
    }
}

function Assert-PasteyElevated {
    if (-not (Test-PasteyAdministrator)) {
        Stop-PasteyAcceptanceBlocked "This stage requires an Administrator PowerShell under the Pastey installation owner."
    }
}

function Get-PasteyRequiredCommand {
    param([Parameter(Mandatory = $true)][string]$Name)

    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command) {
        Stop-PasteyAcceptanceBlocked "Required command is unavailable: $Name"
    }
    return $command.Path
}

function Invoke-PasteyCommand {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$ArgumentList = @()
    )

    $display = @($FilePath) + $ArgumentList
    Write-Host ("COMMAND: " + ($display -join " "))
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # Windows PowerShell 5.1 promotes redirected native stderr under "Stop".
        # Let the native exit code decide success while this one process runs.
        $ErrorActionPreference = "Continue"
        $LASTEXITCODE = $null
        $output = @(& $FilePath @ArgumentList 2>&1)
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($null -eq $exitCode) {
        throw "Native command did not report an exit code: $FilePath"
    }
    foreach ($line in $output) {
        Write-Host ([string]$line)
    }
    Write-Host "COMMAND_EXIT_CODE=$exitCode"
    return [pscustomobject]@{
        ExitCode = $exitCode
        Output = (($output | ForEach-Object { [string]$_ }) -join [Environment]::NewLine)
    }
}

function Invoke-PasteyInstaller {
    param([Parameter(Mandatory = $true)][string]$InstallerPath)

    Write-Host "INSTALLER=$InstallerPath"
    Write-Host "COMMAND: $InstallerPath /S"
    $process = Start-Process -FilePath $InstallerPath -ArgumentList @("/S") -PassThru -Wait
    Write-Host "INSTALLER_EXIT_CODE=$($process.ExitCode)"
    return $process.ExitCode
}

function ConvertFrom-PasteyDisplayIcon {
    param([AllowNull()][string]$DisplayIcon)

    if ([string]::IsNullOrWhiteSpace($DisplayIcon)) {
        return $null
    }
    $value = $DisplayIcon.Trim()
    if ($value -match '^"([^"]+)"') {
        return $Matches[1]
    }
    return (($value -split ",", 2)[0]).Trim().Trim('"')
}

function Resolve-PasteyInstallation {
    $candidates = New-Object System.Collections.Generic.List[object]
    $uninstallRoot = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall"
    if (Test-Path -LiteralPath $uninstallRoot) {
        foreach ($entry in Get-ChildItem -LiteralPath $uninstallRoot -ErrorAction SilentlyContinue) {
            $record = Get-ItemProperty -LiteralPath $entry.PSPath -ErrorAction SilentlyContinue
            if ($null -eq $record) {
                continue
            }
            $displayNameProperty = $record.PSObject.Properties["DisplayName"]
            if ($null -eq $displayNameProperty -or [string]$displayNameProperty.Value -ine "pastey") {
                continue
            }
            $displayIconProperty = $record.PSObject.Properties["DisplayIcon"]
            $displayIcon = if ($null -eq $displayIconProperty) { $null } else { [string]$displayIconProperty.Value }
            $iconPath = ConvertFrom-PasteyDisplayIcon $displayIcon
            if (-not [string]::IsNullOrWhiteSpace($iconPath)) {
                $null = $candidates.Add([pscustomobject]@{ Path = $iconPath; Source = $entry.PSPath })
            }
            $installLocationProperty = $record.PSObject.Properties["InstallLocation"]
            $location = if ($null -eq $installLocationProperty) { $null } else { [string]$installLocationProperty.Value }
            if (-not [string]::IsNullOrWhiteSpace($location) -and (Test-Path -LiteralPath $location -PathType Container)) {
                $found = Get-ChildItem -LiteralPath $location -Filter "pastey.exe" -File -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($null -ne $found) {
                    $null = $candidates.Add([pscustomobject]@{ Path = $found.FullName; Source = $entry.PSPath })
                }
            }
        }
    }

    $appPathKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\pastey.exe"
    if (Test-Path -LiteralPath $appPathKey) {
        $appPath = (Get-Item -LiteralPath $appPathKey).GetValue("")
        if (-not [string]::IsNullOrWhiteSpace([string]$appPath)) {
            $null = $candidates.Add([pscustomobject]@{ Path = [string]$appPath; Source = $appPathKey })
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        foreach ($relative in @("pastey\pastey.exe", "Programs\pastey\pastey.exe")) {
            $null = $candidates.Add([pscustomobject]@{
                Path = Join-Path $env:LOCALAPPDATA $relative
                Source = "current-user LocalAppData"
            })
        }
    }

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate.Path -PathType Leaf) {
            $executable = (Resolve-Path -LiteralPath $candidate.Path).Path
            return [pscustomobject]@{
                Executable = $executable
                InstallRoot = Split-Path -Parent $executable
                Source = $candidate.Source
            }
        }
    }
    return $null
}

function Get-PasteyInstalledProduct {
    $install = Resolve-PasteyInstallation
    if ($null -eq $install) {
        Stop-PasteyAcceptanceBlocked "No current-user Pastey installation could be resolved from Windows installation state."
    }
    Write-Host "INSTALLED_PASTEY=$($install.Executable)"
    Write-Host "INSTALL_STATE_SOURCE=$($install.Source)"
    return $install
}

function Resolve-PasteyInstalledHelper {
    param(
        [Parameter(Mandatory = $true)]$Installation,
        [Parameter(Mandatory = $true)][string]$FileName
    )

    $exeDirectory = Split-Path -Parent $Installation.Executable
    $candidates = New-Object System.Collections.Generic.List[string]
    $null = $candidates.Add((Join-Path $exeDirectory $FileName))
    if ((Split-Path -Leaf $exeDirectory) -ieq "bin") {
        $packageDirectory = Split-Path -Parent $exeDirectory
        $null = $candidates.Add((Join-Path (Join-Path $packageDirectory "codex-resources") $FileName))
    }
    $null = $candidates.Add((Join-Path (Join-Path $exeDirectory "codex-resources") $FileName))

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

function Get-PasteyInstalledHelper {
    param(
        [Parameter(Mandatory = $true)]$Installation,
        [Parameter(Mandatory = $true)][string]$FileName
    )

    $helper = Resolve-PasteyInstalledHelper -Installation $Installation -FileName $FileName
    if ($null -eq $helper) {
        Stop-PasteyAcceptanceBlocked "Installed helper is unavailable through the product helper lookup: $FileName"
    }
    Write-Host "INSTALLED_HELPER_$($FileName.ToUpperInvariant().Replace('.', '_').Replace('-', '_'))=$helper"
    return $helper
}

function Get-PasteySandboxHome {
    if (-not [string]::IsNullOrWhiteSpace($env:PASTEY_APP_DATA_DIR)) {
        return (Join-Path $env:PASTEY_APP_DATA_DIR "windows-codex-sandbox")
    }
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        Stop-PasteyAcceptanceBlocked "LOCALAPPDATA is unavailable, so the production sandbox home cannot be resolved."
    }
    return (Join-Path $env:LOCALAPPDATA "windows-codex-sandbox")
}

function Assert-PasteySetupPrerequisite {
    param([Parameter(Mandatory = $true)][string]$RepositoryRoot)

    $sandboxHome = Get-PasteySandboxHome
    $markerPath = Join-Path $sandboxHome ".sandbox\setup_marker.json"
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        Stop-PasteyAcceptanceBlocked "The production sandbox setup marker is absent: $markerPath"
    }

    $setupSource = Join-Path $RepositoryRoot "src-tauri\crates\windows-codex-sandbox\src\setup.rs"
    $source = Get-Content -LiteralPath $setupSource -Raw
    $versionMatch = [regex]::Match($source, 'pub const SETUP_VERSION:\s*u32\s*=\s*(\d+)\s*;')
    if (-not $versionMatch.Success) {
        Stop-PasteyAcceptanceBlocked "Could not derive the current sandbox setup version from source."
    }
    try {
        $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    }
    catch {
        Stop-PasteyAcceptanceBlocked "The production sandbox setup marker is unreadable or malformed."
    }
    $markerVersionProperty = $marker.PSObject.Properties["version"]
    if ($null -eq $markerVersionProperty) {
        Stop-PasteyAcceptanceBlocked "The production sandbox setup marker has no version."
    }
    $expectedVersion = [int]$versionMatch.Groups[1].Value
    if ([int]$markerVersionProperty.Value -ne $expectedVersion) {
        Stop-PasteyAcceptanceBlocked "The production sandbox setup marker is stale: expected version $expectedVersion."
    }
    Write-Host "SANDBOX_SETUP_MARKER=$markerPath"
    Write-Host "SANDBOX_SETUP_VERSION=$expectedVersion"
}

function Copy-PasteySandboxDiagnostics {
    param([Parameter(Mandatory = $true)]$Context)

    $sandboxHome = Get-PasteySandboxHome
    $sandboxDirectory = Join-Path $sandboxHome ".sandbox"
    if (-not (Test-Path -LiteralPath $sandboxDirectory -PathType Container)) {
        Write-Host "SANDBOX_DIAGNOSTICS=unavailable"
        return
    }

    $logs = Get-ChildItem -LiteralPath $sandboxDirectory -Filter "sandbox.*.log" -File -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 3
    foreach ($log in $logs) {
        $destination = Join-Path $Context.RunDirectory $log.Name
        Copy-Item -LiteralPath $log.FullName -Destination $destination -Force
        Write-Host "CAPTURED_SANDBOX_LOG=$destination"
    }

    $setupError = Join-Path $sandboxDirectory "setup_error.json"
    if (Test-Path -LiteralPath $setupError -PathType Leaf) {
        $destination = Join-Path $Context.RunDirectory "setup_error.json"
        Copy-Item -LiteralPath $setupError -Destination $destination -Force
        Write-Host "CAPTURED_SETUP_ERROR=$destination"
    }
}

function Invoke-PasteyPackagedVerifier {
    param(
        [Parameter(Mandatory = $true)]$Installation,
        [Parameter(Mandatory = $true)][ValidateSet("FAIL", "BLOCKED")][string]$FailureClassification
    )

    $result = Invoke-PasteyCommand -FilePath $Installation.Executable -ArgumentList @("--pastey-verify-windows-codex-sandbox-v1")
    $verified = $result.ExitCode -eq 0 -and $result.Output.Contains("PASTEY_WINDOWS_CODEX_SANDBOX_VERIFIED")
    if (-not $verified) {
        if ($FailureClassification -eq "BLOCKED") {
            Stop-PasteyAcceptanceBlocked "The packaged production verifier prerequisite did not pass."
        }
        Stop-PasteyAcceptanceFailed "The packaged production verifier did not emit its success token with exit code 0."
    }
    return $result
}

function Write-PasteyAcceptanceMetadata {
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$Status,
        [Parameter(Mandatory = $true)][string]$Position
    )

    return @(
        "TimestampUtc=$([DateTime]::UtcNow.ToString('o'))",
        "Stage=$($Context.StageNumber)",
        "GitCommit=$($Context.GitCommit)",
        "WindowsVersion=$($Context.WindowsVersion)",
        "PrivilegeState=$($Context.PrivilegeState)",
        "LogPosition=$Position",
        "FinalStatus=$Status"
    )
}

function Invoke-PasteyAcceptanceStage {
    param(
        [Parameter(Mandatory = $true)][ValidateRange(1, 5)][int]$StageNumber,
        [Parameter(Mandatory = $true)][scriptblock]$Body
    )

    $repositoryRoot = Get-PasteyRepositoryRoot
    $artifactRoot = Join-Path $repositoryRoot "artifacts\windows-acceptance"
    New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
    $runName = "stage-{0}-{1}-{2}" -f $StageNumber, [DateTime]::UtcNow.ToString("yyyyMMddTHHmmssfffZ"), $PID
    $runDirectory = Join-Path $artifactRoot $runName
    New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
    $rawTranscript = Join-Path $runDirectory "transcript.raw.log"
    $finalLog = Join-Path $runDirectory "stage-$StageNumber.log"
    $context = [pscustomobject]@{
        StageNumber = $StageNumber
        RepositoryRoot = $repositoryRoot
        RunDirectory = $runDirectory
        FinalLog = $finalLog
        GitCommit = Get-PasteyGitCommit -RepositoryRoot $repositoryRoot
        WindowsVersion = Get-PasteyWindowsVersion
        PrivilegeState = Get-PasteyPrivilegeLabel
    }

    $status = "FAIL"
    $exitCode = 1
    $transcriptStarted = $false
    $originalLocation = (Get-Location).Path
    try {
        Start-Transcript -Path $rawTranscript -Force | Out-Null
        $transcriptStarted = $true
        Set-Location -LiteralPath $repositoryRoot
        Write-Host "PASTEY_WINDOWS_ACCEPTANCE_STAGE=$StageNumber"
        Write-Host "REPOSITORY_ROOT=$repositoryRoot"
        Write-Host "RUN_DIRECTORY=$runDirectory"
        Write-Host "START_TIMESTAMP_UTC=$([DateTime]::UtcNow.ToString('o'))"
        Write-Host "GIT_COMMIT=$($context.GitCommit)"
        Write-Host "WINDOWS_VERSION=$($context.WindowsVersion)"
        Write-Host "PRIVILEGE_STATE=$($context.PrivilegeState)"
        & $Body $context
        $status = "PASS"
        $exitCode = 0
    }
    catch {
        if ($_.Exception.Data.Contains("PasteyAcceptanceStatus") -and
            $_.Exception.Data["PasteyAcceptanceStatus"] -eq "BLOCKED") {
            $status = "BLOCKED"
            $exitCode = 2
        }
        else {
            $status = "FAIL"
            $exitCode = 1
        }
        Write-Host "ACCEPTANCE_ERROR=$($_.Exception.Message)"
        if ($StageNumber -ge 2) {
            try {
                Copy-PasteySandboxDiagnostics -Context $context
            }
            catch {
                Write-Host "DIAGNOSTIC_CAPTURE_ERROR=$($_.Exception.Message)"
            }
        }
    }
    finally {
        try {
            Set-Location -LiteralPath $originalLocation
        }
        catch {
            Write-Host "LOCATION_RESTORE_ERROR=$($_.Exception.Message)"
        }
        if ($transcriptStarted) {
            try {
                Stop-Transcript | Out-Null
            }
            catch {
                Write-Host "TRANSCRIPT_STOP_ERROR=$($_.Exception.Message)"
            }
        }

        $token = "PASTEY_ACCEPTANCE_STAGE_{0}_{1}" -f $StageNumber, $status
        $header = Write-PasteyAcceptanceMetadata -Context $context -Status $status -Position "BEGIN"
        $footer = Write-PasteyAcceptanceMetadata -Context $context -Status $status -Position "END"
        $raw = @()
        if (Test-Path -LiteralPath $rawTranscript -PathType Leaf) {
            $raw = @(Get-Content -LiteralPath $rawTranscript)
        }
        @($header + $raw + $footer + $token) | Set-Content -LiteralPath $finalLog -Encoding UTF8
        Remove-Item -LiteralPath $rawTranscript -Force -ErrorAction SilentlyContinue
        Write-Host "ACCEPTANCE_LOG=$finalLog"
        Write-Host $token
    }
    exit $exitCode
}
