# A fake `codex app-server --stdio` for the Windows Stop tests (see
# windows_stop_tests.rs). Each turn starts a long-lived descendant, as
# Codex's sandbox runs a command through `codex-command-runner`. Like Codex
# 0.163.0 on Windows (measured), `turn/interrupt` reports the turn
# interrupted at once and leaves that command running. What happens when
# stdin closes depends on `mode`:
#   cooperative: end the descendants, then exit (as Codex does, measured)
#   stubborn:    exit, leaving the descendants running
#   lingering:   keep running, with the descendants
#   complete:    each turn completes with a reply; then exit
$ErrorActionPreference = 'Stop'
$Root = '{ROOT}'
$mode = (Get-Content -Raw "$Root\mode").Trim()
$utf8 = New-Object System.Text.UTF8Encoding $false
$out = New-Object System.IO.StreamWriter([Console]::OpenStandardOutput(), $utf8)
$out.AutoFlush = $true
$in = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), $utf8)
[IO.File]::AppendAllText("$Root\servers", "$PID`n")

function Emit($value) { $out.WriteLine(($value | ConvertTo-Json -Compress -Depth 10)) }
function Reply($id, $result) { Emit ([ordered]@{ id = $id; result = $result }) }
$children = @()
$turns = 0
function Descendant {
  $info = New-Object Diagnostics.ProcessStartInfo 'ping.exe', '-n 600 127.0.0.1'
  $info.UseShellExecute = $false
  $info.CreateNoWindow = $true
  $info.RedirectStandardOutput = $true
  $child = [Diagnostics.Process]::Start($info)
  [IO.File]::AppendAllText("$Root\descendants", [string]$child.Id + "`n")
  $script:children += $child
}

while ($null -ne ($line = $in.ReadLine())) {
  $request = $line | ConvertFrom-Json
  if ($null -eq $request.id) { continue }
  switch ($request.method) {
    'initialize' { Reply $request.id ([ordered]@{ userAgent = 'Pastey/0.163.0 (Windows 10.0; x86_64)' }) }
    'thread/start' { Reply $request.id ([ordered]@{ thread = [ordered]@{ id = 'thread-1' } }) }
    'thread/resume' { Reply $request.id ([ordered]@{ thread = [ordered]@{ id = $request.params.threadId } }) }
    'turn/start' {
      $turns++
      $turn = "turn-$turns"
      Reply $request.id ([ordered]@{ turn = [ordered]@{ id = $turn } })
      if ($mode -eq 'complete') {
        Emit ([ordered]@{ method = 'item/completed'; params = [ordered]@{ threadId = 'thread-1'; turnId = $turn; item = [ordered]@{ type = 'agentMessage'; id = 'm'; text = 'done'; phase = 'final_answer' } } })
        Emit ([ordered]@{ method = 'turn/completed'; params = [ordered]@{ threadId = 'thread-1'; turn = [ordered]@{ id = $turn; status = 'completed' } } })
      } else {
        Descendant
        Emit ([ordered]@{ method = 'item/started'; params = [ordered]@{ threadId = 'thread-1'; turnId = $turn; item = [ordered]@{ type = 'commandExecution'; id = 'c' } } })
      }
    }
    'turn/interrupt' {
      Reply $request.id @{}
      Emit ([ordered]@{ method = 'turn/completed'; params = [ordered]@{ threadId = $request.params.threadId; turn = [ordered]@{ id = $request.params.turnId; status = 'interrupted' } } })
    }
    default { Reply $request.id @{} }
  }
}

switch ($mode) {
  'cooperative' { foreach ($child in $children) { $child.Kill() } }
  'lingering' { while ($true) { Start-Sleep -Seconds 1 } }
}
exit 0
