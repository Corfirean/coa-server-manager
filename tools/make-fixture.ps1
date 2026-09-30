# Builds a disposable copy of a CoA Repack on different ports for start/stop testing.
# NEVER writes to the source repack. Data\ is a junction to the source (read-only use by the server).
param(
    [string]$Source = 'C:\games\CoA-Repack',
    [string]$Target = 'C:\games\coa-fixture',
    [int]$PortBase = 13000
)
$ErrorActionPreference = 'Stop'
if ((Resolve-Path $Source).Path -eq $Target) { throw 'source and target are the same' }
if (Test-Path $Target) { throw "$Target already exists; delete it first (fixtures are disposable)" }
New-Item -ItemType Directory $Target | Out-Null

function Copy-Tree($rel, [string[]]$xd = @(), [string[]]$xf = @()) {
    $args = @("$Source\$rel", "$Target\$rel", '/E', '/R:1', '/W:1', '/NFL', '/NDL', '/NJH', '/NJS', '/NP')
    if ($xd) { $args += '/XD'; $args += $xd }
    if ($xf) { $args += '/XF'; $args += $xf }
    & robocopy @args | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed for $rel ($LASTEXITCODE)" }
}

Copy-Tree 'Core' -xd @('Logs', 'Crashes') -xf @('*.pre-*', '*.bak*', '*.orig', '*.pdb', '*.log', '*.prev_*')
Copy-Tree 'Settings'
Copy-Tree 'Scripts'
Copy-Tree 'Runtime'
Copy-Tree 'BugReport' -xd @('Logs', 'reports')
foreach ($d in 'bin', 'lib', 'share') { Copy-Tree "mysql\$d" }
Copy-Item "$Source\RELEASE.json", "$Source\MANIFEST.json" $Target

# Pristine packaged database (never the live data directory).
& 'C:\Program Files\7-Zip\7z.exe' x "$Source\mysql\data.7z" "-o$Target\mysql" -y | Out-Null
if (-not (Test-Path "$Target\mysql\data\acore_characters")) { throw 'database extraction failed' }

New-Item -ItemType Junction -Path "$Target\Data" -Target "$Source\Data" | Out-Null

$ports = @{ mysqlPort = $PortBase + 307; authPort = $PortBase + 724; worldPort = $PortBase + 85; raPort = $PortBase + 443 }
$cfg = Get-Content "$Target\Settings\repack.json" -Raw | ConvertFrom-Json
foreach ($k in $ports.Keys) { $cfg.$k = $ports[$k] }
$cfg | ConvertTo-Json | Set-Content "$Target\Settings\repack.json" -Encoding utf8
"fixture ready at $Target ports: $($ports | ConvertTo-Json -Compress)"
