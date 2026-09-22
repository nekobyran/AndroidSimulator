param(
    [ValidateSet(
        'Doctor', 'BuildEnhancedScrcpy', 'BuildRust', 'BuildRustRelease', 'TestRust', 'BuildAndroidAgent', 'TestWindows', 'BuildWindows',
        'BuildWindowsRelease', 'BuildAll', 'BuildRelease', 'PackageInstaller', 'RunWindows', 'Check', 'CleanBuildCaches',
        'VerifyScrcpy',
        'DownloadAnimeko', 'ProvisionOwnedRuntime', 'EnsureOwnedRuntime', 'OwnedStatus', 'OwnedImageCheck',
        'OwnedLaunchPlan', 'LaunchOwnedRuntime', 'EnableAcceleration',
        'ListApps', 'LaunchApp', 'InstallApk', 'CreateAppShortcut',
        'RegisterApkAssociation', 'ApkAssociationStatus',
        'InputTap', 'InputKeyEvent', 'InputSwipe', 'Smoke'
    )]
    [string]$Action = 'Doctor',

    [string]$Package,
    [string]$AppName,
    [string]$Apk,
    [int]$X = -1,
    [int]$Y = -1,
    [int]$EndX = -1,
    [int]$EndY = -1,
    [int]$DurationMs = 250,
    [string]$Key,
    [int]$DisplayId = -1,
    [ValidateSet('release', 'debug')]
    [string]$InstallerSource = 'release',
    [switch]$SkipInstallerAppBuild
)

$ErrorActionPreference = 'Stop'
if ($DisplayId -lt -1) {
    throw 'DisplayId must be -1 (explicit main display) or a non-negative Android display id.'
}
$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Resolve-Path (Join-Path $ScriptRoot '..')
$WorkspaceRoot = Resolve-Path (Join-Path $ProjectRoot '..\..')
$RustRoot = Join-Path $ProjectRoot 'rust'
$SimulatorCtl = Join-Path $RustRoot 'target\debug\simulatorctl.exe'
$SimulatorCtlRelease = Join-Path $RustRoot 'target\release\simulatorctl.exe'
$AppProject = Join-Path $ProjectRoot 'src\AndroidSimulator.App\AndroidSimulator.App.csproj'
$AppTestsProject = Join-Path $ProjectRoot 'src\AndroidSimulator.App.Tests\AndroidSimulator.App.Tests.csproj'
$AndroidAgentRoot = Join-Path $ProjectRoot 'android-agent'
$ReleaseRoot = Join-Path $WorkspaceRoot 'release\android-simulator_Windows'
$EnhancedScrcpyBuildScript = Join-Path $ScriptRoot 'Build-EnhancedScrcpy.ps1'

# The owner requires one centralized build SDK and cache root.
$SdkRoot = 'D:\vibecoding\sdk'
$declaredSdkRoot = $env:VIBECODING_SDK_ROOT
if ($declaredSdkRoot -and [IO.Path]::GetFullPath($declaredSdkRoot) -ine [IO.Path]::GetFullPath($SdkRoot)) {
    throw "VIBECODING_SDK_ROOT must remain centralized at $SdkRoot"
}
$resolvedSdkRoot = [IO.Path]::GetFullPath($SdkRoot)
if ([IO.Path]::GetPathRoot($resolvedSdkRoot).TrimEnd('\') -ieq $env:SystemDrive) {
    throw "SDK root must not be on the system drive: $resolvedSdkRoot"
}

# Local automation may intentionally start with a credential-stripped
# environment. NuGet's ConfigurationDefaults still requires the standard
# Windows known-folder variables, so restore them only when the host omitted
# them. Normal interactive shells keep their existing values unchanged.
if ([string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
    $env:ProgramFiles = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
}
if ([string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
    ${env:ProgramFiles(x86)} = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFilesX86)
}
if ([string]::IsNullOrWhiteSpace($env:ProgramData)) {
    $env:ProgramData = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
}
if ([string]::IsNullOrWhiteSpace($env:APPDATA)) {
    $env:APPDATA = [Environment]::GetFolderPath([Environment+SpecialFolder]::ApplicationData)
}
if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    $env:LOCALAPPDATA = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
}

$Dotnet = Join-Path $resolvedSdkRoot 'dotnet\dotnet.exe'
$CargoHome = Join-Path $resolvedSdkRoot 'rust\cargo'
$RustupHome = Join-Path $resolvedSdkRoot 'rust\rustup'
$RustToolchainBin = Join-Path $RustupHome 'toolchains\stable-x86_64-pc-windows-msvc\bin'
$Cargo = Join-Path $RustToolchainBin 'cargo.exe'
$AndroidSdk = Join-Path $resolvedSdkRoot 'android'
$JavaHome = Join-Path $resolvedSdkRoot 'jdk'
$CacheRoot = Join-Path $resolvedSdkRoot 'cache'
$TempRoot = Join-Path $CacheRoot 'temp\android-simulator'
$ScrcpyRoot = Join-Path $resolvedSdkRoot 'scrcpy'
$Scrcpy = Join-Path $ScrcpyRoot 'scrcpy.exe'
$ScrcpyArchive = Join-Path $CacheRoot 'scrcpy-win64-v4.1.zip'
$ScrcpyProvisionArchive = Join-Path $resolvedSdkRoot '.downloads\scrcpy\scrcpy-win64-v4.1.zip'
$ScrcpyProvenance = Join-Path $ScrcpyRoot 'provenance.json'
$ScrcpyArchiveSha256 = '5b12172b3264b2889f4583ee64752ce832e29bc8b1089dca81093459697165db'

foreach ($directory in @(
    $CacheRoot,
    $TempRoot,
    (Join-Path $CacheRoot 'nuget'),
    (Join-Path $CacheRoot 'dotnet-home'),
    (Join-Path $CacheRoot 'gradle')
)) {
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
}

$env:ANDROID_HOME = $AndroidSdk
$env:ANDROID_SDK_ROOT = $AndroidSdk
$env:JAVA_HOME = $JavaHome
$env:CARGO_HOME = $CargoHome
$env:RUSTUP_HOME = $RustupHome
$env:RUSTC = Join-Path $RustToolchainBin 'rustc.exe'
$env:RUSTDOC = Join-Path $RustToolchainBin 'rustdoc.exe'
$env:CARGO_TARGET_DIR = Join-Path $RustRoot 'target'
$env:DOTNET_ROOT = Split-Path -Parent $Dotnet
$env:DOTNET_CLI_HOME = Join-Path $CacheRoot 'dotnet-home'
$env:MSBuildEnableWorkloadResolver = 'false'
$env:DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE = 'true'
$env:DOTNET_NOLOGO = 'true'
$env:NUGET_PACKAGES = Join-Path $CacheRoot 'nuget'
$env:GRADLE_USER_HOME = Join-Path $CacheRoot 'gradle'
$env:TEMP = $TempRoot
$env:TMP = $TempRoot
$env:ANDROID_SIMULATOR_CTL = $SimulatorCtl
$env:Path = @(
    (Split-Path -Parent $Dotnet),
    $RustToolchainBin,
    (Join-Path $CargoHome 'bin'),
    (Join-Path $JavaHome 'bin'),
    (Join-Path $AndroidSdk 'platform-tools'),
    (Join-Path $AndroidSdk 'cmdline-tools\latest\bin'),
    $ScrcpyRoot,
    $env:Path
) -join ';'

$Gradle = Get-ChildItem `
    -Path (Join-Path $CacheRoot 'gradle\wrapper\dists\gradle-8.13-all') `
    -Filter 'gradle.bat' `
    -File `
    -Recurse `
    -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\gradle-8\.13\\bin\\gradle\.bat$' } |
    Sort-Object FullName |
    Select-Object -First 1 -ExpandProperty FullName

function Assert-File {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Label)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing under the D: SDK root: $Path"
    }
}

function Assert-Value {
    param([AllowNull()][AllowEmptyString()][string]$Value, [Parameter(Mandatory)][string]$Label)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "$Label is required for -Action $Action"
    }
}

function Invoke-Step {
    param([string]$Name, [scriptblock]$Script)
    Write-Host "== $Name =="
    & $Script
}

function Invoke-SimulatorCtl {
    param([string[]]$Arguments)
    if (-not (Test-Path -LiteralPath $SimulatorCtl)) {
        Invoke-BuildRust
    }

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $SimulatorCtl
    $startInfo.WorkingDirectory = $RustRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.ArgumentList.Add('--json')
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw 'simulatorctl process could not be started.'
        }

        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $stdout = $stdoutTask.GetAwaiter().GetResult().TrimEnd()
        $stderr = $stderrTask.GetAwaiter().GetResult().TrimEnd()
        if ($process.ExitCode -ne 0) {
            $detail = if (-not [string]::IsNullOrWhiteSpace($stderr)) { $stderr } else { $stdout }
            throw "simulatorctl failed with exit code $($process.ExitCode): $detail"
        }
        if (-not [string]::IsNullOrWhiteSpace($stdout)) {
            Write-Output $stdout
        }
    }
    finally {
        $process.Dispose()
    }
}

function Invoke-VerifyScrcpy {
    Assert-File -Path $Scrcpy -Label 'scrcpy 4.1 executable'
    Assert-File -Path $ScrcpyArchive -Label 'official scrcpy 4.1 archive'
    Assert-File -Path $ScrcpyProvisionArchive -Label 'simulatorctl scrcpy provision archive'
    Assert-File -Path $ScrcpyProvenance -Label 'scrcpy verified-install provenance'

    $archiveHash = (Get-FileHash -LiteralPath $ScrcpyArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($archiveHash -cne $ScrcpyArchiveSha256) {
        throw "scrcpy archive SHA256 mismatch: expected $ScrcpyArchiveSha256, got $archiveHash"
    }
    $provisionArchiveHash = (Get-FileHash -LiteralPath $ScrcpyProvisionArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($provisionArchiveHash -cne $ScrcpyArchiveSha256) {
        throw "simulatorctl scrcpy provision archive SHA256 mismatch: expected $ScrcpyArchiveSha256, got $provisionArchiveHash"
    }
    $provenance = Get-Content -LiteralPath $ScrcpyProvenance -Raw -Encoding UTF8 | ConvertFrom-Json
    $executableHash = (Get-FileHash -LiteralPath $Scrcpy -Algorithm SHA256).Hash.ToLowerInvariant()
    if (
        $provenance.schema_version -ne 1 -or
        $provenance.version -cne '4.1' -or
        $provenance.archive_url -cne 'https://github.com/Genymobile/scrcpy/releases/download/v4.1/scrcpy-win64-v4.1.zip' -or
        $provenance.archive_sha256 -cne $ScrcpyArchiveSha256 -or
        $provenance.executable_sha256 -cne $executableHash
    ) {
        throw 'scrcpy provenance does not match the pinned official 4.1 installation.'
    }

    $versionOutput = @(& $Scrcpy --version 2>&1)
    if ($LASTEXITCODE -ne 0 -or ($versionOutput -join "`n") -notmatch '(?m)^scrcpy 4\.1\b') {
        throw "The centralized executable is not scrcpy 4.1: $($versionOutput -join ' ')"
    }

    $helpOutput = @(& $Scrcpy --help 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "scrcpy --help failed with exit code $LASTEXITCODE"
    }
    $helpText = $helpOutput -join "`n"
    $requiredFlags = @('--new-display', '--start-app', '--no-vd-system-decorations')
    $missingFlags = @($requiredFlags | Where-Object { -not $helpText.Contains($_) })
    if ($missingFlags.Count -gt 0) {
        throw "scrcpy 4.1 is missing required capabilities: $($missingFlags -join ', ')"
    }

    [pscustomobject]@{
        ok = $true
        version = '4.1'
        executable = $Scrcpy
        archive = $ScrcpyArchive
        provision_archive = $ScrcpyProvisionArchive
        archive_sha256 = $archiveHash
        executable_sha256 = $executableHash
        provenance = $ScrcpyProvenance
        capabilities = $requiredFlags
        release = 'https://github.com/Genymobile/scrcpy/releases/tag/v4.1'
    } | ConvertTo-Json -Depth 3
}

function Get-ApkIconDllPath {
    param([ValidateSet('debug', 'release')][string]$Profile = 'debug')
    $candidate = Join-Path $RustRoot "target\$Profile\androidsimulator_apkicon.dll"
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
        return $candidate
    }
    return $null
}

function Copy-ApkIconDllBeside {
    param(
        [Parameter(Mandatory)][string]$DestinationDir,
        [ValidateSet('debug', 'release')][string]$Profile = 'debug'
    )
    $source = Get-ApkIconDllPath -Profile $Profile
    if (-not $source) {
        throw "AndroidSimulator.ApkIcon.dll was not produced by the $Profile Rust build."
    }
    $destination = Join-Path $DestinationDir 'AndroidSimulator.ApkIcon.dll'
    Copy-Item -LiteralPath $source -Destination $destination -Force
    $destination
}

function Invoke-BuildEnhancedScrcpy {
    Assert-File -Path $EnhancedScrcpyBuildScript -Label 'Build-EnhancedScrcpy.ps1'
    Invoke-Step 'Build enhanced scrcpy 4.1 client' {
        & $EnhancedScrcpyBuildScript
        if (-not $?) { throw 'Enhanced scrcpy build failed.' }
    }
}
function Invoke-BuildRust {
    Assert-File -Path $Cargo -Label 'Rust cargo'
    Invoke-Step 'Build Rust control layer' {
        Push-Location $RustRoot
        try {
            $process = Start-Process `
                -FilePath $Cargo `
                -ArgumentList @('build', '-p', 'simulatorctl', '-p', 'androidsimulator-apkicon') `
                -NoNewWindow `
                -Wait `
                -PassThru
            if ($process.ExitCode -ne 0) { throw "cargo build failed: $($process.ExitCode)" }
            $hostProcess = Start-Process `
                -FilePath $Cargo `
                -ArgumentList @('build', '-p', 'simulatorctl', '--bin', 'androidsimulator-host', '--release') `
                -NoNewWindow `
                -Wait `
                -PassThru
            if ($hostProcess.ExitCode -ne 0) { throw "native host release build failed: $($hostProcess.ExitCode)" }
        } finally {
            Pop-Location
        }
    }
}

function Invoke-BuildRustRelease {
    Assert-File -Path $Cargo -Label 'Rust cargo'
    Invoke-Step 'Build compact Rust control layer' {
        Push-Location $RustRoot
        try {
            $process = Start-Process `
                -FilePath $Cargo `
                -ArgumentList @('build', '-p', 'simulatorctl', '-p', 'androidsimulator-apkicon', '--release') `
                -NoNewWindow `
                -Wait `
                -PassThru
            if ($process.ExitCode -ne 0) { throw "cargo release build failed: $($process.ExitCode)" }
        } finally {
            Pop-Location
        }
    }
}

function Invoke-TestRust {
    Assert-File -Path $Cargo -Label 'Rust cargo'
    Invoke-Step 'Test Rust control layer' {
        Push-Location $RustRoot
        try {
            $process = Start-Process `
                -FilePath $Cargo `
                -ArgumentList @('test', '-p', 'simulatorctl') `
                -NoNewWindow `
                -Wait `
                -PassThru
            if ($process.ExitCode -ne 0) { throw "cargo test failed: $($process.ExitCode)" }
        } finally {
            Pop-Location
        }
    }
}

function Invoke-BuildAndroidAgent {
    if ([string]::IsNullOrWhiteSpace($Gradle)) {
        throw 'Gradle 8.13 was not found under the centralized D: SDK cache.'
    }
    Assert-File -Path $Gradle -Label 'Gradle 8.13'
    Invoke-Step 'Build Android Simulator agent debug APK' {
        $process = Start-Process `
            -FilePath $Gradle `
            -ArgumentList @('--no-daemon', '--console=plain', ':app:assembleDebug') `
            -WorkingDirectory $AndroidAgentRoot `
            -NoNewWindow `
            -Wait `
            -PassThru
        if ($process.ExitCode -ne 0) { throw "Android agent Gradle build failed: $($process.ExitCode)" }
    }
}

function Invoke-TestWindows {
    Assert-File -Path $Dotnet -Label '.NET SDK host'
    Assert-File -Path $AppTestsProject -Label 'Windows contract tests project'
    Invoke-Step 'Test Windows activation contracts' {
        $process = Start-Process `
            -FilePath $Dotnet `
            -ArgumentList @('run', '--project', ('"' + $AppTestsProject + '"'), '-c', 'Debug') `
            -WorkingDirectory $ProjectRoot `
            -NoNewWindow `
            -Wait `
            -PassThru
        if ($process.ExitCode -ne 0) { throw "Windows contract tests failed: $($process.ExitCode)" }
    }
}

function Reset-SafeDirectory {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$AllowedRoot
    )
    $fullPath = [IO.Path]::GetFullPath($Path)
    $fullRoot = [IO.Path]::GetFullPath($AllowedRoot).TrimEnd('\') + '\'
    if (-not $fullPath.StartsWith($fullRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to reset a directory outside the allowed root: $fullPath"
    }
    if (Test-Path -LiteralPath $fullPath) {
        Remove-Item -LiteralPath $fullPath -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $fullPath | Out-Null
    $fullPath
}

function Export-WindowsBuild {
    param(
        [Parameter(Mandatory)][ValidateSet('Debug', 'Release')][string]$Configuration,
        [Parameter(Mandatory)][string]$Destination
    )
    $binRoot = Join-Path $ProjectRoot "src\AndroidSimulator.App\bin\x64\$Configuration"
    $candidate = Get-ChildItem -Path $binRoot -Recurse -Filter 'AndroidSimulator.App.exe' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $candidate) {
        throw "AndroidSimulator.App.exe was not produced for $Configuration."
    }
    if (-not (Test-Path -LiteralPath (Join-Path $candidate.DirectoryName 'simulatorctl.exe'))) {
        throw "simulatorctl.exe was not packaged beside the $Configuration WinUI executable."
    }
    $profile = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
    Copy-ApkIconDllBeside -DestinationDir $candidate.DirectoryName -Profile $profile | Out-Null

    $destinationPath = [IO.Path]::GetFullPath($Destination)
    $releaseRootPrefix = [IO.Path]::GetFullPath($ReleaseRoot).TrimEnd('\') + '\'
    if (-not $destinationPath.StartsWith($releaseRootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to export outside the release root: $destinationPath"
    }

    $stagingPath = Join-Path $ReleaseRoot ('.export-' + $Configuration.ToLowerInvariant() + '-' + $PID + '-' + [guid]::NewGuid().ToString('N'))
    $backupPath = $destinationPath + '.previous-' + $PID
    $destinationMovedToBackup = $false
    try {
        $stagingPath = Reset-SafeDirectory -Path $stagingPath -AllowedRoot $ReleaseRoot
        Get-ChildItem -LiteralPath $candidate.DirectoryName -Force |
            Copy-Item -Destination $stagingPath -Recurse -Force

        if (-not (Test-Path -LiteralPath (Join-Path $stagingPath 'AndroidSimulator.ApkIcon.dll') -PathType Leaf)) {
            Copy-ApkIconDllBeside -DestinationDir $stagingPath -Profile $profile | Out-Null
        }

        foreach ($requiredFile in @(
            'AndroidSimulator.App.exe',
            'AndroidSimulator.App.dll',
            'AndroidSimulator.App.deps.json',
            'AndroidSimulator.App.runtimeconfig.json',
            'AndroidSimulator.ApkIcon.dll',
            'simulatorctl.exe',
            'coreclr.dll',
            'hostfxr.dll',
            'hostpolicy.dll'
        )) {
            if (-not (Test-Path -LiteralPath (Join-Path $stagingPath $requiredFile) -PathType Leaf)) {
                throw "Incomplete Windows payload: missing $requiredFile"
            }
        }
        foreach ($requiredDirectory in @('Assets', 'zh-CN')) {
            if (-not (Test-Path -LiteralPath (Join-Path $stagingPath $requiredDirectory) -PathType Container)) {
                throw "Incomplete Windows payload: missing directory $requiredDirectory"
            }
        }

        $stagedFiles = @(Get-ChildItem -LiteralPath $stagingPath -Recurse -File)
        if ($stagedFiles.Count -lt 400) {
            throw "Incomplete Windows payload: expected at least 400 files, found $($stagedFiles.Count)"
        }

        if (Test-Path -LiteralPath $backupPath) {
            Remove-Item -LiteralPath $backupPath -Recurse -Force
        }
        if (Test-Path -LiteralPath $destinationPath) {
            Move-Item -LiteralPath $destinationPath -Destination $backupPath
            $destinationMovedToBackup = $true
        }
        Move-Item -LiteralPath $stagingPath -Destination $destinationPath
        if ($destinationMovedToBackup -and (Test-Path -LiteralPath $backupPath)) {
            Remove-Item -LiteralPath $backupPath -Recurse -Force
            $destinationMovedToBackup = $false
        }
    }
    catch {
        if ($destinationMovedToBackup -and -not (Test-Path -LiteralPath $destinationPath) -and (Test-Path -LiteralPath $backupPath)) {
            Move-Item -LiteralPath $backupPath -Destination $destinationPath
            $destinationMovedToBackup = $false
        }
        throw
    }
    finally {
        if (Test-Path -LiteralPath $stagingPath) {
            Remove-Item -LiteralPath $stagingPath -Recurse -Force -ErrorAction SilentlyContinue
        }
        if (-not $destinationMovedToBackup -and (Test-Path -LiteralPath $backupPath)) {
            Remove-Item -LiteralPath $backupPath -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    $files = Get-ChildItem -LiteralPath $destinationPath -Recurse -File
    $report = [pscustomobject]@{
        configuration = $Configuration
        output = $destinationPath
        executable = Join-Path $destinationPath 'AndroidSimulator.App.exe'
        simulatorctl = Join-Path $destinationPath 'simulatorctl.exe'
        apkIconDll = Join-Path $destinationPath 'AndroidSimulator.ApkIcon.dll'
        files = $files.Count
        bytes = ($files | Measure-Object Length -Sum).Sum
        sha256 = (Get-FileHash -Algorithm SHA256 (Join-Path $destinationPath 'AndroidSimulator.App.exe')).Hash
    }
    $report | ConvertTo-Json -Depth 3
}

function Invoke-BuildWindows {
    Invoke-BuildEnhancedScrcpy
    # The WinUI payload embeds simulatorctl.exe. Always refresh the incremental
    # Rust binary so an existing stale executable cannot hide source changes.
    Invoke-BuildRust
    Assert-File -Path $Dotnet -Label '.NET SDK host'
    Invoke-Step 'Build WinUI shell' {
        $process = Start-Process `
            -FilePath $Dotnet `
            -ArgumentList @('build', ('"' + $AppProject + '"'), '-c', 'Debug', '-p:Platform=x64') `
            -WorkingDirectory $ProjectRoot `
            -NoNewWindow `
            -Wait `
            -PassThru
        if ($process.ExitCode -ne 0) { throw "dotnet build failed: $($process.ExitCode)" }
    }
    Export-WindowsBuild -Configuration Debug -Destination (Join-Path $ReleaseRoot 'debug')
}

function Invoke-BuildWindowsRelease {
    if (-not (Test-Path -LiteralPath $SimulatorCtlRelease)) {
        Invoke-BuildRustRelease
    }
    Assert-File -Path $Dotnet -Label '.NET SDK host'
    Invoke-Step 'Publish compact WinUI shell' {
        $process = Start-Process `
            -FilePath $Dotnet `
            -ArgumentList @('publish', ('"' + $AppProject + '"'), '-c', 'Release', '-r', 'win-x64', '--self-contained', 'false', '-p:Platform=x64') `
            -WorkingDirectory $ProjectRoot `
            -NoNewWindow `
            -Wait `
            -PassThru
        if ($process.ExitCode -ne 0) { throw "dotnet publish failed: $($process.ExitCode)" }
    }
    $publishRoot = Get-ChildItem -Path (Join-Path $ProjectRoot 'src\AndroidSimulator.App\bin') -Recurse -Directory -Filter publish -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\Release\\' } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $publishRoot) {
        throw 'The compact WinUI publish directory was not produced.'
    }
    if (-not (Test-Path -LiteralPath (Join-Path $publishRoot.FullName 'simulatorctl.exe'))) {
        throw 'simulatorctl.exe was not packaged in the compact WinUI publish directory.'
    }
    $destinationPath = Reset-SafeDirectory -Path (Join-Path $ReleaseRoot 'release') -AllowedRoot $ReleaseRoot
    Get-ChildItem -LiteralPath $publishRoot.FullName -Force | Copy-Item -Destination $destinationPath -Recurse -Force
    $files = Get-ChildItem -LiteralPath $destinationPath -Recurse -File
    [pscustomobject]@{
        configuration = 'Release'
        output = $destinationPath
        executable = Join-Path $destinationPath 'AndroidSimulator.App.exe'
        simulatorctl = Join-Path $destinationPath 'simulatorctl.exe'
        files = $files.Count
        bytes = ($files | Measure-Object Length -Sum).Sum
        sha256 = (Get-FileHash -Algorithm SHA256 (Join-Path $destinationPath 'AndroidSimulator.App.exe')).Hash
    } | ConvertTo-Json -Depth 3
}

function Invoke-CleanBuildCaches {
    Invoke-Step 'Clean Android Simulator build caches' {
        foreach ($entry in @(
            @{ Path = (Join-Path $RustRoot 'target'); Root = $ProjectRoot },
            @{ Path = (Join-Path $ProjectRoot 'src\AndroidSimulator.App\bin'); Root = $ProjectRoot },
            @{ Path = (Join-Path $ProjectRoot 'src\AndroidSimulator.App\obj'); Root = $ProjectRoot },
            @{ Path = $TempRoot; Root = $resolvedSdkRoot },
            @{ Path = (Join-Path $CacheRoot 'dotnet\parts-10.0.302'); Root = $resolvedSdkRoot }
        )) {
            $fullPath = [IO.Path]::GetFullPath($entry.Path)
            $fullRoot = [IO.Path]::GetFullPath($entry.Root).TrimEnd('\') + '\'
            if (-not $fullPath.StartsWith($fullRoot, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Unsafe cache path: $fullPath"
            }
            if (Test-Path -LiteralPath $fullPath) {
                Remove-Item -LiteralPath $fullPath -Recurse -Force
            }
        }
        foreach ($file in @(
            (Join-Path $CacheRoot 'dotnet\dotnet-sdk-10.0.302-win-x64.zip'),
            (Join-Path $CacheRoot 'dotnet\releases-10.0.json')
        )) {
            if (Test-Path -LiteralPath $file -PathType Leaf) {
                Remove-Item -LiteralPath $file -Force
            }
        }
        [pscustomobject]@{ ok = $true; release_root = $ReleaseRoot } | ConvertTo-Json
    }
}

function Get-AppExe {
    foreach ($candidate in @(
        (Join-Path $ReleaseRoot 'release\AndroidSimulator.App.exe'),
        (Join-Path $ReleaseRoot 'debug\AndroidSimulator.App.exe')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $ReleaseRoot 'debug\AndroidSimulator.App.exe'))) {
        Invoke-BuildWindows
    }
    $candidate = Join-Path $ReleaseRoot 'debug\AndroidSimulator.App.exe'
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        throw 'AndroidSimulator.App.exe was not produced by the WinUI build.'
    }
    $candidate
}

function Get-DebugAppExe {
    $candidate = Join-Path $ReleaseRoot 'debug\AndroidSimulator.App.exe'
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        Invoke-BuildWindows
    }
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        throw 'AndroidSimulator.App.exe was not produced in the debug release directory.'
    }
    $candidate
}

function Invoke-RegisterApkAssociation {
    $appExe = Get-DebugAppExe
    Invoke-SimulatorCtl @('integration', 'register-apk', '--launcher', $appExe)
}

function Invoke-ApkAssociationStatus {
    $appExe = Get-DebugAppExe
    Invoke-SimulatorCtl @('integration', 'status', '--launcher', $appExe)
}

function Invoke-EnableAcceleration {
    Invoke-Step 'Enable Windows hypervisor acceleration' {
        $changes = @()
        $errors = @()
        foreach ($feature in @('HypervisorPlatform', 'VirtualMachinePlatform', 'Microsoft-Hyper-V-All')) {
            $info = & dism.exe /English /Online /Get-FeatureInfo /FeatureName:$feature
            if ($LASTEXITCODE -ne 0) {
                $errors += "failed to query Windows feature $feature"
                continue
            }
            if (-not ($info | Where-Object { $_ -match 'State : Enabled' })) {
                & dism.exe /English /Online /Enable-Feature /FeatureName:$feature /All /NoRestart | Out-Null
                if ($LASTEXITCODE -ne 0) {
                    $errors += "failed to enable Windows feature $feature"
                    continue
                }
                $changes += $feature
            }
        }
        & bcdedit.exe /set hypervisorlaunchtype auto | Out-Null
        [pscustomobject]@{
            ok = $errors.Count -eq 0
            status = if ($errors.Count -eq 0) { 'reboot-required' } else { 'blocked' }
            changed_features = $changes
            errors = $errors
            hypervisorlaunchtype = 'auto'
            reboot_required = $true
        } | ConvertTo-Json -Depth 3
    }
}

switch ($Action) {
    'Doctor' { Invoke-SimulatorCtl @('env', 'doctor') }
    'VerifyScrcpy' { Invoke-VerifyScrcpy }
    'BuildEnhancedScrcpy' { Invoke-BuildEnhancedScrcpy }
    'BuildRust' { Invoke-BuildRust }
    'BuildRustRelease' { Invoke-BuildRustRelease }
    'TestRust' { Invoke-TestRust }
    'BuildAndroidAgent' { Invoke-BuildAndroidAgent }
    'TestWindows' { Invoke-TestWindows }
    'BuildWindows' { Invoke-BuildWindows }
    'BuildWindowsRelease' { Invoke-BuildWindowsRelease }
    'BuildAll' { Invoke-BuildAndroidAgent; Invoke-BuildWindows }
    'BuildRelease' { Invoke-BuildRustRelease; Invoke-BuildWindowsRelease }
    'PackageInstaller' {
        $packageScript = Join-Path $ScriptRoot 'Package-AndroidSimulatorInstaller.ps1'
        Assert-File -Path $packageScript -Label 'Package-AndroidSimulatorInstaller.ps1'
        $packageArgs = @{
            Action = 'Package'
            SourceConfiguration = $InstallerSource
        }
        if ($SkipInstallerAppBuild) {
            $packageArgs.SkipBuild = $true
        }
        elseif (Test-Path -LiteralPath (Join-Path $ReleaseRoot "$InstallerSource\AndroidSimulator.App.exe") -PathType Leaf) {
            # Reuse the latest exported app payload when present.
            $packageArgs.SkipBuild = $true
        }
        & $packageScript @packageArgs
        if (-not $?) { throw 'PackageInstaller failed.' }
    }
    'CleanBuildCaches' { Invoke-CleanBuildCaches }
    'RunWindows' {
        $appExe = Get-AppExe
        Start-Process -FilePath $appExe -WorkingDirectory (Split-Path -Parent $appExe)
    }
    'Check' { Invoke-BuildEnhancedScrcpy; Invoke-VerifyScrcpy; Invoke-TestRust; Invoke-BuildAndroidAgent; Invoke-TestWindows; Invoke-BuildWindows; Invoke-SimulatorCtl @('env', 'doctor') }
    'DownloadAnimeko' { Invoke-SimulatorCtl @('animeko', 'download', '--version', 'v5.6.0') }
    'ProvisionOwnedRuntime' { Invoke-BuildEnhancedScrcpy; Invoke-SimulatorCtl @('owned', 'provision', '--instance', 'android-simulator') }
    'EnsureOwnedRuntime' { Invoke-SimulatorCtl @('owned', 'ensure', '--instance', 'android-simulator') }
    'OwnedStatus' { Invoke-SimulatorCtl @('owned', 'status', '--instance', 'android-simulator') }
    'OwnedImageCheck' { Invoke-SimulatorCtl @('owned', 'image-check', '--instance', 'android-simulator') }
    'OwnedLaunchPlan' { Invoke-SimulatorCtl @('owned', 'launch-plan', '--instance', 'android-simulator') }
    'LaunchOwnedRuntime' { Invoke-SimulatorCtl @('owned', 'launch', '--instance', 'android-simulator') }
    'EnableAcceleration' { Invoke-EnableAcceleration }
    'ListApps' { Invoke-SimulatorCtl @('app', 'list', '--start') }
    'LaunchApp' {
        Assert-Value -Value $Package -Label 'Package'
        $arguments = @('app', 'launch', '--package', $Package)
        if ($AppName) { $arguments += @('--title', $AppName) }
        Invoke-SimulatorCtl $arguments
    }
    'InstallApk' {
        Assert-Value -Value $Apk -Label 'Apk'
        $arguments = @('apk', 'install', '--apk', [IO.Path]::GetFullPath($Apk), '--launch')
        if ($Package) { $arguments += @('--package', $Package) }
        Invoke-SimulatorCtl $arguments
    }
    'CreateAppShortcut' {
        Assert-Value -Value $Package -Label 'Package'
        Assert-Value -Value $AppName -Label 'AppName'
        $appExe = Get-AppExe
        Invoke-SimulatorCtl @('shortcut', 'create', '--package', $Package, '--name', $AppName, '--launcher', $appExe)
    }
    'RegisterApkAssociation' { Invoke-RegisterApkAssociation }
    'ApkAssociationStatus' { Invoke-ApkAssociationStatus }
    'InputTap' {
        if ($X -lt 0 -or $Y -lt 0) { throw 'X and Y must be non-negative.' }
        $arguments = @('input', 'tap', '--x', "$X", '--y', "$Y")
        if ($DisplayId -ge 0) { $arguments += @('--display-id', "$DisplayId") }
        Invoke-SimulatorCtl $arguments
    }
    'InputKeyEvent' {
        Assert-Value -Value $Key -Label 'Key'
        $arguments = @('input', 'keyevent', '--key', $Key)
        if ($DisplayId -ge 0) { $arguments += @('--display-id', "$DisplayId") }
        Invoke-SimulatorCtl $arguments
    }
    'InputSwipe' {
        if (@($X, $Y, $EndX, $EndY) | Where-Object { $_ -lt 0 }) { throw 'X, Y, EndX and EndY must be non-negative.' }
        if ($DurationMs -lt 1 -or $DurationMs -gt 60000) { throw 'DurationMs must be between 1 and 60000.' }
        $arguments = @(
            'input', 'swipe', '--x1', "$X", '--y1', "$Y",
            '--x2', "$EndX", '--y2', "$EndY", '--duration-ms', "$DurationMs"
        )
        if ($DisplayId -ge 0) { $arguments += @('--display-id', "$DisplayId") }
        Invoke-SimulatorCtl $arguments
    }
    'Smoke' {
        Invoke-VerifyScrcpy
        Invoke-TestRust
        Invoke-TestWindows
        Invoke-BuildRust
        Invoke-BuildWindows
        Invoke-RegisterApkAssociation
        Invoke-ApkAssociationStatus
        Invoke-SimulatorCtl @('env', 'doctor')
        Invoke-SimulatorCtl @('owned', 'ensure', '--instance', 'android-simulator')
        Invoke-SimulatorCtl @('owned', 'launch-plan', '--instance', 'android-simulator')
    }
}
