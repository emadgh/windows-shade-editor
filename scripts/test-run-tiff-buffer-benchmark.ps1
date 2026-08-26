$ErrorActionPreference = 'Stop'

$bufferRunner = Join-Path $PSScriptRoot 'run-tiff-buffer-benchmark.ps1'
$codecRunner = Join-Path $PSScriptRoot 'run-tiff-codec-benchmark.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-buffer-plan-" + [guid]::NewGuid().ToString('N'))

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    $logPath = Join-Path $tempRoot 'buffer-64k.log'
    $warmupPath = Join-Path $tempRoot 'buffer-64k-warmup.log'
    $plan = (& $bufferRunner -Profile release-throughput -BufferBytes 65536 -LogPath $logPath -WarmupLogPath $warmupPath -PlanOnly | Out-String | ConvertFrom-Json)
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

    $largePlan = (& $bufferRunner -Profile release -BufferBytes 4194304 -LogPath (Join-Path $tempRoot 'buffer-4m.log') -PlanOnly | Out-String | ConvertFrom-Json)
    if ([int]$largePlan.buffer_bytes -ne 4194304) {
        throw '4 MiB buffer plan was not preserved.'
    }
    if ($null -ne $largePlan.warmup_log) {
        throw 'Warm-up log should be null when WarmupLogPath is omitted.'
    }

    $rejected = $false
    try {
        & $bufferRunner -BufferBytes 4096 -LogPath (Join-Path $tempRoot 'invalid.log') -PlanOnly | Out-Null
    }
    catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw 'Buffer benchmark accepted a value below the 8 KiB safety floor.'
    }

    $codecWarmup = Join-Path $tempRoot 'codec-lzw-warmup.log'
    $codecPlan = (& $codecRunner -Profile release-throughput -Codec lzw -LogPath (Join-Path $tempRoot 'codec-lzw.log') -WarmupLogPath $codecWarmup -PlanOnly | Out-String | ConvertFrom-Json)
    if ($codecPlan.streaming_codec -ne 'lzw') {
        throw "Default codec plan must remain LZW, found $($codecPlan.streaming_codec)."
    }
    if ($codecPlan.buffer_policy -ne 'production-default') {
        throw "Codec benchmark must keep production buffer policy, found $($codecPlan.buffer_policy)."
    }
    if ($codecPlan.codec_source -ne 'src/lzw_strip_writer.rs') {
        throw "Unexpected streaming codec source: $($codecPlan.codec_source)"
    }
    if (@($codecPlan.compression_tag_sources).Count -ne 2) {
        throw "Expected two codec tag source files, found $(@($codecPlan.compression_tag_sources).Count)."
    }
    if ($codecPlan.codec_call_replacement -ne 'Lzw.write_to(') {
        throw "Unexpected default codec call: $($codecPlan.codec_call_replacement)"
    }
    if ($codecPlan.compression_tag_replacement -ne 'const TIFF_COMPRESSION_LZW: u16 = 5;') {
        throw "Unexpected default compression tag: $($codecPlan.compression_tag_replacement)"
    }
    if ($codecPlan.warmup_log -ne $codecWarmup) {
        throw "Unexpected codec warm-up log path: $($codecPlan.warmup_log)"
    }

    $fastPlan = (& $codecRunner -Codec deflate-fast -LogPath (Join-Path $tempRoot 'codec-deflate-fast.log') -PlanOnly | Out-String | ConvertFrom-Json)
    if ($fastPlan.streaming_codec -ne 'deflate-fast') {
        throw "Deflate Fast plan lost its codec: $($fastPlan.streaming_codec)"
    }
    if ($fastPlan.codec_import_replacement -ne 'use tiff::encoder::compression::{CompressionAlgorithm, Deflate, DeflateLevel};') {
        throw "Unexpected Deflate import replacement: $($fastPlan.codec_import_replacement)"
    }
    if ($fastPlan.codec_call_replacement -ne 'Deflate::with_level(DeflateLevel::Fast).write_to(') {
        throw "Unexpected Deflate Fast call replacement: $($fastPlan.codec_call_replacement)"
    }
    if ($fastPlan.compression_tag_replacement -ne 'const TIFF_COMPRESSION_LZW: u16 = 8;') {
        throw "Deflate Fast plan did not select TIFF compression tag 8: $($fastPlan.compression_tag_replacement)"
    }

    $balancedPlan = (& $codecRunner -Codec deflate-balanced -LogPath (Join-Path $tempRoot 'codec-deflate-balanced.log') -PlanOnly | Out-String | ConvertFrom-Json)
    if ($balancedPlan.streaming_codec -ne 'deflate-balanced') {
        throw "Deflate Balanced plan lost its codec: $($balancedPlan.streaming_codec)"
    }
    if ($balancedPlan.codec_call_replacement -ne 'Deflate::with_level(DeflateLevel::Balanced).write_to(') {
        throw "Unexpected Deflate Balanced call replacement: $($balancedPlan.codec_call_replacement)"
    }
    if ($balancedPlan.compression_tag_replacement -ne 'const TIFF_COMPRESSION_LZW: u16 = 8;') {
        throw "Deflate Balanced plan did not select TIFF compression tag 8: $($balancedPlan.compression_tag_replacement)"
    }

    Write-Host 'TIFF buffer/codec benchmark plan regression test passed.'
}
finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
