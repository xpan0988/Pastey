# A fake `claude -p` for the Windows tests (see windows_tests.rs). It reads
# its prompt, records it in the session's transcript, lists itself busy in
# `live\<session>.json` while it holds `live\<session>.lock` (so a killed run
# drops out of the registry, as from Claude Code's), and then does what
# `mode` says, on stream-json. An interrupt control request on stdin is
# acknowledged as Claude Code acknowledges it.
$ErrorActionPreference = 'Stop'
$Root = '{ROOT}'
$sid = ''
for ($i = 0; $i -lt $args.Count; $i++) {
  if ($args[$i] -eq '--session-id' -or $args[$i] -eq '--resume') { $sid = $args[$i + 1] }
}
$mode = (Get-Content -Raw "$Root\mode").Trim()
$ws = (Get-Location).ProviderPath
$dir = Join-Path $Root ('claude-config\projects\' + ($ws -replace '[^A-Za-z0-9]', '-'))
New-Item -ItemType Directory -Force $dir | Out-Null
$T = Join-Path $dir "$sid.jsonl"
$utf8 = New-Object System.Text.UTF8Encoding $false
$out = New-Object System.IO.StreamWriter([Console]::OpenStandardOutput(), $utf8)
$out.AutoFlush = $true
$in = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), $utf8)

function Json($value) { $value | ConvertTo-Json -Compress -Depth 10 }
function Emit($value) { $out.WriteLine((Json $value)) }
function Now { (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ') }
function Record($value) { [IO.File]::AppendAllText($T, (Json $value) + "`n", $utf8) }
function Note($text) { [IO.File]::AppendAllText("$Root\signals", "$text`n", $utf8) }

if ($mode -eq 'refuse') { [Console]::Error.WriteLine('Error: refused before the session'); exit 1 }
if ($mode -eq 'missing') {
  [Console]::Error.WriteLine("No conversation found with session ID: $sid")
  Emit ([ordered]@{ type = 'result'; subtype = 'error_during_execution'; is_error = $true; num_turns = 0; session_id = $sid })
  exit 1
}

[IO.File]::AppendAllText("$Root\pids", "$PID`n")
$line = $in.ReadLine()
$message = $line | ConvertFrom-Json
New-Item -ItemType Directory -Force "$Root\live" | Out-Null
$lock = [IO.File]::Open("$Root\live\$sid.lock", 'OpenOrCreate', 'ReadWrite', 'None')
[IO.File]::WriteAllText("$Root\live\$sid.json", '{"sessionId":"' + $sid + '","kind":"interactive","status":"busy"}')
$initSid = $sid
if ($mode -eq 'wrong-session') { $initSid = '00000000-0000-4000-8000-000000000000' }
Emit ([ordered]@{ type = 'system'; subtype = 'init'; session_id = $initSid })
Record ([ordered]@{ type = 'user'; cwd = $ws; uuid = $message.uuid; timestamp = (Now); sessionId = $sid; message = [ordered]@{ role = 'user'; content = $message.message.content } })
if ($mode -ne 'unaccepted') { $out.WriteLine('{"isReplay":true,' + $line.Substring(1)) }

function Reply($text) {
  Record ([ordered]@{ type = 'assistant'; cwd = $ws; timestamp = (Now); message = [ordered]@{ role = 'assistant'; content = @([ordered]@{ type = 'text'; text = $text }); stop_reason = 'end_turn' } })
  Emit ([ordered]@{ type = 'assistant'; message = [ordered]@{ content = @([ordered]@{ type = 'text'; text = $text }) } })
}
function Result($subtype, $isError, $turns, $text, $denials) {
  $value = [ordered]@{ type = 'result'; subtype = $subtype; is_error = $isError; num_turns = $turns; session_id = $sid; result = $text; permission_denials = @() }
  if ($null -ne $denials) { $value.permission_denials = $denials }
  Emit $value
}
function Drain { while ($null -ne $in.ReadLine()) { } }
function Finish($code) {
  Remove-Item -Force "$Root\live\$sid.json"
  $lock.Close()
  Remove-Item -Force "$Root\live\$sid.lock"
  exit $code
}
# A long-lived descendant, as a Bash tool's command would be.
function Descendant {
  $info = New-Object Diagnostics.ProcessStartInfo 'ping.exe', '-n 600 127.0.0.1'
  $info.UseShellExecute = $false
  $info.CreateNoWindow = $true
  $info.RedirectStandardOutput = $true
  $child = [Diagnostics.Process]::Start($info)
  [IO.File]::AppendAllText("$Root\descendants", [string]$child.Id + "`n")
}

switch ($mode) {
  'complete' { Reply 'Reply for Pastey'; Result 'success' $false 1 'Reply for Pastey'; Drain; Finish 0 }
  'denied' { Reply 'Done anyway'; Result 'success' $false 2 'Done anyway' @([ordered]@{ tool_name = 'Write' }); Drain; Finish 0 }
  'error' { Result 'success' $true 1 'Failed to authenticate'; Drain; Finish 1 }
  'no-turn' { Result 'success' $false 0 ''; Drain; Finish 0 }
  { $_ -in 'wrong-session', 'unaccepted' } { Reply 'Reply'; Result 'success' $false 1 'Reply'; Drain; Finish 0 }
  'extra-turn' { Reply 'First'; Result 'success' $false 1 'First'; Drain; Start-Sleep -Milliseconds 300; Reply 'Later reply'; Result 'success' $false 1 'Later reply'; Finish 0 }
  'leftover' { Descendant; Reply 'Reply for Pastey'; Result 'success' $false 1 'Reply for Pastey'; Drain; Finish 0 }
  'crash' { Finish 137 }
}

# hang, manual and stubborn: run until `finish` appears, or until
# interrupted (stubborn acknowledges an interrupt and goes on).
Descendant
# Reads stdin without blocking; at most one read is ever outstanding.
$pending = $in.ReadLineAsync()
function DrainPending {
  while ($null -ne $script:pending) {
    $next = $script:pending.Result
    $script:pending = $null
    if ($null -ne $next) { $script:pending = $in.ReadLineAsync() }
  }
}
while ($true) {
  if (Test-Path "$Root\finish") {
    Reply 'Finished later'; Result 'success' $false 1 'Finished later'; DrainPending; Finish 0
  }
  if ($null -ne $pending -and $pending.Wait(20)) {
    $next = $pending.Result
    $pending = $null
    if ($null -eq $next) { continue }
    $request = $next | ConvertFrom-Json
    if ($request.type -eq 'control_request' -and $request.request.subtype -eq 'interrupt') {
      Note 'interrupt'
      Emit ([ordered]@{ type = 'control_response'; response = [ordered]@{ subtype = 'success'; request_id = $request.request_id; response = @{} } })
      if ($mode -ne 'stubborn') {
        Record ([ordered]@{ type = 'user'; cwd = $ws; timestamp = (Now); message = [ordered]@{ role = 'user'; content = '[Request interrupted by user]' } })
        Result 'error_during_execution' $true 1 ''
        Drain
        Finish 0
      }
    }
    $pending = $in.ReadLineAsync()
  } elseif ($null -eq $pending) {
    Start-Sleep -Milliseconds 20
  }
}
