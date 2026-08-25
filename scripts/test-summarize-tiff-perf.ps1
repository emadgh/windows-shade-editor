$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$summarizer = Join-Path $PSScriptRoot 'summarize-tiff-perf.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-perf-test-" + [guid]::NewGuid().ToString('N'))
$logPath = Join-Path $tempRoot 'perf.log'
$csvPath = Join-Path $tempRoot 'summary.csv'

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
    @'
[tiff-perf] operation=export total_ms=1000.000
[tiff-perf] phase=adjustment_render ms=250.000 bytes=10485760 mib_s=40.00
[tiff-perf] phase=compression_encode ms=500.000 bytes=20971520 mib_s=40.00
[tiff-perf] operation=export total_ms=1200.000
[tiff-perf] phase=adjustment_render ms=300.000 bytes=10485760 mib_s=33.33
[tiff-perf] phase=compression_encode ms=600.000 bytes=20971520 mib_s=33.33
[tiff-perf] operation=conversion [tiff-perf] phase=source_identity ms=125.000 bytes=10485760 mib_s=80.00
'@ | Set-Content -LiteralPath $logPath -Encoding UTF8

    & $summarizer -Path $logPath -CsvPath $csvPath | Out-Null
    $rows = @(Import-Csv -LiteralPath $csvPath)

    $exportTotal = @($rows | Where-Object { $_.Operation -eq 'export' -and $_.Phase -eq 'total' })
    if ($exportTotal.Count -ne 1) {
        throw "Expected one grouped export/total row, found $($exportTotal.Count)."
    }
    if ([int]$exportTotal[0].Count -ne 2) {
        throw "Expected two export total samples, found $($exportTotal[0].Count)."
    }
    if ([double]$exportTotal[0].MedianMs -ne 1100.0) {
        throw "Expected export total median 1100 ms, found $($exportTotal[0].MedianMs)."
    }

    $adjustment = @($rows | Where-Object { $_.Operation -eq 'export' -and $_.Phase -eq 'adjustment_render' })
    if ($adjustment.Count -ne 1 -or [int]$adjustment[0].Count -ne 2) {
        throw "Multiline export phases were not associated with their operation."
    }

    $singleLine = @($rows | Where-Object { $_.Operation -eq 'conversion' -and $_.Phase -eq 'source_identity' })
    if ($singleLine.Count -ne 1) {
        throw "Single-line operation/phase records are no longer parsed."
    }
    if ([double]$singleLine[0].MedianMiBPerSec -ne 80.0) {
        throw "Expected conversion source identity rate 80 MiB/s."
    }

    Write-Host 'TIFF performance summarizer regression test passed.'
}
finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
