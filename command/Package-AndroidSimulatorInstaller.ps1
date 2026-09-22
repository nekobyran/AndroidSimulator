param(
    [ValidateSet('Package', 'BuildInstallerOnly')]
    [string]$Action = 'Package',

    [ValidateSet('release', 'debug')]
    [string]$SourceConfiguration = 'release',

    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Resolve-Path (Join-Path $ScriptRoot '..')
$WorkspaceRoot = Resolve-Path (Join-Path $ProjectRoot '..\..')
$ReleaseRoot = Join-Path $WorkspaceRoot 'release\android-simulator_Windows'
$SourceDir = Join-Path $ReleaseRoot $SourceConfiguration
$InstallerProject = Join-Path $ProjectRoot 'installer\AndroidSimulator.Installer\AndroidSimulator.Installer.csproj'
$PayloadDir = Join-Path $ProjectRoot 'installer\AndroidSimulator.Installer\Payload'
$PayloadZip = Join-Path $PayloadDir 'app.zip'
$InstallerOutDir = Join-Path $ReleaseRoot 'installer'
$SdkRoot = 'D:\vibecoding\sdk'
$Dotnet = Join-Path $SdkRoot 'dotnet\dotnet.exe'

# Keep packaging functional in credential-stripped automation shells where
# NuGet's standard Windows known-folder variables are intentionally absent.
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
$env:DOTNET_ROOT = Split-Path -Parent $Dotnet
$env:DOTNET_CLI_HOME = Join-Path $SdkRoot 'cache\dotnet-home'
$env:NUGET_PACKAGES = Join-Path $SdkRoot 'cache\nuget'
$env:MSBuildEnableWorkloadResolver = 'false'
$env:DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE = 'true'
$env:DOTNET_NOLOGO = 'true'

function Write-Step([string]$Message) {
    Write-Host "== $Message =="
}

function Assert-File([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label not found: $Path"
    }
}

function Assert-Dir([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "$Label not found: $Path"
    }
}

function New-CleanDirectory([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
}

function Ensure-ReleasePayload {
    Assert-Dir -Path $SourceDir -Label "Release source ($SourceConfiguration)"
    Assert-File -Path (Join-Path $SourceDir 'AndroidSimulator.App.exe') -Label 'AndroidSimulator.App.exe'
    Assert-File -Path (Join-Path $SourceDir 'simulatorctl.exe') -Label 'simulatorctl.exe'

    New-CleanDirectory -Path $PayloadDir
    if (Test-Path -LiteralPath $PayloadZip) {
        Remove-Item -LiteralPath $PayloadZip -Force
    }

    Write-Step "Pack payload from $SourceConfiguration"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory(
        $SourceDir,
        $PayloadZip,
        [System.IO.Compression.CompressionLevel]::Optimal,
        $false)

    $zipInfo = Get-Item -LiteralPath $PayloadZip
    [pscustomobject]@{
        source = $SourceDir
        payload_zip = $PayloadZip
        bytes = $zipInfo.Length
        mb = [math]::Round($zipInfo.Length / 1MB, 2)
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $PayloadZip).Hash
    } | ConvertTo-Json -Depth 3
}

function Invoke-BuildInstaller {
    Assert-File -Path $Dotnet -Label '.NET SDK host'
    Assert-File -Path $InstallerProject -Label 'Installer project'
    Assert-File -Path $PayloadZip -Label 'Installer payload zip'

    New-CleanDirectory -Path $InstallerOutDir
    $publishDir = Join-Path $InstallerOutDir 'publish-temp'
    New-CleanDirectory -Path $publishDir

    Write-Step 'Publish self-contained installer'
    $argumentLine = @(
        'publish',
        ('"' + $InstallerProject + '"'),
        '-c', 'Release',
        '-r', 'win-x64',
        '--self-contained', 'true',
        '-p:PublishSingleFile=true',
        '-p:IncludeNativeLibrariesForSelfExtract=true',
        '-p:EnableCompressionInSingleFile=true',
        '-o', ('"' + $publishDir + '"')
    ) -join ' '
    $process = Start-Process `
        -FilePath $Dotnet `
        -ArgumentList $argumentLine `
        -WorkingDirectory $ProjectRoot `
        -NoNewWindow `
        -Wait `
        -PassThru
    if ($process.ExitCode -ne 0) {
        throw "dotnet publish installer failed: $($process.ExitCode)"
    }

    $setupExe = Join-Path $publishDir 'AndroidSimulator.Setup.exe'
    Assert-File -Path $setupExe -Label 'AndroidSimulator.Setup.exe'

    $finalExe = Join-Path $InstallerOutDir 'AndroidSimulator-Setup.exe'
    Copy-Item -LiteralPath $setupExe -Destination $finalExe -Force

    # Keep a versioned copy for distribution history.
    $versioned = Join-Path $InstallerOutDir 'AndroidSimulator-Setup-1.1.0-win-x64.exe'
    Copy-Item -LiteralPath $setupExe -Destination $versioned -Force

    Remove-Item -LiteralPath $publishDir -Recurse -Force -ErrorAction SilentlyContinue

    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $finalExe).Hash
    $size = (Get-Item -LiteralPath $finalExe).Length
    [pscustomobject]@{
        ok = $true
        source_configuration = $SourceConfiguration
        installer = $finalExe
        versioned = $versioned
        bytes = $size
        mb = [math]::Round($size / 1MB, 2)
        sha256 = $hash
        payload_zip = $PayloadZip
    } | ConvertTo-Json -Depth 4
}

if (-not $SkipBuild -and $Action -eq 'Package') {
    $invoke = Join-Path $ScriptRoot 'Invoke-AndroidSimulator.ps1'
    if ($SourceConfiguration -eq 'release') {
        Write-Step 'Ensure Windows Release artifacts'
        & $invoke -Action BuildWindowsRelease
    }
    else {
        Write-Step 'Ensure Windows Debug artifacts'
        & $invoke -Action BuildWindows
    }
}

switch ($Action) {
    'Package' {
        Ensure-ReleasePayload | Out-Host
        Invoke-BuildInstaller
    }
    'BuildInstallerOnly' {
        Ensure-ReleasePayload | Out-Host
        Invoke-BuildInstaller
    }
}
