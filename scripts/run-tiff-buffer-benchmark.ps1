param(
    [ValidateSet('release', 'release-throughput')]
    [string]$Profile = 'release-throughput',

    [ValidateRange(8192, 67108864)]
    [int]$BufferBytes = 1048576,

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [string]$WarmupLogPath,
    [string]$SummaryCsv,
    [string]$MetadataPath,
    [switch]$PlanOnly
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$targetTriple = 'x86_64-pc-windows-msvc'
$profileDirectory = if ($Profile -eq 'release') { 'release' } else { $Profile }
$summarizer = Join-Path $PSScriptRoot 'summarize-tiff-perf.ps1'
$bufferSources = @(
    'src/source_tiff_writer_impl.rs',
    'src/conversion_tiff_impl.rs'
)
$bufferPattern = '(?m)^const TIFF_ENCODER_BUFFER_BYTES: usize = [^;]+;$'
$bufferRegex = [regex]::new($bufferPattern)
$replacement = "const TIFF_ENCODER_BUFFER_BYTES: usize = $BufferBytes;"

function Resolve-OutputPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Assert-BufferSourceContract {
    param([string]$BaseDirectory)

    foreach ($relativePath in $bufferSources) {
        $path = Join-Path $BaseDirectory $relativePath
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "TIFF buffer source file not found: $path"
        }
        $text = Get-Content -Raw -LiteralPath $path
        $matches = $bufferRegex.Matches($text)
        if ($matches.Count -ne 1) {
            throw "Expected exactly one TIFF encoder buffer declaration in $relativePath; found $($matches.Count)."
        }
    }
}

function Invoke-ShadeBenchmarkSession {
    param(
        [string]$Executable,
        [string]$PerformanceLog,
        [string]$Label
    )

    $env:SHADE_TIFF_PERF_LOG = $PerformanceLog
    Write-Host "$Label log: $PerformanceLog"
    Write-Host "Run the $Label operation(s) in Shade Editor, then close the application."
    $process = Start-Process -FilePath $Executable -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Shade Editor $Label session exited with code $($process.ExitCode)."
    }
    if (-not (Test-Path -LiteralPath $PerformanceLog -PathType Leaf)) {
        throw "Shade Editor produced no $Label TIFF performance log at $PerformanceLog."
    }
}

$resolvedLog = Resolve-OutputPath $LogPath
$resolvedWarmup = if ($WarmupLogPath) { Resolve-OutputPath $WarmupLogPath } else { $null }
if (-not $SummaryCsv) {
    $SummaryCsv = [System.IO.Path]::ChangeExtension($resolvedLog, '.summary.csv')
}
$resolvedSummary = Resolve-OutputPath $SummaryCsv
if (-not $MetadataPath) {
    $MetadataPath = "$resolvedLog.metadata.json"
}
$resolvedMetadata = Resolve-OutputPath $MetadataPath

Assert-BufferSourceContract -BaseDirectory $root

$plan = [ordered]@{
    profile = $Profile
    target = $targetTriple
    buffer_bytes = $BufferBytes
    buffer_sources = $bufferSources
    replacement = $replacement
    warmup_log = $resolvedWarmup
    log = $resolvedLog
    summary = $resolvedSummary
    metadata = $resolvedMetadata
}

if ($PlanOnly) {
    $plan | ConvertTo-Json -Depth 4
    return
}

$outputPaths = @($resolvedLog, $resolvedSummary, $resolvedMetadata)
if ($resolvedWarmup) {
    $outputPaths += $resolvedWarmup
}
foreach ($path in $outputPaths) {
    $parent = Split-Path -Parent $path
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
}
Remove-Item -LiteralPath $outputPaths -Force -ErrorAction SilentlyContinue

$commit = (& git -C $root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or -not $commit) {
    throw 'Cannot resolve the benchmark commit SHA.'
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-buffer-" + [guid]::NewGuid().ToString('N'))
$worktree = Join-Path $tempRoot 'worktree'
$previousPerf = $env:SHADE_TIFF_PERF
$previousPerfLog = $env:SHADE_TIFF_PERF_LOG

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
    & git -C $root worktree add --detach $worktree $commit
    if ($LASTEXITCODE -ne 0) {
        throw "Cannot create detached benchmark worktree for $commit."
    }

    Assert-BufferSourceContract -BaseDirectory $worktree
    foreach ($relativePath in $bufferSources) {
        $path = Join-Path $worktree $relativePath
        $text = Get-Content -Raw -LiteralPath $path
        $patched = $bufferRegex.Replace($text, $replacement, 1)
        Set-Content -LiteralPath $path -Value $patched -Encoding UTF8 -NoNewline
    }

    Push-Location $worktree
    try {
        & cargo build --locked --profile $Profile --target $targetTriple
        if ($LASTEXITCODE -ne 0) {
            throw "Cargo buffer benchmark build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    $executable = Join-Path $worktree "target/$targetTriple/$profileDirectory/ShadeEditor.exe"
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Buffer benchmark executable not found: $executable"
    }
    $exeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash.ToLowerInvariant()

    $env:SHADE_TIFF_PERF = '1'
    Write-Host "Benchmark commit: $commit"
    Write-Host "Profile: $Profile"
    Write-Host "TIFF encoder buffer: $BufferBytes bytes"
    Write-Host "Executable SHA-256: $exeHash"

    if ($resolvedWarmup) {
        Invoke-ShadeBenchmarkSession -Executable $executable -PerformanceLog $resolvedWarmup -Label 'warm-up'
    }

    $measuredStarted = [DateTimeOffset]::UtcNow
    Invoke-ShadeBenchmarkSession -Executable $executable -PerformanceLog $resolvedLog -Label 'measured'
    $finished = [DateTimeOffset]::UtcNow

    & $summarizer -Path $resolvedLog -CsvPath $resolvedSummary
    if ($LASTEXITCODE -ne 0) {
        throw "TIFF performance summarizer failed with exit code $LASTEXITCODE."
    }

    $metadata = [ordered]@{
        commit = $commit
        profile = $Profile
        target = $targetTriple
        tiff_encoder_buffer_bytes = $BufferBytes
        executable_sha256 = $exeHash
        warmup_log = $resolvedWarmup
        log = $resolvedLog
        summary = $resolvedSummary
        started_utc = $measuredStarted.ToString('o')
        finished_utc = $finished.ToString('o')
        elapsed_seconds = [math]::Round(($finished - $measuredStarted).TotalSeconds, 3)
        isolated_worktree = $true
    }
    $metadata | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $resolvedMetadata -Encoding UTF8
    Write-Host "Wrote buffer benchmark metadata to $resolvedMetadata"
}
finally {
    $env:SHADE_TIFF_PERF = $previousPerf
    $env:SHADE_TIFF_PERF_LOG = $previousPerfLog
    if (Test-Path -LiteralPath $worktree) {
        & git -C $root worktree remove --force $worktree 2>$null
    }
    & git -C $root worktree prune 2>$null
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
