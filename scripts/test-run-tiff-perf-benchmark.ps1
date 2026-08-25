$ErrorActionPreference = 'Stop'

$runner = Join-Path $PSScriptRoot 'run-tiff-perf-benchmark.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-benchmark-plan-" + [guid]::NewGuid().ToString('N'))

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    $releaseLog = Join-Path $tempRoot 'release.log'
    $releasePlan = (& $runner -Profile release -LogPath $releaseLog -PlanOnly | Out-String | ConvertFrom-Json)
    if ($releasePlan.profile -ne 'release') {
        throw "Expected release profile, found $($releasePlan.profile)."
    }
    if (-not $releasePlan.requires_clean_tracked_worktree) {
        throw 'Acceptance benchmark plan must require a clean tracked worktree.'
    }
    if (-not $releasePlan.executable.EndsWith('target\x86_64-pc-windows-msvc\release\ShadeEditor.exe')) {
        throw "Unexpected release executable path: $($releasePlan.executable)"
    }
    if ($releasePlan.build_args -join ' ' -ne 'build --locked --profile release --target x86_64-pc-windows-msvc') {
        throw "Unexpected release build arguments: $($releasePlan.build_args -join ' ')"
    }
    if ($releasePlan.summary -ne [System.IO.Path]::ChangeExtension($releaseLog, '.summary.csv')) {
        throw "Unexpected derived release summary path: $($releasePlan.summary)"
    }
    if ($releasePlan.metadata -ne "$releaseLog.metadata.json") {
        throw "Unexpected derived release metadata path: $($releasePlan.metadata)"
    }

    $throughputLog = Join-Path $tempRoot 'throughput.log'
    $throughputPlan = (& $runner -Profile release-throughput -LogPath $throughputLog -PlanOnly | Out-String | ConvertFrom-Json)
    if ($throughputPlan.profile -ne 'release-throughput') {
        throw "Expected release-throughput profile, found $($throughputPlan.profile)."
    }
    if (-not $throughputPlan.executable.EndsWith('target\x86_64-pc-windows-msvc\release-throughput\ShadeEditor.exe')) {
        throw "Unexpected throughput executable path: $($throughputPlan.executable)"
    }
    if ($throughputPlan.build_args -join ' ' -ne 'build --locked --profile release-throughput --target x86_64-pc-windows-msvc') {
        throw "Unexpected throughput build arguments: $($throughputPlan.build_args -join ' ')"
    }

    Write-Host 'TIFF benchmark runner plan regression test passed.'
}
finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
