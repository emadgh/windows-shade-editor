$ErrorActionPreference = 'Stop'

$runner = Join-Path $PSScriptRoot 'run-tiff-buffer-benchmark.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-buffer-plan-" + [guid]::NewGuid().ToString('N'))

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    $logPath = Join-Path $tempRoot 'buffer-64k.log'
    $warmupPath = Join-Path $tempRoot 'buffer-64k-warmup.log'
    $plan = (& $runner -Profile release-throughput -BufferBytes 65536 -LogPath $logPath -WarmupLogPath $warmupPath -PlanOnly | Out-String | ConvertFrom-Json)
    if ($plan.profile -ne 'release-throughput') {
        throw "Expected release-throughput profile, found $($plan.profile)."
    }
    if ([int]$plan.buffer_bytes -ne 65536) {
        throw "Expected 65536-byte buffer plan, found $($plan.buffer_bytes)."
    }
    if (@($plan.buffer_sources).Count -ne 2) {
        throw "Expected two shared TIFF writer source files, found $(@($plan.buffer_sources).Count)."
    }
    if (-not (@($plan.buffer_sources) -contains 'src/source_tiff_writer.rs')) {
        throw 'Source TIFF writer is missing from the buffer benchmark contract.'
    }
    if (-not (@($plan.buffer_sources) -contains 'src/conversion_tiff.rs')) {
        throw 'Conversion TIFF writer is missing from the buffer benchmark contract.'
    }
    if ($plan.replacement -ne 'const TIFF_ENCODER_BUFFER_BYTES: usize = 65536;') {
        throw "Unexpected buffer replacement text: $($plan.replacement)"
    }
    if ($plan.warmup_log -ne $warmupPath) {
        throw "Unexpected warm-up log path: $($plan.warmup_log)"
    }
    if ($plan.summary -ne [System.IO.Path]::ChangeExtension($logPath, '.summary.csv')) {
        throw "Unexpected buffer summary path: $($plan.summary)"
    }

    $largePlan = (& $runner -Profile release -BufferBytes 4194304 -LogPath (Join-Path $tempRoot 'buffer-4m.log') -PlanOnly | Out-String | ConvertFrom-Json)
    if ([int]$largePlan.buffer_bytes -ne 4194304) {
        throw '4 MiB buffer plan was not preserved.'
    }
    if ($null -ne $largePlan.warmup_log) {
        throw 'Warm-up log should be null when WarmupLogPath is omitted.'
    }

    $rejected = $false
    try {
        & $runner -BufferBytes 4096 -LogPath (Join-Path $tempRoot 'invalid.log') -PlanOnly | Out-Null
    }
    catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw 'Buffer benchmark accepted a value below the 8 KiB safety floor.'
    }

    Write-Host 'TIFF buffer benchmark plan regression test passed.'
}
finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
