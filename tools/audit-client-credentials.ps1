<#
.SYNOPSIS
  Controlled before/after audit of what the Ascension game client persists when the player logs in with "Remember" ticked (Phase 12).

.DESCRIPTION
  The Manager does not write the client's remembered login today: its format is not understood well enough to do it safely (see docs/CONTROL_PROTOCOL.md, "Native
  login prefill"). This script is the audit that has to be done by hand, once, with a disposable account, before that can change. It never prints file contents.

    1. Close the game. Run:      .\audit-client-credentials.ps1 -Phase before
    2. Start the game, log in with a DISPOSABLE account (name and password you pick for this test), tick "Remember", reach character selection, quit the game.
    3. Run:                      $env:COA_AUDIT_USER='<test name>'; $env:COA_AUDIT_PW='<test password>'; .\audit-client-credentials.ps1 -Phase after
    4. Repeat steps 1-3 with a different disposable account to see which bytes follow the account, and once more with "Remember" unticked to see what that removes.

  The report lists the files that changed (path, size before/after, entropy in bits per byte) and, for each, only booleans: whether the test name or password appears
  in the file as plain text, UTF-16 text, hex or base64. A file with high entropy and no match is obfuscated or encrypted; the next step is then to learn the scheme
  from the client itself, which is out of scope for the Manager's control plane and must not involve injecting into or patching the running game.
#>
param(
  [Parameter(Mandatory = $true)][ValidateSet('before', 'after')][string]$Phase,
  [string]$Client = 'C:\games\Ascension',
  [string]$State = (Join-Path $env:TEMP 'coa-client-audit.json')
)
$ErrorActionPreference = 'Stop'

function Snapshot([string]$root) {
  $out = @{}
  foreach ($sub in 'WTF', 'Data') {
    $dir = Join-Path $root $sub
    if (-not (Test-Path $dir)) { continue }
    Get-ChildItem $dir -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.Length -lt 4MB -and $_.Extension -notmatch '^\.(mpq|dbc|blp|m2|wmo|adt|wdt|skin|anim|bone|phys|wav|mp3|ogg|ttf)$' } | ForEach-Object {
      $out[$_.FullName.Substring($root.Length)] = @{ size = $_.Length; sha = (Get-FileHash $_.FullName -Algorithm SHA256).Hash; mtime = $_.LastWriteTimeUtc.ToString('o') }
    }
  }
  foreach ($f in Get-ChildItem $root -File -ErrorAction SilentlyContinue | Where-Object { $_.Extension -match '^\.(wtf|json|cfg|ini|txt)$' -and $_.Length -lt 4MB }) {
    $out[$f.FullName.Substring($root.Length)] = @{ size = $f.Length; sha = (Get-FileHash $f.FullName -Algorithm SHA256).Hash; mtime = $f.LastWriteTimeUtc.ToString('o') }
  }
  $out
}

function Entropy([byte[]]$b) {
  if ($b.Length -eq 0) { return 0 }
  $count = New-Object 'int[]' 256
  foreach ($x in $b) { $count[$x]++ }
  $h = 0.0
  foreach ($c in $count) { if ($c -gt 0) { $p = $c / $b.Length; $h -= $p * [Math]::Log($p, 2) } }
  [Math]::Round($h, 2)
}

$latin1 = [Text.Encoding]::GetEncoding(28591)
function Contains([byte[]]$haystack, [string]$needle) {
  if ([string]::IsNullOrEmpty($needle)) { return 'n/a' }
  $forms = @(
    [Text.Encoding]::UTF8.GetBytes($needle), [Text.Encoding]::Unicode.GetBytes($needle),
    [Text.Encoding]::ASCII.GetBytes(([BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($needle)) -replace '-', '').ToLower()),
    [Text.Encoding]::ASCII.GetBytes([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($needle)).TrimEnd('='))
  )
  $text = $latin1.GetString($haystack)
  foreach ($f in $forms) { if ($text.Contains($latin1.GetString($f))) { return $true } }
  $false
}

if ($Phase -eq 'before') {
  Snapshot $Client | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 $State
  "Snapshot saved ($State). Now log in with the disposable account and quit the game."
  return
}

$before = @{}
(Get-Content $State -Raw | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $before[$_.Name] = $_.Value }
$after = Snapshot $Client
$user = $env:COA_AUDIT_USER; $pw = $env:COA_AUDIT_PW
$changed = @()
foreach ($k in $after.Keys) { if (-not $before.ContainsKey($k) -or $before[$k].sha -ne $after[$k].sha) { $changed += $k } }
foreach ($k in $before.Keys) { if (-not $after.ContainsKey($k)) { "REMOVED  $k" } }
foreach ($k in ($changed | Sort-Object)) {
  $bytes = [IO.File]::ReadAllBytes((Join-Path $Client $k))
  '{0,-8} {1}  size {2} -> {3}  entropy {4} bits/byte  name-in-file: {5}  password-in-file: {6}' -f $(if ($before.ContainsKey($k)) { 'CHANGED' } else { 'NEW' }), $k, $(if ($before.ContainsKey($k)) { $before[$k].size } else { '-' }), $after[$k].size, (Entropy $bytes), (Contains $bytes $user), (Contains $bytes $pw)
}
if (-not $changed) { 'Nothing changed under WTF, Data or the client folder.' }
