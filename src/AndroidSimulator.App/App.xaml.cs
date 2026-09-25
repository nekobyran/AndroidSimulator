using System.Text.Json;
using AndroidSimulator.App.Models;
using AndroidSimulator.App.Services;
using Microsoft.UI.Xaml;

namespace AndroidSimulator.App;

public partial class App : Application
{
    private readonly KeyboardMappingService _keyboardMappingService = new();
    private readonly KeyMappingStore _keyMappingStore = new();
    private readonly AppWindowService _appWindowService = new();
    private readonly DirectLaunchParseResult _launchParseResult;
    private readonly Task<SimulatorCommandResult<JsonElement?>>? _earlyDirectLaunch;
    private readonly bool _redirectedToWarmHost;
    private PackageHostLease? _packageHostLease;
    private WarmHostActivationService? _warmHostActivation;

    public static string[] LaunchArguments { get; private set; } = [];

    public static MainWindow? MainWindow { get; private set; }

    public static string? StartupError { get; private set; }

    internal static KeyMappingStore KeyMappingStore => CurrentApp._keyMappingStore;

    internal static AppWindowService AppWindows => CurrentApp._appWindowService;

    private static App CurrentApp => (App)Current;

    public App()
    {
        EnsureInteractiveProcessPriority();
        LaunchArguments = Environment.GetCommandLineArgs().Skip(1).ToArray();
        _launchParseResult = ApkActivationService.ParseDirectLaunch(LaunchArguments);
        if (_launchParseResult.Request?.PackageName is string packageName
            && !TryOwnPackageHost(packageName))
        {
            _redirectedToWarmHost = true;
        }
        else if (_launchParseResult.Request is not null)
        {
            // Spawn simulatorctl before WinUI creates and lays out its visual tree.
            // The Android display and the hidden host now initialize in parallel.
            _earlyDirectLaunch = StartDirectLaunchAsync(_launchParseResult.Request);
        }

        InitializeComponent();
    }

    internal static void EnsureInteractiveProcessPriority()
    {
        try
        {
            LatencyPriorityService.PromoteCurrentProcess();
        }
        catch
        {
            // Process priority is a best-effort responsiveness guard. Startup and
            // recovery must remain available if an endpoint security policy denies it.
        }
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        if (_redirectedToWarmHost)
        {
            Environment.Exit(0);
            return;
        }

        var parseResult = _launchParseResult;
        var isDirectLaunch = parseResult.Request is not null || parseResult.Error is not null;

        MainWindow = new MainWindow(isDirectLaunch);
        MainWindow.Closed += MainWindow_Closed;
        if (isDirectLaunch)
        {
            MainWindow.PrepareDirectLaunch();
        }
        else
        {
            MainWindow.Activate();
        }

        if (parseResult.Error is not null)
        {
            MainWindow.ShowDirectLaunchFailure(parseResult.Error);
            return;
        }

        if (parseResult.Request is null)
        {
            await Task.WhenAll(
                InitializeKeyboardMappingsAsync(),
                EnsureApkAssociationAsync());
            return;
        }

        var request = parseResult.Request;

        // Start the actual app launch first. Keyboard mappings may initialize in
        // parallel, while package shortcuts do not need to inspect APK file
        // association state on every launch.
        var directLaunch = RunDirectLaunchAsync(
            request,
            _earlyDirectLaunch ?? StartDirectLaunchAsync(request));
        var keyboardInitialization = InitializeKeyboardMappingsAsync();
        var associationInitialization = request.ApkPath is null
            ? Task.CompletedTask
            : EnsureApkAssociationAsync();

        await directLaunch;
        await Task.WhenAll(keyboardInitialization, associationInitialization);
    }

    internal static void ReplaceKeyboardMappings(IEnumerable<KeyBinding> mappings)
    {
        var service = CurrentApp._keyboardMappingService;
        service.ReplaceMappings(mappings);
        service.IsEnabled = service.HasActiveMappings;
        if (service.HasActiveMappings && !service.IsRunning)
        {
            service.Start();
        }
        else if (!service.HasActiveMappings && service.IsRunning)
        {
            service.Stop();
        }
    }

    internal static void EnableWarmHostActivation(string packageName)
    {
        var app = CurrentApp;
        app.EnsureWarmHostActivation(packageName);
    }

    private bool TryOwnPackageHost(string packageName)
    {
        if (WarmHostActivationService.TryActivateExisting(packageName))
        {
            return false;
        }

        // The named mutex is the strict cold-start boundary. A competing process
        // can arrive before the primary has created its pipe, so retry both paths:
        // either the primary exposes warm activation or its abandoned lease becomes
        // available. Never create a second WinUI/scrcpy host while another owner lives.
        var deadline = Environment.TickCount64 + 2000;
        do
        {
            if (PackageHostLease.TryAcquire(packageName, out var lease))
            {
                _packageHostLease = lease;
                EnsureWarmHostActivation(packageName);
                return true;
            }

            if (WarmHostActivationService.TryActivateExisting(packageName, 100))
            {
                return false;
            }

            Thread.Sleep(20);
        }
        while (Environment.TickCount64 < deadline);

        // Fail closed if an owner remains alive but cannot acknowledge activation.
        // Exiting this invocation is preferable to stealing its scrcpy HWND.
        return false;
    }

    private void EnsureWarmHostActivation(string packageName)
    {
        if (_warmHostActivation is not null)
        {
            return;
        }

        _warmHostActivation = new WarmHostActivationService(packageName, () =>
        {
            if (MainWindow is not { } window)
            {
                return;
            }

            _ = window.DispatcherQueue.TryEnqueue(
                Microsoft.UI.Dispatching.DispatcherQueuePriority.High,
                () => window.RestoreWarmHost());
        });
    }

    private async Task InitializeKeyboardMappingsAsync()
    {
        try
        {
            var mappings = await _keyMappingStore.LoadAsync();
            ReplaceKeyboardMappings(mappings);
        }
        catch (Exception exception)
        {
            StartupError = exception.Message;
        }
    }

    private static async Task EnsureApkAssociationAsync()
    {
        // Avoid rewriting registry state and notifying Explorer on every launch.
        // Registration is repaired only after the current executable no longer matches.
        var status = await SimulatorCtlService.GetApkAssociationStatusAsync();
        if (status.Success)
        {
            return;
        }

        var result = await SimulatorCtlService.RegisterApkAssociationAsync();
        if (!result.Success)
        {
            StartupError = $"APK 文件关联未完成：{result.Message}";
        }
    }

    private static Task<SimulatorCommandResult<JsonElement?>> StartDirectLaunchAsync(
        DirectLaunchRequest request)
    {
        return request.ApkPath is not null
            ? SimulatorCtlService.InstallApkAsync(request.ApkPath, launch: true, title: null)
            : SimulatorCtlService.LaunchAppAsync(request.PackageName!, request.DisplayName);
    }

    private async Task RunDirectLaunchAsync(
        DirectLaunchRequest request,
        Task<SimulatorCommandResult<JsonElement?>> launchTask)
    {
        var mainWindow = MainWindow;
        if (mainWindow is null)
        {
            return;
        }

        try
        {
            var result = await launchTask;

            if (!result.Success)
            {
                mainWindow.ShowDirectLaunchFailure(
                    AppWindowService.DescribeLaunchFailure(result));
                return;
            }

            if (!AppWindowMetadata.TryParse(result.Data, out var appWindow))
            {
                mainWindow.ShowDirectLaunchFailure(
                    "控制组件没有返回可信的独立应用窗口信息。为避免误操作 Android 主显示，启动器不会猜测窗口或启用键位。");
                return;
            }

            if (!await ShowAppHostWhenReadyAsync(appWindow))
            {
                var stopTask = SimulatorCtlService.StopAppAsync(appWindow.PackageName);
                mainWindow.ShowDirectLaunchFailure(
                    $"{request.DisplayName} 已在后台 Android 中启动，但受信任的应用窗口尚未能嵌入 WinUI 宿主。可返回应用库重试。");
                _ = await stopTask;
                return;
            }

            if (request.ApkPath is not null)
            {
                var shortcut = await SimulatorCtlService.CreateShortcutAsync(
                    appWindow.PackageName,
                    appWindow.Title);
                if (!shortcut.Success && ReferenceEquals(MainWindow, mainWindow))
                {
                    mainWindow.ShowHostedAppWarning(
                        "桌面快捷方式创建失败",
                        shortcut.Message);
                }
            }
        }
        catch (Exception exception)
        {
            mainWindow.ShowDirectLaunchFailure(exception.Message);
        }
    }

    private async Task<bool> ShowAppHostWhenReadyAsync(AppWindowMetadata appWindow)
    {
        for (var attempt = 0; attempt < 40; attempt++)
        {
            if (MainWindow?.ShowAppHostWindow(appWindow, out _) == true)
            {
                return true;
            }

            await Task.Delay(16);
        }

        return false;
    }

    private void MainWindow_Closed(object sender, WindowEventArgs args)
    {
        _warmHostActivation?.Dispose();
        _warmHostActivation = null;
        _packageHostLease?.Dispose();
        _packageHostLease = null;
        _keyboardMappingService.Dispose();
        MainWindow = null;
        Environment.Exit(0);
    }
}
