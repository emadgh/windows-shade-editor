param(
    [ValidateSet('release', 'release-throughput')]
    [string]$Profile = 'release-throughput',

    [ValidateSet('lzw', 'deflate-fast', 'deflate-balanced')]
    [string]$Codec = 'lzw',

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
$codecSource = 'src/lzw_strip_writer.rs'
$compressionTagSources = @(
    'src/source_tiff_writer.rs',
    'src/conversion_tiff.rs'
)
$codecImportPattern = '(?m)^use tiff::encoder::compression::\{CompressionAlgorithm, Lzw\};$'
$codecImportRegex = [regex]::new($codecImportPattern)
$codecCallPattern = 'Lzw\.write_to\('
$codecCallRegex = [regex]::new($codecCallPattern)
$compressionTagPattern = '(?m)^const TIFF_COMPRESSION_LZW: u16 = 5;$'
$compressionTagRegex = [regex]::new($compressionTagPattern)

switch ($Codec) {
    'lzw' {
        $codecImportReplacement = 'use tiff::encoder::compression::{CompressionAlgorithm, Lzw};'
        $codecCallReplacement = 'Lzw.write_to('
        $compressionTagReplacement = 'const TIFF_COMPRESSION_LZW: u16 = 5;'
    }
    'deflate-fast' {
        $codecImportReplacement = 'use tiff::encoder::compression::{CompressionAlgorithm, Deflate, DeflateLevel};'
        $codecCallReplacement = 'Deflate::with_level(DeflateLevel::Fast).write_to('
        $compressionTagReplacement = 'const TIFF_COMPRESSION_LZW: u16 = 8;'
    }
    'deflate-balanced' {
        $codecImportReplacement = 'use tiff::encoder::compression::{CompressionAlgorithm, Deflate, DeflateLevel};'
        $codecCallReplacement = 'Deflate::with_level(DeflateLevel::Balanced).write_to('
        $compressionTagReplacement = 'const TIFF_COMPRESSION_LZW: u16 = 8;'
    }
}

function Resolve-OutputPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Assert-CodecSourceContract {
    param([string]$BaseDirectory)

    $codecPath = Join-Path $BaseDirectory $codecSource
    if (-not (Test-Path -LiteralPath $codecPath -PathType Leaf)) {
        throw "TIFF streaming codec source file not found: $codecPath"
    }
    $codecText = Get-Content -Raw -LiteralPath $codecPath
    $importMatches = $codecImportRegex.Matches($codecText)
    if ($importMatches.Count -ne 1) {
        throw "Expected exactly one benchmarkable LZW compression import in $codecSource; found $($importMatches.Count)."
    }
    $callMatches = $codecCallRegex.Matches($codecText)
    if ($callMatches.Count -ne 2) {
        throw "Expected exactly two benchmarkable LZW strip compression calls in $codecSource; found $($callMatches.Count)."
    }

    foreach ($relativePath in $compressionTagSources) {
        $path = Join-Path $BaseDirectory $relativePath
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "TIFF compression-tag source file not found: $path"
        }
        $text = Get-Content -Raw -LiteralPath $path
        $matches = $compressionTagRegex.Matches($text)
        if ($matches.Count -ne 1) {
            throw "Expected exactly one LZW TIFF compression tag declaration in $relativePath; found $($matches.Count)."
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

Assert-CodecSourceContract -BaseDirectory $root

$plan = [ordered]@{
    profile = $Profile
    target = $targetTriple
    streaming_codec = $Codec
    buffer_policy = 'production-default'
    codec_source = $codecSource
    compression_tag_sources = $compressionTagSources
    codec_import_replacement = $codecImportReplacement
    codec_call_replacement = $codecCallReplacement
    compression_tag_replacement = $compressionTagReplacement
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
    throw 'Cannot resolve the codec benchmark commit SHA.'
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("shade-tiff-codec-" + [guid]::NewGuid().ToString('N'))
$worktree = Join-Path $tempRoot 'worktree'
$previousPerf = $env:SHADE_TIFF_PERF
$previousPerfLog = $env:SHADE_TIFF_PERF_LOG

try {
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
    & git -C $root worktree add --detach $worktree $commit
    if ($LASTEXITCODE -ne 0) {
        throw "Cannot create detached codec benchmark worktree for $commit."
    }

    Assert-CodecSourceContract -BaseDirectory $worktree

    if ($Codec -ne 'lzw') {
        $codecPath = Join-Path $worktree $codecSource
        $codecText = Get-Content -Raw -LiteralPath $codecPath
        $codecPatched = $codecImportRegex.Replace($codecText, $codecImportReplacement, 1)
        $codecPatched = $codecCallRegex.Replace($codecPatched, $codecCallReplacement)
        Set-Content -LiteralPath $codecPath -Value $codecPatched -Encoding UTF8 -NoNewline

        foreach ($relativePath in $compressionTagSources) {
            $path = Join-Path $worktree $relativePath
            $text = Get-Content -Raw -LiteralPath $path
            $patched = $compressionTagRegex.Replace($text, $compressionTagReplacement, 1)
            Set-Content -LiteralPath $path -Value $patched -Encoding UTF8 -NoNewline
        }
    }

    Push-Location $worktree
    try {
        & cargo build --locked --profile $Profile --target $targetTriple
        if ($LASTEXITCODE -ne 0) {
            throw "Cargo codec benchmark build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    $executable = Join-Path $worktree "target/$targetTriple/$profileDirectory/ShadeEditor.exe"
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Codec benchmark executable not found: $executable"
    }
    $exeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash.ToLowerInvariant()

    $env:SHADE_TIFF_PERF = '1'
    Write-Host "Benchmark commit: $commit"
    Write-Host "Profile: $Profile"
    Write-Host "Streaming codec: $Codec"
    Write-Host 'TIFF encoder buffer: production default'
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
        streaming_codec = $Codec
        buffer_policy = 'production-default'
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
    Write-Host "Wrote codec benchmark metadata to $resolvedMetadata"
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
