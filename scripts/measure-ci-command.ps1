param(
    [Parameter(Mandatory = $true)]
    [string]$Name,

    [Parameter(Mandatory = $true)]
    [string]$Command
)

$ErrorActionPreference = 'Stop'
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$exitCode = 0
$errorMessage = $null

try {
    Invoke-Expression $Command
    if ($null -ne $LASTEXITCODE) {
        $exitCode = [int]$LASTEXITCODE
    }
} catch {
    $exitCode = 1
    $errorMessage = $_.Exception.Message
} finally {
    $stopwatch.Stop()
    $seconds = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 2)
    $status = if ($exitCode -eq 0 -and -not $errorMessage) { 'success' } else { 'failed' }
    Write-Host "CI timing: $Name = ${seconds}s ($status)"

    if ($env:GITHUB_STEP_SUMMARY) {
        "| $Name | $seconds | $status |" | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Encoding utf8 -Append
    }
}

if ($errorMessage) {
    Write-Error $errorMessage
}

if ($exitCode -ne 0) {
    exit $exitCode
}
