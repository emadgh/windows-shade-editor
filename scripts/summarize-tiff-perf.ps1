param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [string]$CsvPath
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "TIFF performance log not found: $Path"
}

function New-PerfRecord {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Operation,

        [Parameter(Mandatory = $true)]
        [string]$Phase,

        [Parameter(Mandatory = $true)]
        [double]$Ms,

        [Nullable[uint64]]$Bytes,
        [Nullable[double]]$MiBPerSec
    )

    [pscustomobject]@{
        Operation = $Operation
        Phase     = $Phase
        Ms        = $Ms
        Bytes     = $Bytes
        MiBPerSec = $MiBPerSec
    }
}

$records = [System.Collections.Generic.List[object]]::new()
$currentOperation = $null

foreach ($line in Get-Content -LiteralPath $Path) {
    if ($line -match '^\[tiff-perf\] operation=(?<operation>\S+)(?: total_ms=(?<total_ms>[0-9.]+))?') {
        $currentOperation = $Matches.operation
        $totalMs = $Matches.total_ms

        if ($totalMs) {
            $records.Add((New-PerfRecord -Operation $currentOperation -Phase 'total' -Ms ([double]$totalMs)))
        }

        if ($line -match '\[tiff-perf\] phase=(?<phase>\S+) ms=(?<ms>[0-9.]+)(?: bytes=(?<bytes>[0-9]+))?(?: mib_s=(?<mibs>[0-9.]+))?') {
            $bytes = if ($Matches.bytes) { [Nullable[uint64]]([uint64]$Matches.bytes) } else { $null }
            $mibPerSec = if ($Matches.mibs) { [Nullable[double]]([double]$Matches.mibs) } else { $null }
            $records.Add((New-PerfRecord -Operation $currentOperation -Phase $Matches.phase -Ms ([double]$Matches.ms) -Bytes $bytes -MiBPerSec $mibPerSec))
        }
        continue
    }

    if ($currentOperation -and $line -match '^\[tiff-perf\] phase=(?<phase>\S+) ms=(?<ms>[0-9.]+)(?: bytes=(?<bytes>[0-9]+))?(?: mib_s=(?<mibs>[0-9.]+))?') {
        $bytes = if ($Matches.bytes) { [Nullable[uint64]]([uint64]$Matches.bytes) } else { $null }
        $mibPerSec = if ($Matches.mibs) { [Nullable[double]]([double]$Matches.mibs) } else { $null }
        $records.Add((New-PerfRecord -Operation $currentOperation -Phase $Matches.phase -Ms ([double]$Matches.ms) -Bytes $bytes -MiBPerSec $mibPerSec))
    }
}

if (-not $records.Count) {
    throw "No TIFF performance records were found in $Path. Set SHADE_TIFF_PERF_LOG before running Shade Editor."
}

function Get-Percentile {
    param(
        [double[]]$Values,
        [double]$Percentile
    )

    $sorted = @($Values | Sort-Object)
    if ($sorted.Count -eq 1) {
        return $sorted[0]
    }

    $rank = ($Percentile / 100.0) * ($sorted.Count - 1)
    $lower = [math]::Floor($rank)
    $upper = [math]::Ceiling($rank)
    if ($lower -eq $upper) {
        return $sorted[$lower]
    }

    $fraction = $rank - $lower
    return $sorted[$lower] + (($sorted[$upper] - $sorted[$lower]) * $fraction)
}

$summary = foreach ($group in ($records | Group-Object Operation, Phase)) {
    $items = @($group.Group)
    $ms = [double[]]@($items | ForEach-Object { $_.Ms })
    $rates = [double[]]@($items | Where-Object { $null -ne $_.MiBPerSec } | ForEach-Object { $_.MiBPerSec })
    $byteValues = @($items | Where-Object { $null -ne $_.Bytes } | ForEach-Object { $_.Bytes })

    [pscustomobject]@{
        Operation       = $items[0].Operation
        Phase           = $items[0].Phase
        Count           = $items.Count
        MedianMs        = [math]::Round((Get-Percentile -Values $ms -Percentile 50), 3)
        P95Ms           = [math]::Round((Get-Percentile -Values $ms -Percentile 95), 3)
        MedianMiBPerSec = if ($rates.Count) {
            [math]::Round((Get-Percentile -Values $rates -Percentile 50), 2)
        } else {
            $null
        }
        LogicalBytes    = if ($byteValues.Count) { $byteValues[0] } else { $null }
    }
}

$summary = @($summary | Sort-Object Operation, Phase)
$summary | Format-Table -AutoSize

if ($CsvPath) {
    $parent = Split-Path -Parent $CsvPath
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    $summary | Export-Csv -LiteralPath $CsvPath -NoTypeInformation -Encoding UTF8
    Write-Host "Wrote CSV summary to $CsvPath"
}
