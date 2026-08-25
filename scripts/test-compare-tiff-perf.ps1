$ErrorActionPreference = 'Stop'

$comparer = Join-Path $PSScriptRoot 'compare-tiff-perf.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-perf-compare-test-" + [guid]::NewGuid().ToString('N'))
$baselinePath = Join-Path $tempRoot 'baseline.csv'
$candidatePath = Join-Path $tempRoot 'candidate.csv'
$outputPath = Join-Path $tempRoot 'comparison.csv'

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

    @(
        [pscustomobject]@{ Operation='export'; Phase='total'; Count=5; MedianMs=1000; P95Ms=1100; MedianMiBPerSec=''; LogicalBytes='' }
        [pscustomobject]@{ Operation='export'; Phase='compression_encode'; Count=5; MedianMs=500; P95Ms=550; MedianMiBPerSec=40; LogicalBytes=20971520 }
    ) | Export-Csv -LiteralPath $baselinePath -NoTypeInformation -Encoding UTF8

    @(
        [pscustomobject]@{ Operation='export'; Phase='total'; Count=5; MedianMs=800; P95Ms=900; MedianMiBPerSec=''; LogicalBytes='' }
        [pscustomobject]@{ Operation='export'; Phase='compression_encode'; Count=5; MedianMs=400; P95Ms=450; MedianMiBPerSec=50; LogicalBytes=20971520 }
    ) | Export-Csv -LiteralPath $candidatePath -NoTypeInformation -Encoding UTF8

    & $comparer -BaselineCsv $baselinePath -CandidateCsv $candidatePath -CsvPath $outputPath -MinimumMedianMiBPerSec 10 -MaxThroughputRegressionPercent 20 | Out-Null
    $rows = @(Import-Csv -LiteralPath $outputPath)
    $compression = @($rows | Where-Object { $_.Operation -eq 'export' -and $_.Phase -eq 'compression_encode' })
    if ($compression.Count -ne 1) {
        throw 'Expected one compression comparison row.'
    }
    if ([double]$compression[0].ThroughputChangePercent -ne 25.0) {
        throw "Expected +25% throughput, found $($compression[0].ThroughputChangePercent)."
    }
    if ($compression[0].Status -ne 'ok') {
        throw "Expected successful comparison status, found $($compression[0].Status)."
    }

    $failedMinimum = $false
    try {
        & $comparer -BaselineCsv $baselinePath -CandidateCsv $candidatePath -MinimumMedianMiBPerSec 60 | Out-Null
    }
    catch {
        $failedMinimum = $_.Exception.Message -match 'below required'
    }
    if (-not $failedMinimum) {
        throw 'Minimum throughput gate did not fail for a 50 MiB/s candidate against a 60 MiB/s requirement.'
    }

    @(
        [pscustomobject]@{ Operation='export'; Phase='total'; Count=5; MedianMs=1300; P95Ms=1400; MedianMiBPerSec=''; LogicalBytes='' }
        [pscustomobject]@{ Operation='export'; Phase='compression_encode'; Count=5; MedianMs=800; P95Ms=850; MedianMiBPerSec=20; LogicalBytes=20971520 }
    ) | Export-Csv -LiteralPath $candidatePath -NoTypeInformation -Encoding UTF8

    $failedRegression = $false
    try {
        & $comparer -BaselineCsv $baselinePath -CandidateCsv $candidatePath -MaxThroughputRegressionPercent 20 | Out-Null
    }
    catch {
        $failedRegression = $_.Exception.Message -match 'regressed'
    }
    if (-not $failedRegression) {
        throw 'Throughput regression gate did not fail for a 50% drop with a 20% limit.'
    }

    Write-Host 'TIFF performance comparison regression test passed.'
}
finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
