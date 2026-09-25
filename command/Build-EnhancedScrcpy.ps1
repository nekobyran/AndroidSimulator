param()

$ErrorActionPreference = 'Stop'

$ProjectRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$SdkRoot = 'D:\vibecoding\sdk'
$MsysRoot = Join-Path $SdkRoot 'msys64'
$Bash = Join-Path $MsysRoot 'usr\bin\bash.exe'
$PatchFile = Join-Path $ProjectRoot 'native\scrcpy-enhanced\androidsimulator-scrcpy-4.1.patch'
$PerformancePatchFile = Join-Path $ProjectRoot 'native\scrcpy-enhanced\androidsimulator-scrcpy-4.1-performance.patch'
$CacheRoot = Join-Path $SdkRoot 'cache\android-simulator\scrcpy-enhanced'
$Archive = Join-Path $CacheRoot 'scrcpy-v4.1-source.zip'
$SessionRoot = Join-Path $CacheRoot ("work-{0}-{1}" -f $PID, [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
$SourceRoot = Join-Path $SessionRoot 'src'
$BuildRoot = Join-Path $SessionRoot 'build'
$InstallRoot = Join-Path $SdkRoot 'scrcpy-enhanced'
$Output = Join-Path $InstallRoot 'scrcpy.exe'
$SourceUrl = 'https://github.com/Genymobile/scrcpy/archive/refs/tags/v4.1.zip'

if (-not (Test-Path -LiteralPath $Bash -PathType Leaf)) {
    throw "MSYS2 bash is missing: $Bash"
}
if (-not (Test-Path -LiteralPath $PatchFile -PathType Leaf)) {
    throw "Android Simulator scrcpy patch is missing: $PatchFile"
}
if (-not (Test-Path -LiteralPath $PerformancePatchFile -PathType Leaf)) {
    throw "Android Simulator scrcpy performance patch is missing: $PerformancePatchFile"
}

New-Item -ItemType Directory -Force -Path $CacheRoot, $InstallRoot, $SessionRoot | Out-Null

if (-not (Test-Path -LiteralPath $Archive -PathType Leaf)) {
    Invoke-WebRequest -Uri $SourceUrl -OutFile $Archive
}

$ExtractRoot = Join-Path $SessionRoot 'extract'
Expand-Archive -LiteralPath $Archive -DestinationPath $ExtractRoot -Force
Move-Item -LiteralPath (Join-Path $ExtractRoot 'scrcpy-4.1') -Destination $SourceRoot
Remove-Item -LiteralPath $ExtractRoot -Recurse -Force

$sourceUnix = (& $Bash -lc "cygpath -u '$($SourceRoot.Replace("'", "'\''"))'").Trim()
$buildUnix = (& $Bash -lc "cygpath -u '$($BuildRoot.Replace("'", "'\''"))'").Trim()
$patchUnix = (& $Bash -lc "cygpath -u '$($PatchFile.Replace("'", "'\''"))'").Trim()
$performancePatchUnix = (& $Bash -lc "cygpath -u '$($PerformancePatchFile.Replace("'", "'\''"))'").Trim()

& $Bash -lc "export PATH=/mingw64/bin:/usr/bin; patch -d '$sourceUnix' -p1 < '$patchUnix'"
if ($LASTEXITCODE -ne 0) {
    throw "Failed to apply Android Simulator scrcpy enhancements: $LASTEXITCODE"
}
& $Bash -lc "export PATH=/mingw64/bin:/usr/bin; patch -d '$sourceUnix' -p1 < '$performancePatchUnix'"
if ($LASTEXITCODE -ne 0) {
    throw "Failed to apply Android Simulator scrcpy performance patch: $LASTEXITCODE"
}

& $Bash -lc "export PATH=/mingw64/bin:/usr/bin; cd '$sourceUnix'; meson setup '$buildUnix' --buildtype=release -Dcompile_server=false -Dportable=true -Dusb=false && meson compile -C '$buildUnix'"
if ($LASTEXITCODE -ne 0) {
    throw "Enhanced scrcpy build failed: $LASTEXITCODE"
}

$BuiltExe = Join-Path $BuildRoot 'app\scrcpy.exe'
if (-not (Test-Path -LiteralPath $BuiltExe -PathType Leaf)) {
    throw "Enhanced scrcpy executable was not produced: $BuiltExe"
}
Copy-Item -LiteralPath $BuiltExe -Destination $Output -Force

$OfficialRuntime = Join-Path $SdkRoot 'scrcpy'
if (Test-Path -LiteralPath $OfficialRuntime -PathType Container) {
    $verifyRoot = Join-Path $CacheRoot 'verify-runtime'
    if (Test-Path -LiteralPath $verifyRoot) {
        Remove-Item -LiteralPath $verifyRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $verifyRoot | Out-Null
    Copy-Item -LiteralPath $Output -Destination (Join-Path $verifyRoot 'scrcpy.exe')
    foreach ($name in @(
        'avcodec-62.dll',
        'avformat-62.dll',
        'avutil-60.dll',
        'swresample-6.dll',
        'SDL3.dll',
        'scrcpy-server',
        'disconnected.png',
        'scrcpy.png'
    )) {
        Copy-Item -LiteralPath (Join-Path $OfficialRuntime $name) -Destination (Join-Path $verifyRoot $name)
    }
    & (Join-Path $verifyRoot 'scrcpy.exe') --version
    if ($LASTEXITCODE -ne 0) {
        throw "Enhanced scrcpy runtime verification failed: $LASTEXITCODE"
    }
}

[pscustomobject]@{
    version = '4.1'
    source = $SourceUrl
    patches = @($PatchFile, $PerformancePatchFile)
    output = $Output
} | ConvertTo-Json -Depth 3
