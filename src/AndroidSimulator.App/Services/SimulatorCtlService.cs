using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;
using AndroidSimulator.App.Models;

namespace AndroidSimulator.App.Services;

public sealed record SimulatorCommandResult<T>(
    bool Success,
    string Status,
    string Message,
    string Output,
    int ExitCode,
    T Data);

public static class SimulatorCtlService
{
    private const string EnvKey = "ANDROID_SIMULATOR_CTL";
    private const string AndroidHomePackage = "com.android.launcher3";
    private const string AndroidHomeTitle = "Android()";
    // simulatorctl writes its complete JSON before exiting. Detached scrcpy/adb
    // descendants may keep inherited pipe handles alive, so never add a full
    // second of artificial latency after the control process has completed.
    private static readonly TimeSpan OutputDrainTimeout = TimeSpan.FromMilliseconds(15);
    private static readonly object AppListCacheSync = new();
    private static readonly SemaphoreSlim AppListCacheFileGate = new(1, 1);
    private static readonly JsonSerializerOptions AppListCacheJsonOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
    };
    private static readonly string AppListCachePath = Path.Combine(
        @"D:\vibecoding\sdk\android-simulator-runtime",
        "cache",
        "app-list-ui.json");
    private static IReadOnlyList<AndroidAppInfo>? _cachedApps;

    public static string ResolveSimulatorCtl()
        => SimulatorCtlPathResolver.Resolve(
            AppContext.BaseDirectory,
            Environment.GetEnvironmentVariable(EnvKey));

    public static Task<SimulatorCommandResult<JsonElement?>> RunAsync(params string[] args) =>
        RunAsync(CancellationToken.None, args);

    public static async Task<SimulatorCommandResult<JsonElement?>> RunAsync(
        CancellationToken cancellationToken,
        params string[] args)
    {
        var simulatorCtl = ResolveSimulatorCtl();
        if (!File.Exists(simulatorCtl))
        {
            return Failure(
                "missing",
                $"simulatorctl.exe was not found. Build the Rust control layer first: {simulatorCtl}");
        }

        var startInfo = new ProcessStartInfo
        {
            FileName = simulatorCtl,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        startInfo.ArgumentList.Add("--json");
        foreach (var arg in args)
        {
            startInfo.ArgumentList.Add(arg);
        }

        try
        {
            using var process = Process.Start(startInfo);
            if (process is null)
            {
                return Failure("error", "Failed to start simulatorctl.");
            }

            try
            {
                // A shortcut may be launched by an efficiency-mode parent. Keep the
                // short-lived control process responsive while it wakes QEMU and ADB.
                process.PriorityClass = ProcessPriorityClass.Normal;
            }
            catch
            {
                // Priority is best effort; command execution still remains available.
            }

            var stdout = new StringBuilder();
            var stderr = new StringBuilder();
            using var outputCancellation = CancellationTokenSource.CreateLinkedTokenSource(
                cancellationToken);
            var stdoutTask = ReadProcessOutputAsync(
                process.StandardOutput,
                stdout,
                outputCancellation.Token);
            var stderrTask = ReadProcessOutputAsync(
                process.StandardError,
                stderr,
                outputCancellation.Token);
            try
            {
                await process.WaitForExitAsync(cancellationToken);
            }
            catch (OperationCanceledException)
            {
                TryTerminate(process);
                throw;
            }

            try
            {
                await Task.WhenAll(stdoutTask, stderrTask)
                    .WaitAsync(OutputDrainTimeout, cancellationToken);
            }
            catch (TimeoutException)
            {
                // A detached runtime child must never keep the launcher's anonymous
                // simulatorctl pipes open. The control process has already exited and
                // its JSON has been consumed incrementally, so close only our readers
                // instead of waiting for an unrelated long-lived child process.
                outputCancellation.Cancel();
                process.StandardOutput.Close();
                process.StandardError.Close();
                await IgnoreExpectedReaderShutdownAsync(stdoutTask, stderrTask);
            }

            var stdoutText = stdout.ToString();
            var stderrText = stderr.ToString();
            var envelope = ParseEnvelope(stdoutText);
            var output = FormatOutput(stdoutText, stderrText);
            return new SimulatorCommandResult<JsonElement?>(
                process.ExitCode == 0 && envelope.Ok,
                envelope.Status,
                envelope.Message,
                output,
                process.ExitCode,
                envelope.Data);
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception exception)
        {
            return Failure("error", $"Failed to run simulatorctl: {exception.Message}");
        }
    }

    public static async Task<IReadOnlyList<AndroidAppInfo>> ReadCachedAppsAsync(
        CancellationToken cancellationToken = default)
    {
        lock (AppListCacheSync)
        {
            if (_cachedApps is not null)
            {
                return _cachedApps;
            }
        }

        await AppListCacheFileGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            lock (AppListCacheSync)
            {
                if (_cachedApps is not null)
                {
                    return _cachedApps;
                }
            }

            if (!File.Exists(AppListCachePath))
            {
                return Array.Empty<AndroidAppInfo>();
            }

            var json = await File.ReadAllTextAsync(AppListCachePath, cancellationToken)
                .ConfigureAwait(false);
            var cache = JsonSerializer.Deserialize<CachedAppList>(json, AppListCacheJsonOptions);
            if (cache is null || cache.SchemaVersion != 1 || cache.Apps is null)
            {
                return Array.Empty<AndroidAppInfo>();
            }

            var apps = cache.Apps
                .Where(item => !string.IsNullOrWhiteSpace(item.PackageName))
                .Select(CreateCachedApp)
                .ToArray();
            lock (AppListCacheSync)
            {
                _cachedApps = apps;
            }
            return apps;
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch
        {
            return Array.Empty<AndroidAppInfo>();
        }
        finally
        {
            AppListCacheFileGate.Release();
        }
    }

    public static Task<SimulatorCommandResult<IReadOnlyList<AndroidAppInfo>>> ListAppsAsync(
        CancellationToken cancellationToken = default)
        => ListAppsAsync(startRuntime: false, forceRefresh: false, cancellationToken);

    public static Task<SimulatorCommandResult<IReadOnlyList<AndroidAppInfo>>> ListAppsAsync(
        bool startRuntime,
        CancellationToken cancellationToken = default)
        => ListAppsAsync(startRuntime, forceRefresh: false, cancellationToken);

    public static async Task<SimulatorCommandResult<IReadOnlyList<AndroidAppInfo>>> ListAppsAsync(bool startRuntime, bool forceRefresh, CancellationToken cancellationToken = default)
    {
        if (!forceRefresh)
        {
            var cachedApps = await ReadCachedAppsAsync(cancellationToken).ConfigureAwait(false);
            if (cachedApps.Count > 0)
            {
                return new SimulatorCommandResult<IReadOnlyList<AndroidAppInfo>>(
                    true,
                    "cached",
                    "Cached application list loaded.",
                    string.Empty,
                    0,
                    cachedApps);
            }
        }

        var args = startRuntime
            ? new[] { "app", "list", "--start" }
            : new[] { "app", "list" };
        var result = await RunAsync(cancellationToken, args).ConfigureAwait(false);
        if (!result.Success || result.Data is not JsonElement data)
        {
            return Project(result, (IReadOnlyList<AndroidAppInfo>)Array.Empty<AndroidAppInfo>());
        }

        if (!data.TryGetProperty("apps", out var appsElement)
            || appsElement.ValueKind != JsonValueKind.Array)
        {
            return new SimulatorCommandResult<IReadOnlyList<AndroidAppInfo>>(
                false,
                "unparsed",
                "simulatorctl app list returned an invalid data payload.",
                result.Output,
                result.ExitCode,
                Array.Empty<AndroidAppInfo>());
        }

        var apps = new List<AndroidAppInfo>(appsElement.GetArrayLength());
        foreach (var appElement in appsElement.EnumerateArray())
        {
            if (appElement.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            var packageName = GetString(appElement, "package");
            if (string.IsNullOrWhiteSpace(packageName))
            {
                continue;
            }

            var name = GetString(appElement, "label");
            var iconPath = GetString(appElement, "icon_path");
            if (string.IsNullOrWhiteSpace(iconPath))
            {
                iconPath = GetString(appElement, "iconPath");
            }

            var version = GetString(appElement, "version");
            string? resolvedIconPath = null;
            if (!string.IsNullOrWhiteSpace(iconPath))
            {
                try
                {
                    var fullPath = Path.GetFullPath(iconPath);
                    if (File.Exists(fullPath))
                    {
                        resolvedIconPath = fullPath;
                    }
                }
                catch
                {
                    resolvedIconPath = null;
                }
            }

            apps.Add(new AndroidAppInfo
            {
                PackageName = packageName,
                Name = string.IsNullOrWhiteSpace(name) ? packageName : name,
                ActivityName = GetString(appElement, "activity"),
                Version = version,
                IconPath = resolvedIconPath,
                HasIcon = resolvedIconPath is not null,
            });
        }

        UpdateAppListCache(apps);
        return Project(result, (IReadOnlyList<AndroidAppInfo>)apps);
    }

    public static Task<SimulatorCommandResult<JsonElement?>> LaunchAppAsync(
        string packageName,
        string title,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(packageName))
        {
            return Task.FromResult(Failure("invalid", "Package name is required."));
        }

        if (string.IsNullOrWhiteSpace(title))
        {
            return Task.FromResult(Failure("invalid", "Application title is required."));
        }

        var args = new List<string>
        {
            "app",
            "launch",
            "--package",
            packageName.Trim(),
            "--title",
            title.Trim(),
        };
        AddAndroidHomeIconArgument(args, packageName, title);
        return RunAsync(cancellationToken, args.ToArray());
    }

    public static Task<SimulatorCommandResult<JsonElement?>> StopAppAsync(
        string packageName,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(packageName))
        {
            return Task.FromResult(Failure("invalid", "Package name is required."));
        }

        return RunAsync(
            cancellationToken,
            "app",
            "stop",
            "--package",
            packageName.Trim());
    }

    public static Task<SimulatorCommandResult<JsonElement?>> InstallApkAsync(
        string apkPath,
        bool launch,
        string? title,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(apkPath))
        {
            return Task.FromResult(Failure("invalid", "APK path is required."));
        }

        string fullPath;
        try
        {
            fullPath = Path.GetFullPath(apkPath);
        }
        catch (Exception exception) when (exception is ArgumentException or NotSupportedException or PathTooLongException)
        {
            return Task.FromResult(Failure("invalid", $"APK path is invalid: {exception.Message}"));
        }

        if (!File.Exists(fullPath))
        {
            return Task.FromResult(Failure("missing", $"APK file was not found: {fullPath}"));
        }

        var args = new List<string> { "apk", "install", "--apk", fullPath };
        if (launch)
        {
            args.Add("--launch");
            if (!string.IsNullOrWhiteSpace(title))
            {
                args.Add("--title");
                args.Add(title.Trim());
            }
        }

        return RunAsync(cancellationToken, args.ToArray());
    }

    public static Task<SimulatorCommandResult<JsonElement?>> CreateShortcutAsync(
        string packageName,
        string applicationName,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(packageName))
        {
            return Task.FromResult(Failure("invalid", "Package name is required."));
        }

        if (string.IsNullOrWhiteSpace(applicationName))
        {
            return Task.FromResult(Failure("invalid", "Application name is required."));
        }

        var args = new List<string>
        {
            "shortcut",
            "create",
            "--package",
            packageName.Trim(),
            "--name",
            applicationName.Trim(),
            "--launcher",
            ResolveHostLauncher(),
        };
        AddAndroidHomeIconArgument(args, packageName, applicationName);
        return RunAsync(cancellationToken, args.ToArray());
    }

    internal static string ResolveHostLauncher()
    {
        var host = Path.Combine(AppContext.BaseDirectory, "AndroidSimulator.Host.exe");
        return File.Exists(host) ? host : ResolveLauncher();
    }

    public static Task<SimulatorCommandResult<JsonElement?>> RenameAppAsync(
        string packageName,
        string applicationName,
        CancellationToken cancellationToken = default)
        => RunAsync(
            cancellationToken,
            "app",
            "rename",
            "--package",
            packageName.Trim(),
            "--name",
            applicationName.Trim());

    public static Task<SimulatorCommandResult<JsonElement?>> UninstallAppAsync(
        string packageName,
        CancellationToken cancellationToken = default)
        => RunAsync(
            cancellationToken,
            "app",
            "uninstall",
            "--package",
            packageName.Trim());

        private static void AddAndroidHomeIconArgument(

        List<string> args,
        string packageName,
        string applicationName)
    {
        if (!string.Equals(packageName.Trim(), AndroidHomePackage, StringComparison.Ordinal)
            || !string.Equals(applicationName.Trim(), AndroidHomeTitle, StringComparison.Ordinal))
        {
            return;
        }

        args.Add("--icon");
        args.Add(Path.Combine(AppContext.BaseDirectory, "Assets", "AndroidIcon.ico"));
    }

    public static Task<SimulatorCommandResult<JsonElement?>> RegisterApkAssociationAsync(
        CancellationToken cancellationToken = default) =>
        RunAsync(
            cancellationToken,
            "integration",
            "register-apk",
            "--launcher",
            ResolveLauncher());

    public static Task<SimulatorCommandResult<JsonElement?>> GetApkAssociationStatusAsync(
        CancellationToken cancellationToken = default) =>
        RunAsync(
            cancellationToken,
            "integration",
            "status",
            "--launcher",
            ResolveLauncher());

    internal static async Task<SimulatorCommandResult<SimulatorRuntimeSettings>> GetSettingsAsync(
        CancellationToken cancellationToken = default)
    {
        var result = await RunAsync(cancellationToken, "settings", "show").ConfigureAwait(false);
        if (!result.Success || result.Data is not JsonElement data)
        {
            return Project(result, SimulatorRuntimeSettings.Defaults);
        }

        try
        {
            return Project(result, SimulatorRuntimeSettings.FromJson(data));
        }
        catch (JsonException exception)
        {
            return new SimulatorCommandResult<SimulatorRuntimeSettings>(
                false,
                "unparsed",
                $"simulatorctl settings show returned invalid data: {exception.Message}",
                result.Output,
                result.ExitCode,
                SimulatorRuntimeSettings.Defaults);
        }
    }

    internal static async Task<SimulatorCommandResult<SimulatorRuntimeSettings>> SaveSettingsAsync(
        SimulatorRuntimeSettings settings,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(settings);
        var args = new List<string>
        {
            "settings",
            "set",
            "--performance-mode",
            settings.PerformanceMode,
            "--cpu-cores",
            settings.CpuCores.ToString(CultureInfo.InvariantCulture),
            "--memory-mb",
            settings.MemoryMb.ToString(CultureInfo.InvariantCulture),
            "--renderer-mode",
            settings.RendererMode,
            "--graphics-strategy",
            settings.GraphicsStrategy,
            "--resolution-mode",
            settings.ResolutionMode,
            "--custom-width",
            settings.CustomWidth.ToString(CultureInfo.InvariantCulture),
            "--custom-height",
            settings.CustomHeight.ToString(CultureInfo.InvariantCulture),
            "--custom-dpi",
            settings.CustomDpi.ToString(CultureInfo.InvariantCulture),
            "--max-frame-rate",
            settings.MaxFrameRate.ToString(CultureInfo.InvariantCulture),
            "--dynamic-frame-rate",
            settings.DynamicFrameRate ? "true" : "false",
            "--dynamic-low-frame-rate",
            settings.DynamicLowFrameRate.ToString(CultureInfo.InvariantCulture),
            "--vertical-sync",
            settings.VerticalSync ? "true" : "false",
            "--super-resolution",
            settings.SuperResolution ? "true" : "false",
            "--super-resolution-scale-percent",
            settings.SuperResolutionScalePercent.ToString(CultureInfo.InvariantCulture),
            "--frame-interpolation",
            settings.FrameInterpolation ? "true" : "false",
            "--system-audio",
            settings.SystemAudio ? "true" : "false",
            "--keep-app-alive",
            settings.KeepAppAlive ? "true" : "false",
            "--remember-window-position",
            settings.RememberWindowPosition ? "true" : "false",
            "--fixed-window-size",
            settings.FixedWindowSize ? "true" : "false",
            "--auto-rotate",
            settings.AutoRotate ? "true" : "false",
            "--quit-confirm",
            settings.QuitConfirm ? "true" : "false",
        };

        var result = await RunAsync(cancellationToken, args.ToArray()).ConfigureAwait(false);
        if (!result.Success || result.Data is not JsonElement data)
        {
            return Project(result, settings);
        }

        try
        {
            return Project(result, SimulatorRuntimeSettings.FromJson(data));
        }
        catch (JsonException exception)
        {
            return new SimulatorCommandResult<SimulatorRuntimeSettings>(
                false,
                "unparsed",
                $"simulatorctl settings set returned invalid data: {exception.Message}",
                result.Output,
                result.ExitCode,
                settings);
        }
    }

    internal static async Task<SimulatorCommandResult<SimulatorRuntimeSettings>> ResetSettingsAsync(
        CancellationToken cancellationToken = default)
    {
        var result = await RunAsync(cancellationToken, "settings", "reset").ConfigureAwait(false);
        if (!result.Success || result.Data is not JsonElement data)
        {
            return Project(result, SimulatorRuntimeSettings.Defaults);
        }

        try
        {
            return Project(result, SimulatorRuntimeSettings.FromJson(data));
        }
        catch (JsonException exception)
        {
            return new SimulatorCommandResult<SimulatorRuntimeSettings>(
                false,
                "unparsed",
                $"simulatorctl settings reset returned invalid data: {exception.Message}",
                result.Output,
                result.ExitCode,
                SimulatorRuntimeSettings.Defaults);
        }
    }
    public static Task<SimulatorCommandResult<JsonElement?>> ImageCheckAsync(
        CancellationToken cancellationToken = default) =>
        RunAsync(
            cancellationToken,
            "owned",
            "image-check",
            "--instance",
            "android-simulator");

    public static Task<SimulatorCommandResult<JsonElement?>> BootstrapOwnedRuntimeAsync(
        CancellationToken cancellationToken = default) =>
        RunAsync(
            cancellationToken,
            "owned",
            "bootstrap",
            "--instance",
            "android-simulator");
    public static Task<SimulatorCommandResult<JsonElement?>> ProvisionOwnedRuntimeAsync(
        CancellationToken cancellationToken = default) =>
        RunAsync(
            cancellationToken,
            "owned",
            "provision",
            "--instance",
            "android-simulator");

    public static bool IsImageReady(SimulatorCommandResult<JsonElement?> result) =>
        result.Data is { } data
        && data.TryGetProperty("image_ready", out var imageReady)
        && imageReady.ValueKind is JsonValueKind.True or JsonValueKind.False
        && imageReady.GetBoolean();

    public static string ResolveLauncher()
    {
        var launcher = Environment.ProcessPath;
        return string.IsNullOrWhiteSpace(launcher)
            ? Path.Combine(AppContext.BaseDirectory, "AndroidSimulator.App.exe")
            : launcher;
    }

    public static Task<SimulatorCommandResult<JsonElement?>> SendTapAsync(
        uint displayId,
        int x,
        int y,
        CancellationToken cancellationToken = default)
    {
        if (displayId == 0)
        {
            return Task.FromResult(Failure("invalid", "A non-primary Android display id is required."));
        }

        return RunAsync(
            cancellationToken,
            "input",
            "tap",
            "--display-id",
            displayId.ToString(CultureInfo.InvariantCulture),
            "--x",
            x.ToString(CultureInfo.InvariantCulture),
            "--y",
            y.ToString(CultureInfo.InvariantCulture));
    }

    public static Task<SimulatorCommandResult<JsonElement?>> SendKeyEventAsync(
        uint displayId,
        int androidKeyCode,
        CancellationToken cancellationToken = default) =>
        SendKeyEventAsync(
            displayId,
            androidKeyCode.ToString(CultureInfo.InvariantCulture),
            cancellationToken);

    public static Task<SimulatorCommandResult<JsonElement?>> SendKeyEventAsync(
        uint displayId,
        string androidKeyCode,
        CancellationToken cancellationToken = default)
    {
        if (displayId == 0)
        {
            return Task.FromResult(Failure("invalid", "A non-primary Android display id is required."));
        }

        if (string.IsNullOrWhiteSpace(androidKeyCode))
        {
            return Task.FromResult(Failure("invalid", "Android key code is required."));
        }

        return RunAsync(
            cancellationToken,
            "input",
            "keyevent",
            "--display-id",
            displayId.ToString(CultureInfo.InvariantCulture),
            "--key",
            androidKeyCode.Trim());
    }

    public static Task<SimulatorCommandResult<JsonElement?>> SendSwipeAsync(
        uint displayId,
        int x1,
        int y1,
        int x2,
        int y2,
        int durationMs = 300,
        CancellationToken cancellationToken = default)
    {
        if (displayId == 0)
        {
            return Task.FromResult(Failure("invalid", "A non-primary Android display id is required."));
        }

        if (durationMs < 0)
        {
            return Task.FromResult(Failure("invalid", "Swipe duration cannot be negative."));
        }

        return RunAsync(
            cancellationToken,
            "input",
            "swipe",
            "--display-id",
            displayId.ToString(CultureInfo.InvariantCulture),
            "--x1",
            x1.ToString(CultureInfo.InvariantCulture),
            "--y1",
            y1.ToString(CultureInfo.InvariantCulture),
            "--x2",
            x2.ToString(CultureInfo.InvariantCulture),
            "--y2",
            y2.ToString(CultureInfo.InvariantCulture),
            "--duration-ms",
            durationMs.ToString(CultureInfo.InvariantCulture));
    }

    private static AndroidAppInfo CreateCachedApp(CachedAppInfo item)
    {
        string? resolvedIconPath = null;
        if (!string.IsNullOrWhiteSpace(item.IconPath))
        {
            try
            {
                var fullPath = Path.GetFullPath(item.IconPath);
                if (File.Exists(fullPath))
                {
                    resolvedIconPath = fullPath;
                }
            }
            catch
            {
                resolvedIconPath = null;
            }
        }

        return new AndroidAppInfo
        {
            PackageName = item.PackageName,
            Name = string.IsNullOrWhiteSpace(item.Name) ? item.PackageName : item.Name,
            ActivityName = item.ActivityName ?? string.Empty,
            Version = item.Version ?? string.Empty,
            IconPath = resolvedIconPath,
            HasIcon = resolvedIconPath is not null,
        };
    }

    private static void UpdateAppListCache(IReadOnlyList<AndroidAppInfo> apps)
    {
        var memorySnapshot = apps.ToArray();
        lock (AppListCacheSync)
        {
            _cachedApps = memorySnapshot;
        }

        var cache = new CachedAppList(
            1,
            DateTimeOffset.UtcNow,
            memorySnapshot
                .Select(app => new CachedAppInfo(
                    app.PackageName,
                    app.Name,
                    app.ActivityName,
                    app.Version,
                    app.IconPath))
                .ToArray());
        _ = PersistAppListCacheAsync(cache);
    }

    private static async Task PersistAppListCacheAsync(CachedAppList cache)
    {
        await AppListCacheFileGate.WaitAsync().ConfigureAwait(false);
        string? temporaryPath = null;
        try
        {
            var cacheDirectory = Path.GetDirectoryName(AppListCachePath);
            if (string.IsNullOrWhiteSpace(cacheDirectory))
            {
                return;
            }

            Directory.CreateDirectory(cacheDirectory);
            temporaryPath = $"{AppListCachePath}.{Environment.ProcessId}.tmp";
            var json = JsonSerializer.Serialize(cache, AppListCacheJsonOptions);
            await File.WriteAllTextAsync(temporaryPath, json, Encoding.UTF8).ConfigureAwait(false);
            File.Move(temporaryPath, AppListCachePath, overwrite: true);
            temporaryPath = null;
        }
        catch
        {
            // A cache write must never make live application enumeration fail.
        }
        finally
        {
            if (!string.IsNullOrWhiteSpace(temporaryPath))
            {
                try
                {
                    File.Delete(temporaryPath);
                }
                catch
                {
                }
            }
            AppListCacheFileGate.Release();
        }
    }

    private static SimulatorCommandResult<T> Project<T>(
        SimulatorCommandResult<JsonElement?> source,
        T data) =>
        new(
            source.Success,
            source.Status,
            source.Message,
            source.Output,
            source.ExitCode,
            data);

    private static SimulatorCommandResult<JsonElement?> Failure(string status, string message) =>
        new(false, status, message, string.Empty, -1, null);

    private static async Task ReadProcessOutputAsync(
        StreamReader reader,
        StringBuilder destination,
        CancellationToken cancellationToken)
    {
        var buffer = new char[4096];
        try
        {
            while (true)
            {
                var read = await reader.ReadAsync(buffer.AsMemory(), cancellationToken);
                if (read == 0)
                {
                    return;
                }

                destination.Append(buffer, 0, read);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
        catch (ObjectDisposedException) when (cancellationToken.IsCancellationRequested)
        {
        }
        catch (IOException) when (cancellationToken.IsCancellationRequested)
        {
        }
    }

    private static async Task IgnoreExpectedReaderShutdownAsync(params Task[] readers)
    {
        try
        {
            await Task.WhenAll(readers);
        }
        catch (Exception exception) when (
            exception is OperationCanceledException
                or ObjectDisposedException
                or IOException)
        {
        }
    }

    private static CommandEnvelope ParseEnvelope(string text)
    {
        try
        {
            using var document = JsonDocument.Parse(text);
            var root = document.RootElement;
            if (root.ValueKind != JsonValueKind.Object)
            {
                return CommandEnvelope.Unparsed;
            }

            var ok = root.TryGetProperty("ok", out var okElement)
                && okElement.ValueKind is JsonValueKind.True or JsonValueKind.False
                && okElement.GetBoolean();
            var status = GetString(root, "status");
            var message = GetString(root, "message");
            JsonElement? data = root.TryGetProperty("data", out var dataElement)
                ? dataElement.Clone()
                : null;
            return new CommandEnvelope(
                ok,
                string.IsNullOrWhiteSpace(status) ? "unknown" : status,
                message,
                data);
        }
        catch (JsonException)
        {
            return CommandEnvelope.Unparsed;
        }
    }

    private static string GetString(JsonElement element, string propertyName) =>
        element.TryGetProperty(propertyName, out var property)
        && property.ValueKind == JsonValueKind.String
            ? property.GetString() ?? string.Empty
            : string.Empty;

    private static string FormatOutput(string stdout, string stderr)
    {
        var formattedStdout = PrettyJsonOrText(stdout);
        var formattedStderr = stderr.Trim();
        if (formattedStderr.Length == 0)
        {
            return formattedStdout;
        }

        return formattedStdout.Length == 0
            ? formattedStderr
            : $"{formattedStdout}{Environment.NewLine}{formattedStderr}";
    }

    private static string PrettyJsonOrText(string text)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            return string.Empty;
        }

        try
        {
            using var document = JsonDocument.Parse(text);
            return JsonSerializer.Serialize(
                document.RootElement,
                new JsonSerializerOptions { WriteIndented = true });
        }
        catch (JsonException)
        {
            return text.Trim();
        }
    }

    private static void TryTerminate(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // Cancellation should still propagate even if the process exits concurrently.
        }
    }

    private sealed record CachedAppList(
        int SchemaVersion,
        DateTimeOffset UpdatedAt,
        CachedAppInfo[] Apps);

    private sealed record CachedAppInfo(
        string PackageName,
        string Name,
        string? ActivityName,
        string? Version,
        string? IconPath);

    private sealed record CommandEnvelope(
        bool Ok,
        string Status,
        string Message,
        JsonElement? Data)
    {
        public static CommandEnvelope Unparsed { get; } =
            new(false, "unparsed", "simulatorctl returned non-JSON output.", null);
    }
}
