$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$rustPaths = @(
    (Join-Path $root 'rust\androidsimulator-core\src\lib.rs')
)
$rustPaths += Get-ChildItem -LiteralPath (Join-Path $root 'rust\simulatorctl\src') -Filter '*.rs' -File |
    Select-Object -ExpandProperty FullName

$managedBridge = Join-Path $root 'src\AndroidSimulator.App\Services\LatencyPriorityService.cs'
$coreSource = Join-Path $root 'rust\androidsimulator-core\src\lib.rs'

foreach ($path in @($rustPaths) + @($managedBridge, $coreSource)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Rust core audit input is missing: $path"
    }
}

$nativeBytes = ($rustPaths | ForEach-Object { (Get-Item -LiteralPath $_).Length } | Measure-Object -Sum).Sum
$managedBridgeBytes = (Get-Item -LiteralPath $managedBridge).Length
$totalBoundaryBytes = $nativeBytes + $managedBridgeBytes
$nativeRatio = if ($totalBoundaryBytes -eq 0) { 0.0 } else { $nativeBytes / $totalBoundaryBytes }

$bridgeText = Get-Content -LiteralPath $managedBridge -Raw
$coreText = Get-Content -LiteralPath $coreSource -Raw

$forbiddenManagedPolicyTokens = @(
    'SetPriorityClass',
    'SetProcessInformation',
    'OpenProcess',
    'GetPriorityClass',
    'PROCESS_POWER_THROTTLING',
    'QemuExecutablePath',
    'QemuPidPath'
)

foreach ($token in $forbiddenManagedPolicyTokens) {
    if ($bridgeText.Contains($token, [System.StringComparison]::Ordinal)) {
        throw "Managed scheduler bridge contains native policy implementation token '$token'."
    }
}

if (-not $bridgeText.Contains('AndroidSimulator.Core.dll', [System.StringComparison]::Ordinal) -or
    -not $bridgeText.Contains('AndroidSimulatorSetCurrentProcessActivity', [System.StringComparison]::Ordinal)) {
    throw 'Managed scheduler bridge must call the Rust native FFI export.'
}

if (-not $coreText.Contains('pub extern "system" fn AndroidSimulatorSetCurrentProcessActivity', [System.StringComparison]::Ordinal)) {
    throw 'Rust core FFI export AndroidSimulatorSetCurrentProcessActivity is missing.'
}

if ($nativeRatio -lt 0.99) {
    throw ("Rust runtime/core boundary ratio is {0:P3}; expected at least 99%." -f $nativeRatio)
}

Write-Host ("Rust runtime/core boundary: {0:P3} native ({1} / {2} bytes)." -f $nativeRatio, $nativeBytes, $totalBoundaryBytes)
Write-Host 'Managed scheduler policy duplication: none detected.'
Write-Host 'Native scheduler FFI boundary: verified.'
