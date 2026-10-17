# Read-only UI Automation probe of the ChatGPT / Codex desktop app on
# Windows (research; the Windows counterpart of the macOS probe). It reads
# the main window's UI Automation tree and writes a redacted summary. It never
# invokes, types, focuses or scrolls anything, and reads no file, database,
# IPC or credential of the app.
#
# Unverified: written on a Mac and not yet run on Windows.
#
#   powershell -ExecutionPolicy Bypass -File .\tools\desktop-ui-probe\windows-uia-probe.ps1
#
# Output: %TEMP%\pastey-desktop-uia\summary.json and tree.txt. Text is
# redacted to its length and a hash, except names that match interface words.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$P = [System.Windows.Automation.AutomationPattern]

$interfaceWords = 'send|stop|new|search|settings|attach|model|dictat|approve|deny|allow|copy|' +
    '发送|停止|新|搜索|设置|附加|批准|拒绝|允许|复制|听写|模型'
function Redact([string]$text, [bool]$interface) {
    if (-not $text) { return '' }
    if ($interface -and $text.Length -le 40 -and $text -match $interfaceWords) { return $text }
    $hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash(
        [Text.Encoding]::UTF8.GetBytes($text)), 0, 4).Replace('-', '').ToLower()
    return "<$($text.Length) chars #$hash>"
}

$process = Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 -and $_.ProcessName -match 'ChatGPT|Codex|OpenAI' } |
    Select-Object -First 1
if (-not $process) { throw 'No ChatGPT / Codex desktop window found.' }
$window = $A::FromHandle($process.MainWindowHandle)

$walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
$nodes = New-Object System.Collections.Generic.List[object]
function Visit($element, [int]$depth, [string]$path) {
    if ($nodes.Count -ge 20000 -or $depth -gt 80) { return }
    $c = $element.Current
    $type = $c.ControlType.ProgrammaticName -replace 'ControlType\.', ''
    $interface = $type -in 'Button', 'SplitButton', 'MenuItem', 'CheckBox', 'ComboBox', 'TabItem'
    $patterns = @($element.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'PatternIdentifiers\.Pattern', '' })
    $valueLength = -1
    if ($patterns -contains 'Value') {
        $valueLength = $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value.Length
    }
    $nodes.Add([pscustomobject]@{
        depth = $depth; path = $path; type = $type; class = $c.ClassName
        name = Redact $c.Name $interface; automationId = $c.AutomationId
        enabled = $c.IsEnabled; focus = $c.HasKeyboardFocus; offscreen = $c.IsOffscreen
        patterns = $patterns -join ','; valueLength = $valueLength
    })
    $child = $walker.GetFirstChild($element)
    $index = 0
    while ($child) {
        Visit $child ($depth + 1) "$path.$index"
        $child = $walker.GetNextSibling($child)
        $index++
    }
}
$watch = [Diagnostics.Stopwatch]::StartNew()
Visit $window 0 'w'

# The same anchors the macOS probe found, if Chromium exposes DOM classes
# as UI Automation class names here (to be checked).
function Find([string]$pattern) { @($nodes | Where-Object { $_.class -match $pattern }) }
$sendOrStop = @($nodes | Where-Object { $_.type -eq 'Button' -and $_.name -match '^(发送|停止|send|stop)' })
$summary = [ordered]@{
    process = $process.ProcessName
    nodes = $nodes.Count
    walk_ms = $watch.ElapsedMilliseconds
    types = $nodes | Group-Object type | ForEach-Object { @{ $_.Name = $_.Count } }
    thread_container = (Find 'thread-scroll-container').Count
    composer = @(Find 'ProseMirror' | Select-Object path, type, patterns, valueLength, focus)
    placeholder = (Find '(^|\s)placeholder(\s|$)').Count
    sidebar_items = (Find 'sidebar-item').Count
    send_or_stop = @($sendOrStop | Select-Object name, enabled, patterns)
    documents = @($nodes | Where-Object type -eq 'Document' | Select-Object path, name)
}
$out = Join-Path $env:TEMP 'pastey-desktop-uia'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$summary | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 (Join-Path $out 'summary.json')
$nodes | ForEach-Object {
    ('  ' * $_.depth) + "$($_.type) .$($_.class) name=$($_.name) id=$($_.automationId) " +
        "enabled=$($_.enabled) focus=$($_.focus) offscreen=$($_.offscreen) patterns=$($_.patterns) value=$($_.valueLength) @$($_.path)"
} | Set-Content -Encoding UTF8 (Join-Path $out 'tree.txt')
$summary | ConvertTo-Json -Depth 5
Write-Output "Saved to $out (review before sharing)."
