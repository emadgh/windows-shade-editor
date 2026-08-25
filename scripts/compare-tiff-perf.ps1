param(
    [Parameter(Mandatory = $true)]
    [string]$BaselineCsv,

    [Parameter(Mandatory = $true)]
    [string]$CandidateCsv,

    [string]$CsvPath,

    [double]$MinimumMedianMiBPerSec = 0.0,

    [double]$MaxThroughputRegressionPercent = 100.0
)

$ErrorActionPreference = 'Stop'

foreach ($inputPath in @($BaselineCsv, $CandidateCsv)) {
    if (-not (Test-Path -LiteralPath $inputPath -PathType Leaf)) {
        throw "TIFF performance summary not found: $inputPath"
    }
}
if ($MinimumMedianMiBPerSec -lt 0.0) {
    throw 'MinimumMedianMiBPerSec cannot be negative.'
}
if ($MaxThroughputRegressionPercent -lt 0.0) {
    throw 'MaxThroughputRegressionPercent cannot be negative.'
}

function Read-Summary {
    param([string]$Path)

    $map = @{}
    foreach ($row in Import-Csv -LiteralPath $Path) {
        if (-not $row.Operation -or -not $row.Phase) {
            throw "Summary row in $Path is missing Operation or Phase."
        }
        $key = "$($row.Operation)`n$($row.Phase)"
        if ($map.ContainsKey($key)) {
            throw "Duplicate summary row for $($row.Operation)/$($row.Phase) in $Path."
        }
        $map[$key] = $row
    }
    return $map
}

function Parse-OptionalDouble {
    param($Value)
    if ($null -eq $Value -or [string]::IsNullOrWhiteSpace([string]$Value)) {
        return $null
    }
    return [double]$Value
}

function Percent-Change {
    param(
        [Nullable[double]]$Baseline,
        [Nullable[double]]$Candidate
    )
    if ($null -eq $Baseline -or $null -eq $Candidate -or $Baseline -eq 0.0) {
        return $null
    }
    return (($Candidate - $Baseline) / $Baseline) * 100.0
}

$baseline = Read-Summary -Path $BaselineCsv
$candidate = Read-Summary -Path $CandidateCsv
$keys = @($baseline.Keys + $candidate.Keys | Sort-Object -Unique)
$rows = [System.Collections.Generic.List[object]]::new()
$violations = [System.Collections.Generic.List[string]]::new()

foreach ($key in $keys) {
    $base = $baseline[$key]
    $cand = $candidate[$key]
    $parts = $key -split "`n", 2
    $operation = $parts[0]
    $phase = $parts[1]

    if ($null -eq $base) {
        $rows.Add([pscustomobject]@{
            Operation = $operation; Phase = $phase; Status = 'candidate_only'
            BaselineMedianMs = $null; CandidateMedianMs = [double]$cand.MedianMs; MedianMsChangePercent = $null
            BaselineMedianMiBPerSec = $null; CandidateMedianMiBPerSec = Parse-OptionalDouble $cand.MedianMiBPerSec; ThroughputChangePercent = $null
        })
        continue
    }
    if ($null -eq $cand) {
        $rows.Add([pscustomobject]@{
            Operation = $operation; Phase = $phase; Status = 'missing_candidate'
            BaselineMedianMs = [double]$base.MedianMs; CandidateMedianMs = $null; MedianMsChangePercent = $null
            BaselineMedianMiBPerSec = Parse-OptionalDouble $base.MedianMiBPerSec; CandidateMedianMiBPerSec = $null; ThroughputChangePercent = $null
        })
        $violations.Add("Candidate summary is missing $operation/$phase.")
        continue
    }

    $baseMs = [double]$base.MedianMs
    $candMs = [double]$cand.MedianMs
    $baseRate = Parse-OptionalDouble $base.MedianMiBPerSec
    $candRate = Parse-OptionalDouble $cand.MedianMiBPerSec
    $timeChange = Percent-Change $baseMs $candMs
    $rateChange = Percent-Change $baseRate $candRate
    $status = 'ok'

    if ($null -ne $candRate -and $MinimumMedianMiBPerSec -gt 0.0 -and $candRate -lt $MinimumMedianMiBPerSec) {
        $status = 'below_minimum_throughput'
        $violations.Add(("{0}/{1} candidate throughput {2:N2} MiB/s is below required {3:N2} MiB/s." -f $operation, $phase, $candRate, $MinimumMedianMiBPerSec))
    }
    if ($null -ne $rateChange -and $rateChange -lt (-1.0 * $MaxThroughputRegressionPercent)) {
        $status = 'throughput_regression'
        $violations.Add(("{0}/{1} throughput regressed {2:N2}% (allowed {3:N2}%)." -f $operation, $phase, (-1.0 * $rateChange), $MaxThroughputRegressionPercent))
    }

    $rows.Add([pscustomobject]@{
        Operation = $operation
        Phase = $phase
        Status = $status
        BaselineMedianMs = [math]::Round($baseMs, 3)
        CandidateMedianMs = [math]::Round($candMs, 3)
        MedianMsChangePercent = if ($null -ne $timeChange) { [math]::Round($timeChange, 2) } else { $null }
        BaselineMedianMiBPerSec = if ($null -ne $baseRate) { [math]::Round($baseRate, 2) } else { $null }
        CandidateMedianMiBPerSec = if ($null -ne $candRate) { [math]::Round($candRate, 2) } else { $null }
        ThroughputChangePercent = if ($null -ne $rateChange) { [math]::Round($rateChange, 2) } else { $null }
    })
}

$result = @($rows | Sort-Object Operation, Phase)
$result | Format-Table -AutoSize

if ($CsvPath) {
    $parent = Split-Path -Parent $CsvPath
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    $result | Export-Csv -LiteralPath $CsvPath -NoTypeInformation -Encoding UTF8
    Write-Host "Wrote TIFF performance comparison to $CsvPath"
}

if ($violations.Count) {
    throw ("TIFF performance regression gate failed:`n- " + ($violations -join "`n- "))
}
