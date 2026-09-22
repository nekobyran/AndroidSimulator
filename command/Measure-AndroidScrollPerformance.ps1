[CmdletBinding()]
param(
    [string]$Serial = '127.0.0.1:15555',

    [Parameter(Mandatory)]
    [string]$Package,

    [ValidateRange(0, 4096)]
    [int]$DisplayId = 0,

    [ValidateRange(1, 20)]
    [int]$Rounds = 3,

    [ValidateRange(1, 100)]
    [int]$SwipePairs = 20,

    [ValidateRange(0, 16384)]
    [int]$StartX = 800,

    [ValidateRange(0, 16384)]
    [int]$StartY = 700,

    [ValidateRange(0, 16384)]
    [int]$EndX = 800,

    [ValidateRange(0, 16384)]
    [int]$EndY = 180,

    [ValidateRange(1, 5000)]
    [int]$DurationMs = 400,

    [ValidateRange(0, 5000)]
    [int]$IntervalMs = 150,

    [Parameter(Mandatory)]
    [string]$OutputPath,

    [switch]$DryRun,

    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$FixedOwnedSerial = '127.0.0.1:15555'
$AdbPath = 'D:\vibecoding\sdk\android\platform-tools\adb.exe'
$PackagePattern = '^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+$'

function Assert-SafeInputs {
    if ($Serial -cne $FixedOwnedSerial) {
        throw "Only the owned Android runtime at $FixedOwnedSerial is permitted."
    }
    if ($Package.Length -gt 255 -or $Package -cnotmatch $PackagePattern) {
        throw 'Package must be a canonical Android application id.'
    }
    if ($StartX -eq $EndX -and $StartY -eq $EndY) {
        throw 'The swipe start and end coordinates must differ.'
    }

    $script:ResolvedOutputPath = [IO.Path]::GetFullPath($OutputPath)
    $outputRoot = [IO.Path]::GetPathRoot($script:ResolvedOutputPath)
    if ($outputRoot -ine 'D:\') {
        throw 'OutputPath must be an absolute path on the D: drive.'
    }
    if ([IO.Path]::GetExtension($script:ResolvedOutputPath) -ine '.json') {
        throw 'OutputPath must use the .json extension.'
    }
    $pathSegments = $script:ResolvedOutputPath.Split(
        [IO.Path]::DirectorySeparatorChar,
        [StringSplitOptions]::RemoveEmptyEntries)
    if (-not ($pathSegments | Where-Object { $_ -ieq 'verification' })) {
        throw 'OutputPath must be inside a directory segment named verification.'
    }
    if ((Test-Path -LiteralPath $script:ResolvedOutputPath) -and -not $Force) {
        throw 'OutputPath already exists. Pass -Force to replace this measurement file.'
    }
    if (-not (Test-Path -LiteralPath $AdbPath -PathType Leaf)) {
        throw "The centralized Android SDK adb is missing: $AdbPath"
    }

    $estimatedDurationMs = [int64]$Rounds * $SwipePairs * 2 * ($DurationMs + $IntervalMs)
    if ($estimatedDurationMs -gt 3600000) {
        throw 'The requested gesture plan exceeds the one-hour safety limit.'
    }
    $script:EstimatedDurationMs = $estimatedDurationMs
}

function Invoke-OwnedAdb {
    param(
        [Parameter(Mandatory)]
        [string]$Operation,

        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    # The fixed serial is deliberately inserted here rather than accepting a
    # caller-supplied device argument. This prevents accidental physical-device
    # input even if the host has other ADB transports attached.
    $output = @(& $AdbPath '-s' $FixedOwnedSerial @Arguments 2>&1 |
        ForEach-Object { $_.ToString() })
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "Owned ADB operation '$Operation' failed with exit code $exitCode."
    }
    return $output
}

function ConvertFrom-GfxInfo {
    param([Parameter(Mandatory)][string]$Text)

    $options = [Text.RegularExpressions.RegexOptions]::Multiline
    $metricMatches = [ordered]@{
        totalFrames = [regex]::Matches($Text, '^\s*Total frames rendered:\s*(\d+)\s*$', $options)
        jankyFrames = [regex]::Matches(
            $Text,
            '^\s*Janky frames:\s*(\d+)\s*\(([0-9]+(?:\.[0-9]+)?)%\)\s*$',
            $options)
        p50Ms = [regex]::Matches($Text, '^\s*50th percentile:\s*(\d+)ms\s*$', $options)
        p90Ms = [regex]::Matches($Text, '^\s*90th percentile:\s*(\d+)ms\s*$', $options)
        p95Ms = [regex]::Matches($Text, '^\s*95th percentile:\s*(\d+)ms\s*$', $options)
        p99Ms = [regex]::Matches($Text, '^\s*99th percentile:\s*(\d+)ms\s*$', $options)
        missedVsync = [regex]::Matches($Text, '^\s*Number Missed Vsync:\s*(\d+)\s*$', $options)
    }
    $profileCount = $metricMatches.totalFrames.Count
    if ($profileCount -eq 0) {
        throw "gfxinfo did not contain a 'Total frames rendered' metric."
    }
    foreach ($entry in $metricMatches.GetEnumerator()) {
        if ($entry.Value.Count -ne $profileCount) {
            throw "gfxinfo metric '$($entry.Key)' had $($entry.Value.Count) profiles; expected $profileCount."
        }
    }

    $profiles = @()
    for ($index = 0; $index -lt $profileCount; $index++) {
        $profiles += [pscustomobject][ordered]@{
            index = $index
            totalFrames = [int64]$metricMatches.totalFrames[$index].Groups[1].Value
            jankyFrames = [int64]$metricMatches.jankyFrames[$index].Groups[1].Value
            reportedJankyPercent = [double]::Parse(
                $metricMatches.jankyFrames[$index].Groups[2].Value,
                [Globalization.CultureInfo]::InvariantCulture)
            p50Ms = [int]$metricMatches.p50Ms[$index].Groups[1].Value
            p90Ms = [int]$metricMatches.p90Ms[$index].Groups[1].Value
            p95Ms = [int]$metricMatches.p95Ms[$index].Groups[1].Value
            p99Ms = [int]$metricMatches.p99Ms[$index].Groups[1].Value
            missedVsync = [int64]$metricMatches.missedVsync[$index].Groups[1].Value
        }
    }

    # Android prints the canonical package summary first. Later renderer/window
    # profiles are diagnostic detail and may overlap that package total, so they
    # must never be added to the aggregate metrics.
    $canonical = $profiles[0]

    [pscustomobject][ordered]@{
        totalFrames = $canonical.totalFrames
        jankyFrames = $canonical.jankyFrames
        jankyPercent = $canonical.reportedJankyPercent
        p50Ms = $canonical.p50Ms
        p90Ms = $canonical.p90Ms
        p95Ms = $canonical.p95Ms
        p99Ms = $canonical.p99Ms
        missedVsync = $canonical.missedVsync
        rendererProfileCount = $profileCount
        canonicalProfileIndex = 0
        rendererProfiles = $profiles
    }
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$Text)

    $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
    $hash = [Security.Cryptography.SHA256]::HashData($bytes)
    return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Get-Median {
    param([Parameter(Mandatory)][double[]]$Values)

    if ($Values.Count -eq 0) {
        return $null
    }
    $sorted = @($Values | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    if (($sorted.Count % 2) -eq 1) {
        return $sorted[$middle]
    }
    return ($sorted[$middle - 1] + $sorted[$middle]) / 2.0
}

function Write-Utf8JsonAtomic {
    param(
        [Parameter(Mandatory)][object]$Value,
        [Parameter(Mandatory)][string]$Path
    )

    $directory = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $temporaryPath = Join-Path $directory ('.' + [IO.Path]::GetFileName($Path) + ".${PID}.tmp")
    try {
        $json = $Value | ConvertTo-Json -Depth 8
        [IO.File]::WriteAllText(
            $temporaryPath,
            $json + [Environment]::NewLine,
            [Text.UTF8Encoding]::new($false))
        Move-Item -LiteralPath $temporaryPath -Destination $Path -Force:$Force
    }
    finally {
        if (Test-Path -LiteralPath $temporaryPath) {
            Remove-Item -LiteralPath $temporaryPath -Force
        }
    }
}

Assert-SafeInputs

$configuration = [pscustomobject][ordered]@{
    serial = $FixedOwnedSerial
    package = $Package
    displayId = $DisplayId
    rounds = $Rounds
    swipePairsPerRound = $SwipePairs
    upwardSwipe = [pscustomobject][ordered]@{
        x1 = $StartX
        y1 = $StartY
        x2 = $EndX
        y2 = $EndY
        durationMs = $DurationMs
    }
    downwardSwipe = [pscustomobject][ordered]@{
        x1 = $EndX
        y1 = $EndY
        x2 = $StartX
        y2 = $StartY
        durationMs = $DurationMs
    }
    intervalMs = $IntervalMs
    estimatedGestureDurationMs = $EstimatedDurationMs
}

if ($DryRun) {
    $report = [pscustomobject][ordered]@{
        schemaVersion = 1
        status = 'dry-run'
        generatedAt = [DateTimeOffset]::Now.ToString('o')
        configuration = $configuration
        plannedGestures = $Rounds * $SwipePairs * 2
        measurements = @()
        aggregateMedian = $null
    }
    Write-Utf8JsonAtomic -Value $report -Path $ResolvedOutputPath
    Write-Output $ResolvedOutputPath
    return
}

$deviceState = (Invoke-OwnedAdb -Operation 'get-state' -Arguments @('get-state')) -join "`n"
if ($deviceState.Trim() -cne 'device') {
    throw "The owned Android runtime is not online."
}

$measurements = @()
for ($round = 1; $round -le $Rounds; $round++) {
    Invoke-OwnedAdb -Operation "gfxinfo reset round $round" -Arguments @(
        'shell', 'dumpsys', 'gfxinfo', $Package, 'reset') | Out-Null

    $startedAt = [DateTimeOffset]::Now
    for ($pair = 1; $pair -le $SwipePairs; $pair++) {
        Invoke-OwnedAdb -Operation "upward swipe round $round pair $pair" -Arguments @(
            'shell', 'input', '-d', "$DisplayId", 'swipe',
            "$StartX", "$StartY", "$EndX", "$EndY", "$DurationMs") | Out-Null
        if ($IntervalMs -gt 0) {
            Start-Sleep -Milliseconds $IntervalMs
        }

        Invoke-OwnedAdb -Operation "downward swipe round $round pair $pair" -Arguments @(
            'shell', 'input', '-d', "$DisplayId", 'swipe',
            "$EndX", "$EndY", "$StartX", "$StartY", "$DurationMs") | Out-Null
        if ($IntervalMs -gt 0) {
            Start-Sleep -Milliseconds $IntervalMs
        }
    }

    $rawLines = Invoke-OwnedAdb -Operation "gfxinfo framestats round $round" -Arguments @(
        'shell', 'dumpsys', 'gfxinfo', $Package, 'framestats')
    $rawText = $rawLines -join "`n"
    $metrics = ConvertFrom-GfxInfo -Text $rawText
    $measurements += [pscustomobject][ordered]@{
        round = $round
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTimeOffset]::Now.ToString('o')
        metrics = $metrics
        rawGfxInfoSha256 = Get-Sha256 -Text $rawText
        rawGfxInfoLineCount = $rawLines.Count
    }
}

$aggregateMedian = [pscustomobject][ordered]@{
    totalFrames = Get-Median -Values @($measurements.metrics.totalFrames)
    jankyFrames = Get-Median -Values @($measurements.metrics.jankyFrames)
    jankyPercent = Get-Median -Values @($measurements.metrics.jankyPercent)
    p50Ms = Get-Median -Values @($measurements.metrics.p50Ms)
    p90Ms = Get-Median -Values @($measurements.metrics.p90Ms)
    p95Ms = Get-Median -Values @($measurements.metrics.p95Ms)
    p99Ms = Get-Median -Values @($measurements.metrics.p99Ms)
    missedVsync = Get-Median -Values @($measurements.metrics.missedVsync)
}

$report = [pscustomobject][ordered]@{
    schemaVersion = 1
    status = 'completed'
    generatedAt = [DateTimeOffset]::Now.ToString('o')
    configuration = $configuration
    measurements = $measurements
    aggregateMedian = $aggregateMedian
}
Write-Utf8JsonAtomic -Value $report -Path $ResolvedOutputPath
Write-Output $ResolvedOutputPath
