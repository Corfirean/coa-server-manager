<#
.SYNOPSIS
  Recreate one disposable MySQL realm of the portable-characters live tests from the pristine schema fixture.

.DESCRIPTION
  The live tests (crates/coa-core/src/portable/realm/live*.rs) run against two disposable MySQL servers, realm A (port 13998)
  and realm B (port 13997). Most tests leave them as they found them; the ones that update a fixture character in place
  (COA_PORTABLE_LIVE_MUTATE_A=1) do not. This script throws the realm's data directory away and builds it again.

  It stops ONLY the mysqld whose process id is in <Root>\<Name>\pid.txt and whose command line names that data directory.
  It never stops anything by image name.

.EXAMPLE
  pwsh tools/reset_live_realm.ps1 -Root $env:TEMP\realms -Name A -Port 13998 -Fixture C:\games\coa-schema-fixture-20261005 -Sql crates\coa-core\src\portable\realm\testdata\realm-fixture.sql
#>
param(
  [Parameter(Mandatory)] [string] $Root,
  [Parameter(Mandatory)] [ValidateSet('A', 'B')] [string] $Name,
  [Parameter(Mandatory)] [int] $Port,
  [Parameter(Mandatory)] [string] $Fixture,
  [Parameter(Mandatory)] [string] $Sql,
  [string] $RootPassword = 'portable-test'
)
$ErrorActionPreference = 'Stop'
$dir = Join-Path $Root $Name
$data = Join-Path $dir 'data'
$bin = Join-Path $Fixture 'mysql'
$pidFile = Join-Path $dir 'pid.txt'

if (Test-Path $pidFile) {
  $old = [int](Get-Content $pidFile -Raw).Trim()
  $proc = Get-CimInstance Win32_Process -Filter "ProcessId = $old" -ErrorAction SilentlyContinue
  if ($proc -and $proc.Name -eq 'mysqld.exe' -and $proc.CommandLine -like "*$data*") {
    Stop-Process -Id $old -Force
    Wait-Process -Id $old -Timeout 30 -ErrorAction SilentlyContinue
  } elseif ($proc) {
    throw "process $old is not the disposable mysqld of realm $Name; refusing to stop it"
  }
}
for ($i = 0; $i -lt 20 -and (Test-Path $data); $i++) {
  try { Remove-Item $data -Recurse -Force -ErrorAction Stop } catch { Start-Sleep 1 }
}
if (Test-Path $data) { throw "could not remove $data" }
robocopy (Join-Path $Fixture 'mysql\data') $data /E /NFL /NDL /NJH /NJS /NP | Out-Null

$args = @('--no-defaults', '--no-monitor', "--basedir=$bin", "--datadir=$data", "--port=$Port", '--bind-address=127.0.0.1', '--mysqlx=OFF', '--skip-log-bin', '--innodb-buffer-pool-size=256M', "--init-file=$(Join-Path $Root 'init.sql')", '--console')
$p = Start-Process -FilePath (Join-Path $bin 'bin\mysqld.exe') -ArgumentList $args -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $dir 'out.log') -RedirectStandardError (Join-Path $dir 'err.log')
$p.Id | Out-File $pidFile
for ($i = 0; $i -lt 60; $i++) {
  Start-Sleep 1
  if ((Get-Content (Join-Path $dir 'err.log') -Tail 5 -ErrorAction SilentlyContinue) -match 'ready for connections') { break }
}
$env:MYSQL_PWD = $RootPassword
Get-Content $Sql -Raw | & (Join-Path $bin 'bin\mysql.exe') --protocol=tcp --host=127.0.0.1 "--port=$Port" --user=root --default-character-set=utf8mb4 --max-allowed-packet=64M
"realm $Name reset: pid $($p.Id), port $Port"
