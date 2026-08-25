param(
    [ValidateSet('release', 'release-throughput')]
    [string]$Profile = 'release',

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [string]$SummaryCsv,
    [string]$MetadataPath,
    [switch]$SkipBuild,
    [switch]$PlanOnly
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$targetTriple = 'x86_64-pc-windows-msvc'
$profileDirectory = if ($Profile -eq 'release') { 'release' } else { $Profile }
$executable = Join-Path $root "target/$targetTriple/$profileDirectory/ShadeEditor.exe"
$summarizer = Join-Path $PSScriptRoot 'summarize-tiff-perf.ps1'

function Resolve-OutputPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

$resolvedLog = Resolve-OutputPath $LogPath
if (-not $SummaryCsv) {
    $SummaryCsv = [System.IO.Path]::ChangeExtension($resolvedLog, '.summary.csv')
}
$resolvedSummary = Resolve-OutputPath $SummaryCsv
if (-not $MetadataPath) {
    $MetadataPath = "$resolvedLog.metadata.json"
}
$resolvedMetadata = Resolve-OutputPath $MetadataPath
$buildArgs = @('build', '--locked', '--profile', $Profile, '--target', $targetTriple)

$plan = [ordered]@{
    profile = $Profile
    target = $targetTriple
    executable = $executable
    log = $resolvedLog
    summary = $resolvedSummary
    metadata = $resolvedMetadata
    build_args = $buildArgs
    requires_clean_tracked_worktree = $true
}

if ($PlanOnly) {
    $plan | ConvertTo-Json -Depth 4
    return
}

$commit = (& git -C $root rev-parse HEAD 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or -not $commit) {
    throw 'Cannot resolve the exact benchmark commit SHA. Run the benchmark from a Git checkout.'
}
$trackedChanges = @(& git -C $root status --porcelain --untracked-files=no 2>$null)
if ($LASTEXITCODE -ne 0) {
    throw 'Cannot verify the benchmark working-tree state.'
}
if ($trackedChanges.Count -gt 0) {
    throw 'Benchmark checkout has tracked modifications. Commit/stash them before recording acceptance evidence.'
}

foreach ($path in @($resolvedLog, $resolvedSummary, $resolvedMetadata)) {
    $parent = Split-Path -Parent $path
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
}
Remove-Item -LiteralPath $resolvedLog, $resolvedSummary, $resolvedMetadata -Force -ErrorAction SilentlyContinue

if (-not $SkipBuild) {
    Push-Location $root
    try {
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) {
            throw "Cargo benchmark build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Benchmark executable not found: $executable"
}
if (-not (Test-Path -LiteralPath $summarizer -PathType Leaf)) {
    throw "TIFF performance summarizer not found: $summarizer"
}

$cargoVersion = (& cargo --version).Trim()
if ($LASTEXITCODE -ne 0 -or -not $cargoVersion) {
    throw 'Cannot record the Cargo toolchain identity.'
}
$exeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash.ToLowerInvariant()
$started = [DateTimeOffset]::UtcNow
$previousPerf = $env:SHADE_TIFF_PERF
$previousPerfLog = $env:SHADE_TIFF_PERF_LOG

try {
    $env:SHADE_TIFF_PERF = '1'
    $env:SHADE_TIFF_PERF_LOG = $resolvedLog
    Write-Host "Benchmark profile: $Profile"
    Write-Host "Commit: $commit"
    Write-Host "Executable: $executable"
    Write-Host "Performance log: $resolvedLog"
    Write-Host 'Run the required benchmark operation(s) in Shade Editor, then close the application.'

    $process = Start-Process -FilePath $executable -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Shade Editor exited with code $($process.ExitCode)."
    }
}
finally {
    $env:SHADE_TIFF_PERF = $previousPerf
    $env:SHADE_TIFF_PERF_LOG = $previousPerfLog
}

$finished = [DateTimeOffset]::UtcNow
if (-not (Test-Path -LiteralPath $resolvedLog -PathType Leaf)) {
    throw "Shade Editor produced no TIFF performance log at $resolvedLog."
}

& $summarizer -Path $resolvedLog -CsvPath $resolvedSummary

$metadata = [ordered]@{
    commit = $commit
    tracked_worktree_clean = $true
    profile = $Profile
    target = $targetTriple
    cargo = $cargoVersion
    executable = $executable
    executable_sha256 = $exeHash
    log = $resolvedLog
    summary = $resolvedSummary
    started_utc = $started.ToString('o')
    finished_utc = $finished.ToString('o')
    elapsed_seconds = [math]::Round(($finished - $started).TotalSeconds, 3)
}
$metadata | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $resolvedMetadata -Encoding UTF8
Write-Host "Wrote benchmark metadata to $resolvedMetadata"
