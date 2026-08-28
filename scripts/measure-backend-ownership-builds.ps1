param(
    [Parameter(Mandatory = $true)]
    [string]$BaselinePath,

    [Parameter(Mandatory = $true)]
    [string]$CurrentPath,

    [string]$OutputJson = "backend-ownership-build-timings.json"
)

$ErrorActionPreference = 'Stop'
$TargetTriple = 'x86_64-pc-windows-msvc'
$Commands = @(
    [pscustomobject]@{ Name = 'cargo check'; Command = "cargo check --locked --target $TargetTriple" },
    [pscustomobject]@{ Name = 'cargo test --lib'; Command = "cargo test --locked --target $TargetTriple --lib" },
    [pscustomobject]@{ Name = 'cargo test'; Command = "cargo test --locked --target $TargetTriple" },
    [pscustomobject]@{ Name = 'cargo build --release'; Command = "cargo build --release --locked --target $TargetTriple" }
)
$Results = [System.Collections.Generic.List[object]]::new()

function Invoke-CargoChecked {
    param(
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][string]$Command
    )

    Push-Location $WorkingDirectory
    try {
        Invoke-Expression $Command
        if ($LASTEXITCODE -ne 0) {
            throw ("Command failed with exit code {0}: {1}" -f $LASTEXITCODE, $Command)
        }
    } finally {
        Pop-Location
    }
}

function Invoke-TimedCargo {
    param(
        [Parameter(Mandatory = $true)][string]$Variant,
        [Parameter(Mandatory = $true)][string]$Phase,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )

    $Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    Invoke-CargoChecked -WorkingDirectory $WorkingDirectory -Command $Command
    $Stopwatch.Stop()
    $Seconds = [Math]::Round($Stopwatch.Elapsed.TotalSeconds, 2)

    $Result = [pscustomobject]@{
        variant = $Variant
        phase = $Phase
        command = $Name
        seconds = $Seconds
    }
    $script:Results.Add($Result)
    Write-Host "BUILD_MEASUREMENT|$Variant|$Phase|$Name|${Seconds}s"
}

function Reset-BuildArtifacts {
    param([Parameter(Mandatory = $true)][string]$WorkingDirectory)

    Invoke-CargoChecked -WorkingDirectory $WorkingDirectory -Command "cargo clean --target $TargetTriple"
}

function Add-RepresentativeBackendEdit {
    param(
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][string]$RelativeModelPath,
        [Parameter(Mandatory = $true)][string]$Variant
    )

    $Path = Join-Path $WorkingDirectory $RelativeModelPath
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Representative shared model source does not exist for ${Variant}: $Path"
    }
    Add-Content -LiteralPath $Path -Value "`n// issue-396 representative incremental benchmark edit: $Variant"
}

function Measure-Variant {
    param(
        [Parameter(Mandatory = $true)][string]$Variant,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][string]$ModelPath
    )

    $Resolved = (Resolve-Path -LiteralPath $WorkingDirectory).Path
    Write-Host "=== Measuring $Variant at $Resolved ==="

    # One empty target directory precedes the required command sequence. The
    # sequence order matches the project's normal validation progression; later
    # commands may reuse artifacts produced by earlier commands, identically for
    # baseline and current source trees.
    Reset-BuildArtifacts -WorkingDirectory $Resolved
    foreach ($Entry in $Commands) {
        Invoke-TimedCargo -Variant $Variant -Phase 'clean-sequence' -Name $Entry.Name -Command $Entry.Command -WorkingDirectory $Resolved
    }

    # The clean sequence leaves representative debug/release artifacts warm.
    # Mutate the same high-fan-out shared model implementation in each source
    # tree, then measure the same command sequence as an incremental edit.
    Add-RepresentativeBackendEdit -WorkingDirectory $Resolved -RelativeModelPath $ModelPath -Variant $Variant
    foreach ($Entry in $Commands) {
        Invoke-TimedCargo -Variant $Variant -Phase 'incremental-model-edit' -Name $Entry.Name -Command $Entry.Command -WorkingDirectory $Resolved
    }
}

Measure-Variant -Variant 'baseline-pre-396' -WorkingDirectory $BaselinePath -ModelPath 'src/model.rs'
Measure-Variant -Variant 'current-canonical' -WorkingDirectory $CurrentPath -ModelPath 'src/model_impl.rs'

$OutputPath = if ([System.IO.Path]::IsPathRooted($OutputJson)) {
    $OutputJson
} else {
    Join-Path (Get-Location) $OutputJson
}
$Results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8

$Summary = @()
$Summary += '| Variant | Phase | Command | Seconds |'
$Summary += '| --- | --- | --- | ---: |'
foreach ($Result in $Results) {
    $Summary += "| $($Result.variant) | $($Result.phase) | $($Result.command) | $($Result.seconds) |"
}
$SummaryText = $Summary -join "`n"
Write-Host $SummaryText

if ($env:GITHUB_STEP_SUMMARY) {
    "## Issue #396 backend ownership build timings`n" | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
    $SummaryText | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
}

Write-Host "Measurement JSON: $OutputPath"
