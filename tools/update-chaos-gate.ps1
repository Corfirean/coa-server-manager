param(
    [Parameter(Mandatory)][string]$BasePackage,
    [Parameter(Mandatory)][string]$CandidatePackage,
    [string]$DataSource = '',
    [string]$PreviousPackage = '',
    [ValidateSet('coa','wildcard','dual','squid','imported')][string]$Mode = 'coa',
    [switch]$SkipBuild,
    [string]$ReportFile = '',
    [string]$Target = (Join-Path $env:RUNNER_TEMP ('coa-schema-fixture-chaos-' + [guid]::NewGuid().ToString('N')))
)
$ErrorActionPreference = 'Stop'
$project = Split-Path $PSScriptRoot -Parent
$tool = Join-Path $project 'target/debug/coa-release.exe'
$test = Join-Path $project 'target/debug/examples/update_acceptance_e2e.exe'
if (Test-Path -LiteralPath $Target) { throw 'Use a new disposable fixture directory.' }
if ((Split-Path $Target -Leaf) -notlike 'coa-schema-fixture-chaos-*') { throw 'Invalid disposable fixture name.' }
if (-not $SkipBuild) {
    & cargo build --manifest-path "$project/Cargo.toml" -p coa-release -p coa-core --example update_acceptance_e2e --features coa-core/testkit --locked
    if ($LASTEXITCODE) { throw 'Could not build acceptance tools.' }
    & cargo build --manifest-path "$project/Cargo.toml" -p coa-release --locked
    if ($LASTEXITCODE) { throw 'Could not build release tools.' }
}
if (-not (Test-Path -LiteralPath $tool) -or -not (Test-Path -LiteralPath $test)) { throw 'Acceptance tools are missing.' }
& $tool extract-schema-base --package $BasePackage --fixture $Target
if ($LASTEXITCODE) { throw 'Could not prepare the signed base.' }
[IO.File]::WriteAllText((Join-Path $Target '.coa-chaos-fixture'), 'disposable update chaos fixture')
# A real independent copy: a junction would allow validation to modify source data.
if ($DataSource) {
    & robocopy $DataSource (Join-Path $Target 'Data') /E /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw 'Could not copy isolated game data.' }
} else {
    & $tool extract-game-data --package $BasePackage --fixture $Target
    if ($LASTEXITCODE) { throw 'Could not extract signed game data.' }
}
& $test init $Target
if ($LASTEXITCODE) { throw 'Could not initialize sentinels.' }
& $test seed-players $Target
if ($LASTEXITCODE) { throw 'Could not seed real account, character and inventory records.' }
if ($PreviousPackage) {
    & $test baseline-release $Target $PreviousPackage
    if ($LASTEXITCODE) { throw 'Previous published release could not be established.' }
}
& $test apply $Target $CandidatePackage
if ($LASTEXITCODE) { throw 'Candidate failed acceptance.' }
& $test prepare-mode $Target $Mode
if ($LASTEXITCODE) { throw "Could not prepare $Mode installation." }
if ($PreviousPackage) {
    # Re-establish the old signed release after mode provisioning, so the actual
    # upgrade below runs in this mode rather than only in the initial CoA setup.
    & $test baseline-release $Target $PreviousPackage
    if ($LASTEXITCODE) { throw "Previous release could not be established in $Mode mode." }
}
& $test seed-config $Target
if ($LASTEXITCODE) { throw 'Could not seed a modified user configuration.' }
& $test break-legacy-baseline $Target
if ($LASTEXITCODE) { throw 'Could not inject missing-table baseline.' }
& $test apply $Target $CandidatePackage
if ($LASTEXITCODE) { throw 'Legacy missing-table baseline was not repaired.' }
& $test verify $Target
if ($LASTEXITCODE) { throw 'Sentinels were lost.' }
foreach ($rejection in @('expect-corrupt-package','expect-locked-file','expect-occupied-port')) {
    & $test $rejection $Target $CandidatePackage
    if ($LASTEXITCODE) { throw "$rejection did not reject safely." }
}
$oldPythonFailure = $env:COA_UPDATE_PYTHON_FAILURE
try {
    $env:COA_UPDATE_PYTHON_FAILURE = '1'
    & $test expect-rejection $Target $CandidatePackage
    if ($LASTEXITCODE) { throw 'Python failure was not restored.' }
} finally { $env:COA_UPDATE_PYTHON_FAILURE = $oldPythonFailure }
$oldValidation = $env:COA_UPDATE_VALIDATION
try {
    $env:COA_UPDATE_VALIDATION = '1'
    & $test start-fixture $Target
    if ($LASTEXITCODE) { throw 'Original offline fixture could not start.' }
    $env:COA_UPDATE_PYTHON_FAILURE = '1'
    & $test expect-rejection $Target $CandidatePackage
    if ($LASTEXITCODE) { throw 'The originally running server was not restored.' }
} finally {
    $env:COA_UPDATE_PYTHON_FAILURE = $oldPythonFailure
    & $test stop-fixture $Target
    $stopResult = $LASTEXITCODE
    $env:COA_UPDATE_VALIDATION = $oldValidation
    if ($stopResult) { throw 'The offline running-state fixture could not be stopped.' }
}
$oldSqlFailure = $env:COA_UPDATE_SQL_FAILURE
try {
    & $test break-legacy-baseline $Target
    if ($LASTEXITCODE) { throw 'Could not prepare partial-DDL failure.' }
    $env:COA_UPDATE_SQL_FAILURE = 'manager_repair__20261005_reconcile_wildcard_tables'
    & $test expect-rejection $Target $CandidatePackage
    if ($LASTEXITCODE) { throw 'Partial DDL/SQL failure was not restored.' }
} finally { $env:COA_UPDATE_SQL_FAILURE = $oldSqlFailure }
$oldCrash = $env:COA_UPDATE_CRASH_AT
try {
    $env:COA_UPDATE_CRASH_AT = 'snapshot-recorded'
    & $test apply $Target $CandidatePackage
    if ($LASTEXITCODE -ne 91) { throw 'The recorded-snapshot interruption was not exercised.' }
    $env:COA_UPDATE_CRASH_AT = ''
    foreach ($corruption in @('expect-corrupt-journal','expect-corrupt-recovery-point')) {
        & $test $corruption $Target
        if ($LASTEXITCODE) { throw "Corruption guard $corruption failed." }
    }
    & $test rollback $Target
    if ($LASTEXITCODE) { throw 'Recovery after removing injected corruption failed.' }
    foreach ($point in @('stopped','snapshot','trial','before-file:0','journal-file:0','written-file:0','completed-file:0','applied','before-migrations','before-sql:manager_repair__20261005_reconcile_wildcard_tables','after-sql:manager_repair__20261005_reconcile_wildcard_tables','recorded-sql:manager_repair__20261005_reconcile_wildcard_tables','migrated','validated','before-commit','commit-metadata')) {
        $env:COA_UPDATE_CRASH_AT = ''
        & $test break-legacy-baseline $Target
        if ($LASTEXITCODE) { throw 'Could not reset isolated baseline.' }
        & $test dirty-fixture $Target $CandidatePackage
        if ($LASTEXITCODE) { throw 'Could not arrange a real file replacement.' }
        $env:COA_UPDATE_CRASH_AT = $point
        & $test apply $Target $CandidatePackage
        if ($LASTEXITCODE -ne 91) { throw "Fault point $point was not exercised (exit $LASTEXITCODE)." }
        foreach ($recoveryPoint in @('restore-stopped','restore-databases','restore-files')) {
            $env:COA_UPDATE_CRASH_AT = $recoveryPoint
            & $test rollback $Target
            # restore-databases may not be reached before any live database changes.
            if ($LASTEXITCODE -notin @(0,91)) { throw "Recovery fault $recoveryPoint failed." }
            if ($LASTEXITCODE -eq 0) { break }
        }
        $env:COA_UPDATE_CRASH_AT = ''
        & $test rollback $Target
        if ($LASTEXITCODE) { throw "Recovery from $point failed." }
    }
} finally { $env:COA_UPDATE_CRASH_AT = $oldCrash }
$manifest = Get-Content (Join-Path $CandidatePackage 'manifest.json') -Raw | ConvertFrom-Json
$managerSource = (& git -C $project rev-parse HEAD).Trim()
if ($LASTEXITCODE -or $managerSource -notmatch '^[0-9a-f]{40}$') { throw 'Cannot identify the tested manager source.' }
$report = @{ schema = 1; version = $manifest.version; manifestSha256 = (Get-FileHash (Join-Path $CandidatePackage 'manifest.json') -Algorithm SHA256).Hash.ToLower(); manager = (Get-FileHash $test -Algorithm SHA256).Hash.ToLower(); managerSource = $managerSource; mode = $Mode; baseline = $(if ($PreviousPackage) { (Get-Content (Join-Path $PreviousPackage 'manifest.json') -Raw | ConvertFrom-Json).version } else { 'base' }); result = 'passed' }
$report.coverage = @('signed-package-corruption','locked-file','occupied-port','python-startup-failure','partial-ddl-invalid-sql','update-process-crash','recovery-process-crash','modified-config-preservation','running-state-recovery','player-data-preservation','corrupt-journal','corrupt-recovery-point')
if (-not $ReportFile) { $ReportFile = Join-Path $CandidatePackage 'acceptance.json' }
$report | ConvertTo-Json | Set-Content $ReportFile -Encoding utf8NoBOM
