using AndroidSimulator.App.Services;
using System.Diagnostics;
using Windows.Graphics;

var tests = new (string Name, Action Body)[]
{
    ("APK direct launch parses canonical path and background flag", TestApkDirectLaunch),
    ("Package direct launch keeps app name and background flag", TestPackageDirectLaunch),
    ("Package desktop host arguments preserve package and title", TestPackageHostArguments),
    ("Parser rejects conflicting direct launch modes", TestConflictingDirectLaunch),
    ("Adaptive title color rejects insufficient edge samples", TestAdaptiveTitleColorRejectsInsufficientSamples),
    ("Adaptive title color follows dominant app edge instead of icon outliers", TestAdaptiveTitleColorUsesDominantEdge),
    ("Adaptive title color chooses readable foreground", TestAdaptiveTitleColorChoosesReadableForeground),
    ("Hosted app window centers inside the active work area", TestHostedAppWindowCentersInWorkArea),
    ("Hosted app window stays inside a smaller work area", TestHostedAppWindowClampsToWorkArea),
    ("Hosted app scale corrects a DPI-virtualized work area", TestHostedAppScaleCorrectsVirtualizedWorkArea),
    ("Hosted app default size is twenty percent larger", TestHostedAppDefaultSizeIsLarger),
    ("Hosted app viewport fills the physical client area", TestPhysicalHostViewportFillsClientArea),
    ("Warm host channel is stable and package isolated", TestWarmHostChannelIsStableAndIsolated),
    ("Warm host activation still acknowledges and dispatches quickly", TestWarmHostActivationRoundTrip),
    ("Backdrop configuration rejects reentrant activation", TestBackdropConfigurationRejectsReentry),
    ("Package host lease excludes a concurrent cold-start owner", TestPackageHostLeaseExcludesConcurrentOwner),
    ("Hosted window recovery gate prevents reentry and stops after close", TestHostedWindowRecoveryGate),
    ("Host app icon prefers the compact desktop-rounded variant", TestHostAppIconPrefersDesktopRoundedVariant),
    ("Packaged simulatorctl wins over stale workspace debug binaries", TestPackagedSimulatorCtlWins),
};

var failures = new List<string>();
foreach (var test in tests)
{
    try
    {
        test.Body();
        Console.WriteLine($"PASS {test.Name}");
    }
    catch (Exception exception)
    {
        failures.Add($"{test.Name}: {exception.Message}");
        Console.Error.WriteLine($"FAIL {test.Name}: {exception}");
    }
}

if (failures.Count > 0)
{
    Console.Error.WriteLine(string.Join(Environment.NewLine, failures));
    return 1;
}

return 0;


static void TestHostAppIconPrefersDesktopRoundedVariant()
{
    var root = Path.Combine(Path.GetTempPath(), "android-simulator-icon-" + Guid.NewGuid().ToString("N"));
    Directory.CreateDirectory(root);
    try
    {
        var raw = Path.Combine(root, "com.example.reader-abcdef.ico");
        var rounded = Path.Combine(root, "com.example.reader-abcdef-desktop-rounded-0123456789abcdef.ico");
        File.WriteAllBytes(raw, new byte[] { 1, 2, 3, 4, 5, 6, 7, 8 });
        File.WriteAllBytes(rounded, new byte[] { 9, 10, 11 });
        AssertEqual(
            rounded,
            HostAppIcon.ResolveDisplayIconPath(raw),
            "Desktop-rounded icon must win over the multi-megabyte APK dump");
        AssertEqual(
            rounded,
            HostAppIcon.ResolveDisplayIconPath(rounded),
            "Already-rounded icons stay unchanged");
        AssertTrue(HostAppIcon.ResolveDisplayIconPath(Path.Combine(root, "missing.ico")) is null, "Missing icons return null");
    }
    finally
    {
        Directory.Delete(root, recursive: true);
    }
}

static void TestPackagedSimulatorCtlWins()
{
    var root = Path.Combine(
        @"D:\vibecoding\sdk\cache\temp\android-simulator-tests",
        $"simulatorctl-path-{Guid.NewGuid():N}");
    var packagedDirectory = Path.Combine(root, "release", "android-simulator_Windows", "release");
    var packaged = Path.Combine(packagedDirectory, "simulatorctl.exe");
    var workspace = Path.Combine(root, "project", "android-simulator", "rust", "target", "debug", "simulatorctl.exe");

    try
    {
        Directory.CreateDirectory(packagedDirectory);
        Directory.CreateDirectory(Path.GetDirectoryName(workspace)!);
        File.WriteAllText(packaged, "packaged");
        File.WriteAllText(workspace, "stale workspace debug");

        AssertEqual(
            packaged,
            SimulatorCtlPathResolver.Resolve(packagedDirectory, overridePath: null),
            "packaged simulatorctl path");
    }
    finally
    {
        if (Directory.Exists(root))
        {
            Directory.Delete(root, recursive: true);
        }
    }
}

static void TestApkDirectLaunch()
{
    var result = ApkActivationService.ParseDirectLaunch([
        "--apk",
        @"D:\Apps\Reader Debug.apk",
        "--background",
    ]);

    AssertNull(result.Error, "APK parse should not fail");
    AssertNotNull(result.Request, "APK parse should produce a request");
    AssertEqual(Path.GetFullPath(@"D:\Apps\Reader Debug.apk"), result.Request!.ApkPath, "APK path");
    AssertEqual("Reader Debug", result.Request.DisplayName, "APK display name");
    AssertTrue(result.Request.Background, "APK background");
}

static void TestPackageDirectLaunch()
{
    var result = ApkActivationService.ParseDirectLaunch([
        "--package",
        "com.example.reader",
        "--app-name",
        "Reader Desk",
        "--background",
    ]);

    AssertNull(result.Error, "Package parse should not fail");
    AssertEqual("com.example.reader", result.Request!.PackageName, "Package name");
    AssertEqual("Reader Desk", result.Request.DisplayName, "Package display name");
    AssertTrue(result.Request.Background, "Package background");
}

static void TestConflictingDirectLaunch()
{
    var result = ApkActivationService.ParseDirectLaunch([
        "--package",
        "com.example.reader",
        "--apk",
        @"D:\Apps\reader.apk",
    ]);

    AssertEqual("--package 与 --apk 不能同时使用。", result.Error, "Conflict error");
}

static void TestPackageHostArguments()
{
    var arguments = ApkActivationService.BuildPackageHostArguments(
        "com.example.reader",
        "Reader Desk");

    AssertEqual(
        "--package|com.example.reader|--app-name|Reader Desk|--background",
        string.Join('|', arguments),
        "Package host arguments");
}

static void TestAdaptiveTitleColorRejectsInsufficientSamples()
{
    AssertFalse(
        AdaptiveTitleBarColor.TryCreatePalette(
            [0x123456, 0x123456, 0x123456, 0x123456, 0x123456, 0x123456, 0x123456],
            out _),
        "Seven pixels are not a reliable app-edge sample");
}

static void TestAdaptiveTitleColorUsesDominantEdge()
{
    var pixels = Enumerable.Repeat(0x123F68, 96)
        .Concat(Enumerable.Repeat(0xFFFFFF, 12))
        .Concat(Enumerable.Repeat(0x000000, 12))
        .ToArray();

    AssertTrue(
        AdaptiveTitleBarColor.TryCreatePalette(pixels, out var palette),
        "Dominant app-edge sample should produce a palette");
    AssertEqual((byte)0x12, palette.Red, "Dominant edge red");
    AssertEqual((byte)0x3F, palette.Green, "Dominant edge green");
    AssertEqual((byte)0x68, palette.Blue, "Dominant edge blue");
    AssertTrue(palette.UseLightForeground, "Dark app edge should use light title controls");
}

static void TestAdaptiveTitleColorChoosesReadableForeground()
{
    AssertTrue(
        AdaptiveTitleBarColor.TryCreatePalette(
            Enumerable.Repeat(0xF5E7D0, 40).ToArray(),
            out var lightPalette),
        "Light app edge should produce a palette");
    AssertFalse(lightPalette.UseLightForeground, "Light app edge should use dark title controls");
}

static void TestHostedAppWindowCentersInWorkArea()
{
    var bounds = WindowPlacement.CenterInWorkArea(
        new RectInt32(1920, 0, 2560, 1400),
        new SizeInt32(560, 210));

    AssertEqual(new RectInt32(2920, 595, 560, 210), bounds, "Centered hosted-app bounds");
}

static void TestHostedAppWindowClampsToWorkArea()
{
    var bounds = WindowPlacement.CenterInWorkArea(
        new RectInt32(-1280, 0, 1280, 720),
        new SizeInt32(1680, 920));

    AssertEqual(new RectInt32(-1280, 0, 1280, 720), bounds, "Clamped hosted-app bounds");
}

static void TestHostedAppScaleCorrectsVirtualizedWorkArea()
{
    var scale = WindowPlacement.ResolveMonitorScale(
        100,
        new RectInt32(0, 0, 2560, 1400),
        new RectInt32(0, 0, 2048, 1120));

    AssertEqual(1.25, scale, "DPI-virtualized monitor scale");
}

static void TestHostedAppDefaultSizeIsLarger()
{
    AssertEqual(
        new SizeInt32(1440, 842),
        WindowPlacement.ScaleHostSize(1440, 32, 1.0),
        "100% hosted app size");
    AssertEqual(
        new SizeInt32(1800, 1052),
        WindowPlacement.ScaleHostSize(1440, 32, 1.25),
        "125% hosted app size");
}

static void TestPhysicalHostViewportFillsClientArea()
{
    AssertEqual(
        new RectInt32(0, 40, 1800, 1012),
        WindowPlacement.CreatePhysicalHostViewport(1800, 1052, 32, 1.25),
        "Initial physical viewport");
    AssertEqual(
        new RectInt32(0, 40, 2560, 1400),
        WindowPlacement.CreatePhysicalHostViewport(2560, 1440, 32, 1.25),
        "Fullscreen physical viewport");
}

static void TestWarmHostChannelIsStableAndIsolated()
{
    var first = WarmHostActivationService.GetPipeName("com.example.reader");
    AssertEqual(first, WarmHostActivationService.GetPipeName("com.example.reader"), "Stable pipe name");
    AssertFalse(
        first.Equals(
            WarmHostActivationService.GetPipeName("com.example.other"),
            StringComparison.Ordinal),
        "Packages must not share a warm-host channel");
}

static void TestWarmHostActivationRoundTrip()
{
    var packageName = $"com.example.warm.{Guid.NewGuid():N}";
    using var activated = new ManualResetEventSlim();
    using var service = new WarmHostActivationService(packageName, activated.Set);

    var stopwatch = Stopwatch.StartNew();
    AssertTrue(
        WarmHostActivationService.TryActivateExisting(packageName, timeoutMilliseconds: 500),
        "The warm-host pipe must acknowledge SHOW");
    AssertTrue(activated.Wait(TimeSpan.FromSeconds(1)), "The warm host must dispatch activation");
    stopwatch.Stop();
    AssertTrue(
        stopwatch.Elapsed < TimeSpan.FromMilliseconds(250),
        $"Warm activation exceeded 250ms: {stopwatch.Elapsed.TotalMilliseconds:F1}ms");
}

static void TestBackdropConfigurationRejectsReentry()
{
    var gate = new BackdropConfigurationGate();
    AssertTrue(gate.TryEnter(), "First backdrop configuration must enter");
    AssertFalse(gate.TryEnter(), "Reentrant activation must be rejected");
    gate.Exit();
    AssertTrue(gate.TryEnter(), "Backdrop configuration must reopen after exit");
    gate.Exit();
}

static void TestPackageHostLeaseExcludesConcurrentOwner()
{
    var packageName = $"com.example.lease.{Guid.NewGuid():N}";
    AssertTrue(
        PackageHostLease.TryAcquire(packageName, out var primary),
        "The first cold-start host must own the package lease");
    AssertNotNull(primary, "Primary package lease");

    try
    {
        var competingAcquired = Task.Run(() =>
        {
            var acquired = PackageHostLease.TryAcquire(packageName, out var competing);
            competing?.Dispose();
            return acquired;
        }).GetAwaiter().GetResult();
        AssertFalse(competingAcquired, "A concurrent host must not own the same package lease");
    }
    finally
    {
        primary!.Dispose();
    }

    var reacquired = Task.Run(() =>
    {
        var acquired = PackageHostLease.TryAcquire(packageName, out var replacement);
        replacement?.Dispose();
        return acquired;
    }).GetAwaiter().GetResult();
    AssertTrue(reacquired, "The package lease must be available after the host exits");
}

static void TestHostedWindowRecoveryGate()
{
    var gate = new HostedWindowRecoveryGate();

    AssertFalse(gate.TryBegin(isHostedWindowHealthy: true), "A healthy HWND must not trigger recovery");
    AssertTrue(gate.TryBegin(isHostedWindowHealthy: false), "An invalid HWND must trigger recovery");
    AssertFalse(gate.TryBegin(isHostedWindowHealthy: false), "Recovery must not reenter");

    gate.Complete();
    AssertTrue(gate.TryBegin(isHostedWindowHealthy: false), "A later health tick may retry after failure");
    gate.Close();
    gate.Complete();
    AssertFalse(gate.TryBegin(isHostedWindowHealthy: false), "A closed host must never restart recovery");
}

static void AssertTrue(bool condition, string message)
{
    if (!condition)
    {
        throw new InvalidOperationException(message);
    }
}

static void AssertFalse(bool condition, string message)
{
    if (condition)
    {
        throw new InvalidOperationException(message);
    }
}

static void AssertNull(object? value, string message)
{
    if (value is not null)
    {
        throw new InvalidOperationException($"{message}: expected null, got {value}");
    }
}

static void AssertNotNull(object? value, string message)
{
    if (value is null)
    {
        throw new InvalidOperationException($"{message}: expected non-null");
    }
}

static void AssertEqual<T>(T expected, T actual, string message)
{
    if (!EqualityComparer<T>.Default.Equals(expected, actual))
    {
        throw new InvalidOperationException($"{message}: expected {expected}, got {actual}");
    }
}
