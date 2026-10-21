param(
    [Parameter(Mandatory)][string]$BasePackage,
    [Parameter(Mandatory)][string]$CandidatePackage,
    [string]$PreviousPackage = '',
    [string]$DataSource = '',
    [ValidateRange(1,5)][int]$MaxParallel = 2,
    [string]$WorkDirectory = (Join-Path $env:RUNNER_TEMP ('coa-acceptance-' + [guid]::NewGuid().ToString('N')))
)
$ErrorActionPreference = 'Stop'
$project = Split-Path $PSScriptRoot -Parent
& git -C $project diff --quiet HEAD --
if ($LASTEXITCODE) { throw 'Release acceptance requires committed manager source; tracked changes are present.' }
$untrackedSource = & git -C $project ls-files --others --exclude-standard -- crates src src-tauri tools .github
if ($LASTEXITCODE -or $untrackedSource) { throw 'Release acceptance requires committed manager source; untracked source files are present.' }
$base = (Resolve-Path -LiteralPath $BasePackage).Path
$candidate = (Resolve-Path -LiteralPath $CandidatePackage).Path
$previous = if ($PreviousPackage) { (Resolve-Path -LiteralPath $PreviousPackage).Path } else { '' }
$data = if ($DataSource) { (Resolve-Path -LiteralPath $DataSource).Path } else { '' }
if (Test-Path -LiteralPath $WorkDirectory) { throw 'Use a fresh acceptance work directory.' }
New-Item -ItemType Directory -Path $WorkDirectory | Out-Null
$work = (Resolve-Path -LiteralPath $WorkDirectory).Path
& cargo build --manifest-path "$project/Cargo.toml" -p coa-core --example update_acceptance_e2e --features testkit --locked
if ($LASTEXITCODE) { throw 'Could not build the marked-fixture acceptance tool.' }
& cargo build --manifest-path "$project/Cargo.toml" -p coa-release --locked
if ($LASTEXITCODE) { throw 'Could not build signature verification tools.' }

$queue = [Collections.Generic.Queue[object]]::new()
foreach ($mode in @('coa','wildcard','dual','squid','imported')) {
    $queue.Enqueue(@{ mode=$mode; baseline='base'; previous='' })
    if ($previous) { $queue.Enqueue(@{ mode=$mode; baseline='previous'; previous=$previous }) }
}
$active = [Collections.Generic.List[object]]::new()
$completed = [Collections.Generic.List[object]]::new()
try {
    while ($queue.Count -or $active.Count) {
        while ($queue.Count -and $active.Count -lt $MaxParallel) {
            $job = $queue.Dequeue()
            $name = "$($job.mode)-$($job.baseline)"
            $report = Join-Path $work "$name.json"
            $target = Join-Path $work "coa-schema-fixture-chaos-$name"
            $arguments = @('-NoProfile','-File', ('"' + (Join-Path $PSScriptRoot 'update-chaos-gate.ps1') + '"'), '-SkipBuild', '-BasePackage', ('"' + $base + '"'), '-CandidatePackage', ('"' + $candidate + '"'), '-Target', ('"' + $target + '"'), '-Mode', $job.mode, '-ReportFile', ('"' + $report + '"'))
            if ($job.previous) { $arguments += @('-PreviousPackage', ('"' + $job.previous + '"')) }
            if ($data) { $arguments += @('-DataSource', ('"' + $data + '"')) }
            $process = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $work "$name.log") -RedirectStandardError (Join-Path $work "$name.errors.log")
            $active.Add(@{ process=$process; name=$name; report=$report })
            Write-Output "Started isolated $name acceptance"
        }
        foreach ($job in @($active.ToArray())) {
            $job.process.Refresh()
            if (-not $job.process.HasExited) { continue }
            $job.process.WaitForExit()
            if ($job.process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $job.report)) {
                throw "Acceptance $($job.name) failed. Logs and marked copies are preserved at $work."
            }
            $completed.Add((Get-Content -LiteralPath $job.report -Raw | ConvertFrom-Json))
            $active.Remove($job) | Out-Null
            Write-Output "Passed isolated $($job.name) acceptance"
        }
        if ($active.Count) { Start-Sleep -Seconds 1 }
    }
} catch {
    # Do not kill recovery processes or remove their copies. Each active fixture completes
    # independently; the parent fails publication immediately.
    throw
}
$manifestHash = (Get-FileHash (Join-Path $candidate 'manifest.json')).Hash.ToLower()
$manifest = Get-Content (Join-Path $candidate 'manifest.json') -Raw | ConvertFrom-Json
foreach ($result in $completed) {
    if ($result.result -ne 'passed' -or $result.manifestSha256 -ne $manifestHash -or $result.version -ne $manifest.version -or $result.managerSource -ne $completed[0].managerSource) { throw 'Acceptance results refer to different source or package bytes.' }
}
$expected = if ($previous) { 10 } else { 5 }
if ($completed.Count -ne $expected) { throw 'Incomplete update acceptance matrix.' }
$requiredCoverage = @('signed-package-corruption','locked-file','occupied-port','python-startup-failure','partial-ddl-invalid-sql','update-process-crash','recovery-process-crash','modified-config-preservation','running-state-recovery','player-data-preservation','disk-exhaustion','write-denial','corrupt-journal','corrupt-recovery-point','database-process-crash','new-server-crash','network-action-containment','all-file-operation-phases')
$missingCoverage = @($requiredCoverage | Where-Object {
    $required = $_
    @($completed | Where-Object { $required -notin $_.coverage }).Count -gt 0
})
$combined = @{ schema=1; result=$(if ($missingCoverage.Count) { 'incomplete' } else { 'passed' }); version=$manifest.version; manifestSha256=$manifestHash; manager=$completed[0].manager; managerSource=$completed[0].managerSource; scenarios=$completed.ToArray(); missingCoverage=$missingCoverage }
$combined | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $candidate 'acceptance.json') -Encoding utf8NoBOM
if ($missingCoverage.Count) { throw "Publication is blocked: release fault coverage is incomplete ($($missingCoverage -join ', ')). Scenario results were preserved." }
Write-Output "Passed all $expected update acceptance scenarios"
