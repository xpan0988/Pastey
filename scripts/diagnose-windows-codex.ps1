# Read-only diagnosis of why Pastey on this Windows account reports Codex as
# unavailable, covering both a Codex CLI and the Codex bundled with a desktop
# app. It changes nothing, installs nothing and reads no credential.
# Run in a normal (non-elevated) PowerShell:
#   powershell -ExecutionPolicy Bypass -File .\scripts\diagnose-windows-codex.ps1
# The report is printed and saved to %TEMP%\pastey-codex-diagnostics.txt with
# the Windows user name replaced by <user>. Review it before sharing.

$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.Generic.List[string]
function Say([string]$text) { $lines.Add($text) }
function Section([string]$title) { Say ''; Say "== $title" }

Section 'System'
Say ("Windows {0}; PowerShell {1}" -f [Environment]::OSVersion.Version, $PSVersionTable.PSVersion)

Section 'What a new terminal runs for "codex"'
$commands = Get-Command codex -All -ErrorAction SilentlyContinue
if (-not $commands) { Say 'Get-Command: no command named codex' }
foreach ($command in $commands) { Say ("{0}: {1}" -f $command.CommandType, $command.Source) }
Say ("where.exe: " + ((where.exe codex 2>&1) -join ' | '))

Section 'How Pastey searches (codex.exe, then codex.cmd, per PATH folder)'
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$searched = @($env:Path, $machinePath, $userPath) -join ';' -split ';' |
    Where-Object { $_ -and [IO.Path]::IsPathRooted($_) } | Select-Object -Unique
$found = $false
foreach ($dir in $searched) {
    foreach ($name in 'codex.exe', 'codex.cmd') {
        $candidate = Join-Path $dir $name
        $item = Get-Item -LiteralPath $candidate -Force -ErrorAction SilentlyContinue
        if ($item) {
            $found = $true
            Say ("{0}  attributes={1}  linktype={2}" -f $candidate, $item.Attributes, $item.LinkType)
        }
    }
}
if (-not $found) { Say 'No codex.exe or codex.cmd in any PATH folder (process, system or user PATH).' }
Say ("This terminal's PATH lacks entries of the saved user PATH: " +
    ((($userPath -split ';') | Where-Object { $_ -and ($env:Path -split ';') -notcontains $_ }) -join ' | '))

Section 'App execution aliases (packaged apps)'
Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WindowsApps" -Filter 'codex*' -Force -ErrorAction SilentlyContinue |
    ForEach-Object { Say ("{0}  attributes={1}" -f $_.FullName, $_.Attributes) }
Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'codex|chatgpt|openai' } |
    ForEach-Object { Say ("package {0} {1}" -f $_.Name, $_.Version) }

Section 'Codex bundled with a desktop app (codex-cli\codex-package.json)'
$desktopRoots = @()
if ($env:LOCALAPPDATA) { $desktopRoots += Join-Path $env:LOCALAPPDATA 'Programs' }
$desktopRoots += Get-AppxPackage -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match 'OpenAI|ChatGPT|Codex' } | ForEach-Object { $_.InstallLocation }
$manifests = foreach ($root in $desktopRoots | Where-Object { $_ }) {
    Get-ChildItem -LiteralPath $root -Recurse -Depth 5 -Filter 'codex-package.json' -ErrorAction SilentlyContinue
}
if (-not $manifests) { Say 'No codex-package.json found under per-user programs or OpenAI packages.' }
foreach ($manifestFile in $manifests) {
    Say ("manifest: " + $manifestFile.FullName)
    $manifest = Get-Content -LiteralPath $manifestFile.FullName -Raw | ConvertFrom-Json
    Say ("  layoutVersion={0} version={1} target={2} variant={3} entrypoint={4}" -f
        $manifest.layoutVersion, $manifest.version, $manifest.target, $manifest.variant, $manifest.entrypoint)
    $entry = Join-Path $manifestFile.DirectoryName $manifest.entrypoint
    $item = Get-Item -LiteralPath $entry -Force -ErrorAction SilentlyContinue
    Say ("  entrypoint exists={0} attributes={1}" -f [bool]$item, $(if ($item) { $item.Attributes } else { '-' }))
    if ($item) {
        foreach ($probe in @(@('--version'), @('app-server', '--help'))) {
            $watch = [Diagnostics.Stopwatch]::StartNew()
            $output = & $entry @probe 2>&1 | Select-Object -First 2
            Say ("  {0}: exit={1} ms={2}" -f ($probe -join ' '), $LASTEXITCODE, $watch.ElapsedMilliseconds)
            $output | ForEach-Object { Say ("    " + $_) }
        }
        $login = & $entry login status 2>&1 | Select-Object -First 1
        Say ("  login status: " + $login)
    }
}
Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -match 'codex' } |
    ForEach-Object { Say ("running: {0} {1}" -f $_.ProcessName, $_.Path) }

Section 'npm global shims and Node'
Get-ChildItem "$env:APPDATA\npm" -Filter 'codex*' -ErrorAction SilentlyContinue | ForEach-Object { Say $_.FullName }
$node = Get-Command node -ErrorAction SilentlyContinue
Say ("node: " + $(if ($node) { "$($node.Source) $((node --version) 2>&1)" } else { 'not on PATH' }))

Section 'Running Codex as Pastey does'
foreach ($probe in @(@('--version'), @('app-server', '--help'))) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $output = & codex @probe 2>&1 | Select-Object -First 3
    Say ("codex {0}: exit={1} ms={2}" -f ($probe -join ' '), $LASTEXITCODE, $watch.ElapsedMilliseconds)
    $output | ForEach-Object { Say ("  " + $_) }
}

Section 'Pastey'
Get-Process pastey -ErrorAction SilentlyContinue |
    ForEach-Object { Say ("pastey pid={0} started={1}" -f $_.Id, $_.StartTime) }
$log = Join-Path $env:LOCALAPPDATA 'pastey\logs\pastey.log'
if (Test-Path -LiteralPath $log) {
    Select-String -LiteralPath $log -Pattern 'codex_availability' | Select-Object -Last 5 |
        ForEach-Object { Say $_.Line }
} else { Say "No log at $log" }

$report = ($lines -join [Environment]::NewLine).Replace($env:USERNAME, '<user>')
$out = Join-Path $env:TEMP 'pastey-codex-diagnostics.txt'
Set-Content -LiteralPath $out -Value $report -Encoding UTF8
Write-Output $report
Write-Output ''
Write-Output "Saved to $out"
