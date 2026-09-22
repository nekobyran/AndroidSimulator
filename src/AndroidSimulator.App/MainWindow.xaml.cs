using AndroidSimulator.App.Pages;
using AndroidSimulator.App.Services;
using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Graphics;
using Windows.Foundation;
using Windows.System.Power;
using Windows.UI;
using Windows.UI.ViewManagement;

namespace AndroidSimulator.App;

public sealed partial class MainWindow : Window
{
    private const int DefaultWindowWidth = 1360;
    private const int DefaultWindowHeight = 840;
    private const int DefaultAppHostLogicalWidth = 1440;
    private const int AppHostTitleBarHeight = 32;
    private const int MinimumWindowWidth = 760;
    private const int MinimumWindowHeight = 540;
    private const int MinimumAppHostWidth = 640;
    private const int MinimumAppHostHeight = 408;
    private const int DirectLaunchWindowWidth = 560;
    private const int DirectLaunchWindowHeight = 210;
    private const int DirectLaunchFailureWidth = 680;
    private const int DirectLaunchFailureHeight = 380;
    private const double DefaultDpi = 96.0;
    private const uint MonitorDefaultToNearest = 2;
    private const uint VirtualKeyF12 = 0x7B;
    private const int WhKeyboardLl = 13;
    private const int HcAction = 0;
    private const uint WmKeyDown = 0x0100;
    private const uint WmSysKeyDown = 0x0104;
    private const uint GaRoot = 2;
    private static readonly TimeSpan AppHostHealthCheckInterval = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan AppStopTimeout = TimeSpan.FromSeconds(3);
    private static readonly TimeSpan HostLayoutSettlementDelay = TimeSpan.FromMilliseconds(250);

    private readonly AccessibilitySettings _accessibilitySettings = new();
    private readonly UISettings _uiSettings = new();
    private readonly nint _windowHandle;
    private readonly HostedWindowRecoveryGate _hostRecoveryGate = new();
    private readonly BackdropConfigurationGate _backdropConfigurationGate = new();
    private readonly LowLevelKeyboardProc _keyboardHookProc;
    private nint _keyboardHook;
    private string _backdropMode = string.Empty;
    private HostedAppWindow? _hostedAppWindow;
    private AppWindowMetadata? _hostedAppMetadata;
    private Microsoft.UI.Dispatching.DispatcherQueueTimer? _hostHealthTimer;
    private Microsoft.UI.Dispatching.DispatcherQueueTimer? _hostLayoutSettlementTimer;
    private CancellationTokenSource? _hostRecoveryCancellation;
    private AppWindowPresenterKind _presenterBeforeFullScreen = AppWindowPresenterKind.Overlapped;
    private CancellationTokenSource? _adaptiveTitleBarSampleCancellation;
    private Color? _adaptiveTitleBarColor;
    private bool _adaptiveTitleBarUsesLightForeground;
    private bool _isDirectLaunchPresentation;
    private bool _isFullScreen;
    private bool _hostLayoutQueued;
    private bool _sessionTornDown;

    public string BackdropDescription => _backdropMode switch
    {
        "acrylic" => "桌面亚克力",
        "mica" => "云母（节能回退）",
        _ => "系统纯色（高对比或兼容回退）",
    };

    public MainWindow(bool directLaunch = false)
    {
        InitializeComponent();
        _isDirectLaunchPresentation = directLaunch;

        _windowHandle = WinRT.Interop.WindowNative.GetWindowHandle(this);
        _keyboardHookProc = KeyboardHookCallback;
        _keyboardHook = SetWindowsHookEx(
            WhKeyboardLl,
            _keyboardHookProc,
            0,
            0);
        AppWindow.Title = "Android Simulator";
        AppWindow.TitleBar.IconShowOptions = IconShowOptions.HideIconAndSystemMenu;
        AppWindow.SetIcon("Assets/AppIcon.ico");
        if (directLaunch)
        {
            // Establish the final host geometry while the window is still hidden.
            // Later title-bar and content swaps must not move or resize it.
            MoveAndResizeCentered(GetDefaultAppHostSize());
        }
        else
        {
            AppWindow.Resize(new SizeInt32(DefaultWindowWidth, DefaultWindowHeight));
        }

        // Window geometry is final before any custom title-bar is installed.
        // SetTitleBar only changes hit-testing/visuals and never owns placement.
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);

        Activated += MainWindow_Activated;
        SizeChanged += MainWindow_SizeChanged;
        AppWindow.Changed += AppWindow_Changed;
        AppWindow.Closing += AppWindow_Closing;
        Closed += MainWindow_Closed;
        if (directLaunch)
        {
            RootGrid.Background = new SolidColorBrush(
                _uiSettings.GetColorValue(UIColorType.Background));
            _backdropMode = "solid";
        }
        else
        {
            ConfigureBackdrop();
        }
        ConfigureTitleBar();

        if (!directLaunch)
        {
            NavigateTo("apps");
        }
    }

    public void NavigateTo(string destination)
    {
        NavigateFrame(destination);
    }

    public void PrepareDirectLaunch()
    {
        _isDirectLaunchPresentation = true;
        AppTitleBar.Visibility = Visibility.Collapsed;
        ContentFrame.Visibility = Visibility.Collapsed;
        AppHostShell.Visibility = Visibility.Collapsed;
        DirectLaunchStatusPanel.Visibility = Visibility.Collapsed;
        SetTitleBar(DirectLaunchTitleBar);
        AppWindow.IsShownInSwitchers = false;
    }

    public void ShowDirectLaunchProgress(string title, string message)
    {
        DirectLaunchStatusPanel.Visibility = Visibility.Visible;
        DirectLaunchTitle.Text = title;
        DirectLaunchMessage.Text = message;
        AppWindow.Title = title;
        DirectLaunchInfoBar.IsOpen = false;
        DirectLaunchBackButton.Visibility = Visibility.Collapsed;
        _ = DispatcherQueue.TryEnqueue(
            Microsoft.UI.Dispatching.DispatcherQueuePriority.Low,
            ConfigureBackdrop);
    }

    public void ShowDirectLaunchFailure(string message)
    {
        MoveAndResizeCentered(new SizeInt32(DirectLaunchFailureWidth, DirectLaunchFailureHeight));
        DirectLaunchStatusPanel.Visibility = Visibility.Visible;
        DirectLaunchTitle.Text = "无法打开 Android 应用";
        DirectLaunchMessage.Text = "后台 Android 或独立应用窗口没有完成目标操作。可返回应用库重试。";
        DirectLaunchInfoBar.Message = message;
        DirectLaunchInfoBar.IsOpen = true;
        DirectLaunchBackButton.Visibility = Visibility.Visible;
        AppWindow.IsShownInSwitchers = true;
        AppWindow.Show();
        Activate();
    }

    internal bool ShowAppHostWindow(AppWindowMetadata metadata, out string failure)
    {
        ArgumentNullException.ThrowIfNull(metadata);
        failure = string.Empty;
        if (!App.AppWindows.TryCreateHostTarget(metadata, out var target, out failure))
        {
            return false;
        }

        var previousTarget = _hostedAppWindow;
        var wasHostingApp = _hostedAppMetadata is not null
            && AppHostShell.Visibility == Visibility.Visible;
        _hostedAppWindow = target;
        _isDirectLaunchPresentation = false;
        _sessionTornDown = false;
        App.AppWindows.RegisterHostFocusTarget(_windowHandle, metadata);
        AppTitleBar.Visibility = Visibility.Collapsed;
        ContentFrame.Visibility = Visibility.Collapsed;
        DirectLaunchStatusPanel.Visibility = Visibility.Collapsed;
        AppHostShell.Visibility = Visibility.Visible;
        SetTitleBar(AppHostTitleBarDragRegion);

        AppHostTitleText.Text = metadata.Title;
        AppWindow.Title = metadata.Title;
        ApplyAppIcon(metadata.IconPath);
        AppWindow.IsShownInSwitchers = true;
        AppWindow.Show();
        Activate();

        RootGrid.UpdateLayout();
        var bounds = GetHostViewportBounds();
        if (bounds.Width > 1
            && bounds.Height > 1
            && !App.AppWindows.TryAttachToHost(target, _windowHandle, bounds, out failure))
        {
            _hostedAppWindow = previousTarget;
            App.AppWindows.ClearHostFocusTarget(_windowHandle);
            if (wasHostingApp)
            {
                return false;
            }

            AppHostShell.Visibility = Visibility.Collapsed;
            ContentFrame.Visibility = Visibility.Visible;
            AppTitleBar.Visibility = Visibility.Visible;
            SetTitleBar(AppTitleBar);
            AppWindow.Title = "Android Simulator";
            AppWindow.SetIcon("Assets/AppIcon.ico");
            return false;
        }

        _hostedAppMetadata = metadata;
        // Attachment can pump window messages before metadata is committed.
        // Apply once synchronously so the initial Android virtual display never
        // remains at scrcpy's bootstrap size until the user manually resizes.
        RootGrid.UpdateLayout();
        ApplyHostLayoutNow(sampleTitleBar: false);
        QueueSettledHostLayout();
        QueueAdaptiveTitleBarColorSample(TimeSpan.FromMilliseconds(240));
        StartHostHealthMonitor();
        TryRegisterFullScreenHotKey();
        App.EnableWarmHostActivation(metadata.PackageName);
        return true;
    }

    internal void RestoreWarmHost()
    {
        if (_hostedAppWindow is null)
        {
            return;
        }

        AppWindow.IsShownInSwitchers = true;
        AppWindow.Show();
        Activate();
        QueueHostLayout();
        StartHostHealthMonitor();
        _ = CheckHostedWindowHealthAsync();
    }

    public void ApplyRequestedTheme(ElementTheme theme)
    {
        RootGrid.RequestedTheme = theme;
        ConfigureBackdrop();
    }

    private void MainWindow_Activated(object sender, WindowActivatedEventArgs args)
    {
        App.EnsureInteractiveProcessPriority();
        if (!_isDirectLaunchPresentation)
        {
            ConfigureBackdrop();
        }
        ConfigureTitleBar();
        if (args.WindowActivationState != WindowActivationState.Deactivated
            && _hostedAppWindow is not null)
        {
            TryRegisterFullScreenHotKey();
            // Initial frame and rotation events already schedule one edge sample.
        }
    }

    private void MainWindow_SizeChanged(object sender, WindowSizeChangedEventArgs args)
    {
        var minimumWidth = _isDirectLaunchPresentation
            ? DirectLaunchWindowWidth
            : _hostedAppWindow is null ? MinimumWindowWidth : MinimumAppHostWidth;
        var minimumHeight = _isDirectLaunchPresentation
            ? DirectLaunchWindowHeight
            : _hostedAppWindow is null ? MinimumWindowHeight : MinimumAppHostHeight;
        var width = Math.Max(minimumWidth, args.Size.Width);
        var height = Math.Max(minimumHeight, args.Size.Height);
        if (width > args.Size.Width || height > args.Size.Height)
        {
            var scale = RootGrid.XamlRoot?.RasterizationScale ?? 1.0;
            AppWindow.Resize(new SizeInt32(
                (int)Math.Ceiling(width * scale),
                (int)Math.Ceiling(height * scale)));
        }

        QueueHostLayout();
    }

    private void ConfigureBackdrop()
    {
        if (!_backdropConfigurationGate.TryEnter())
        {
            return;
        }

        try
        {
            if (_hostedAppWindow is not null || _isDirectLaunchPresentation)
            {
                ApplyMicaOrSolid();
                return;
            }

            if (IsHighContrastEnabled())
            {
                ApplySolidBackdrop();
                return;
            }

            if (IsEnergySaverEnabled())
            {
                ApplyMicaOrSolid();
                return;
            }

            if (_backdropMode == "acrylic")
            {
                return;
            }

            try
            {
                SystemBackdrop = new DesktopAcrylicBackdrop();
                RootGrid.Background = new SolidColorBrush(Colors.Transparent);
                _backdropMode = "acrylic";
            }
            catch
            {
                ApplyMicaOrSolid();
            }
        }
        finally
        {
            _backdropConfigurationGate.Exit();
        }
    }

    private void AppWindow_Changed(AppWindow sender, AppWindowChangedEventArgs args)
    {
        if (args.DidPositionChange)
        {
            ApplyHostLayoutNow(sampleTitleBar: false);
            QueueSettledHostLayout();
        }
    }

    private void ApplyMicaOrSolid()
    {
        if (_backdropMode == "mica")
        {
            return;
        }

        try
        {
            SystemBackdrop = new MicaBackdrop();
            RootGrid.Background = new SolidColorBrush(Colors.Transparent);
            _backdropMode = "mica";
        }
        catch
        {
            ApplySolidBackdrop();
        }
    }

    private void ApplySolidBackdrop()
    {
        SystemBackdrop = null;
        RootGrid.Background = new SolidColorBrush(_uiSettings.GetColorValue(UIColorType.Background));
        _backdropMode = "solid";
    }

    private void ConfigureTitleBar()
    {
        if (_hostedAppWindow is not null && _adaptiveTitleBarColor is Color adaptiveColor)
        {
            ApplyAdaptiveTitleBarColors(adaptiveColor, _adaptiveTitleBarUsesLightForeground);
            return;
        }

        var titleBar = AppWindow.TitleBar;
        titleBar.ButtonBackgroundColor = Colors.Transparent;
        titleBar.ButtonInactiveBackgroundColor = Colors.Transparent;
        titleBar.ButtonHoverBackgroundColor = Color.FromArgb(0x22, 0xFF, 0x98, 0x00);
        titleBar.ButtonPressedBackgroundColor = Color.FromArgb(0x33, 0xFF, 0x98, 0x00);
        titleBar.ButtonForegroundColor = _uiSettings.GetColorValue(UIColorType.Foreground);
        titleBar.ButtonHoverForegroundColor = _uiSettings.GetColorValue(UIColorType.Foreground);
        titleBar.ButtonPressedForegroundColor = _uiSettings.GetColorValue(UIColorType.Foreground);
        titleBar.ButtonInactiveForegroundColor = Colors.Gray;
    }

    private bool IsHighContrastEnabled()
    {
        try
        {
            return _accessibilitySettings.HighContrast;
        }
        catch
        {
            return true;
        }
    }

    private static bool IsEnergySaverEnabled()
    {
        try
        {
            return PowerManager.EnergySaverStatus == EnergySaverStatus.On;
        }
        catch
        {
            return true;
        }
    }

    private void NavigateFrame(string destination)
    {
        var pageType = destination switch
        {
            "apps" => typeof(AppsPage),
            "key-mappings" => typeof(KeyMappingPage),
            "settings" => typeof(SettingsPage),
            _ => typeof(AppsPage),
        };

        if (ContentFrame.CurrentSourcePageType == pageType)
        {
            return;
        }

        NavigationTransitionInfo transition = _uiSettings.AnimationsEnabled
            ? new EntranceNavigationTransitionInfo()
            : new SuppressNavigationTransitionInfo();
        ContentFrame.Navigate(pageType, null, transition);
    }

    private void ContentFrame_Navigated(object sender, Microsoft.UI.Xaml.Navigation.NavigationEventArgs e)
    {
        DirectLaunchStatusPanel.Visibility = Visibility.Collapsed;
    }

    private void DirectLaunchBackButton_Click(object sender, RoutedEventArgs e)
    {
        _isDirectLaunchPresentation = false;
        AppWindow.Resize(new SizeInt32(DefaultWindowWidth, DefaultWindowHeight));
        DirectLaunchStatusPanel.Visibility = Visibility.Collapsed;
        AppTitleBar.Visibility = Visibility.Visible;
        ContentFrame.Visibility = Visibility.Visible;
        SetTitleBar(AppTitleBar);
        AppWindow.Title = "Android Simulator";
        NavigateTo("apps");
    }

    private void MoveAndResizeCentered(SizeInt32 requestedSize)
    {
        if (TryGetNativeMonitorWorkArea(out var nativeWorkArea))
        {
            AppWindow.MoveAndResize(WindowPlacement.CenterInWorkArea(nativeWorkArea, requestedSize));
            return;
        }

        var displayArea = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary);
        if (displayArea is null)
        {
            AppWindow.Resize(requestedSize);
            return;
        }

        AppWindow.MoveAndResize(WindowPlacement.CenterInWorkArea(displayArea.WorkArea, requestedSize));
    }

    private void AppHostViewport_SizeChanged(object sender, SizeChangedEventArgs e)
    {
        ApplyHostLayoutNow(sampleTitleBar: false);
        QueueSettledHostLayout();
    }

    internal void ToggleFullScreen()
    {
        if (!_isFullScreen)
        {
            _presenterBeforeFullScreen = AppWindow.Presenter.Kind;
            AppWindow.SetPresenter(AppWindowPresenterKind.FullScreen);
            _isFullScreen = true;
        }
        else
        {
            AppWindow.SetPresenter(_presenterBeforeFullScreen == AppWindowPresenterKind.FullScreen
                ? AppWindowPresenterKind.Overlapped
                : _presenterBeforeFullScreen);
            _isFullScreen = false;
        }

        QueueHostLayout();
        QueueSettledHostLayout();
    }

    private void TryRegisterFullScreenHotKey()
    {
        if (_keyboardHook != 0 || _hostedAppWindow is null)
        {
            return;
        }

        _keyboardHook = SetWindowsHookEx(
            WhKeyboardLl,
            _keyboardHookProc,
            0,
            0);
        if (_keyboardHook == 0)
        {
            ShowHostWarning("F12 全屏暂不可用", "系统未能安装当前窗口的键盘监听；重新打开窗口后可重试。");
        }
    }

    private void UnregisterFullScreenHotKey()
    {
        if (_keyboardHook == 0)
        {
            return;
        }

        _ = UnhookWindowsHookEx(_keyboardHook);
        _keyboardHook = 0;
    }

    private nint KeyboardHookCallback(
        int code,
        nint wParam,
        nint lParam)
    {
        if (code == HcAction
            && ((uint)wParam == WmKeyDown || (uint)wParam == WmSysKeyDown)
            && Marshal.PtrToStructure<LowLevelKeyboardInput>(lParam).VirtualKey == VirtualKeyF12
            && (GetAncestor(GetForegroundWindow(), GaRoot) == _windowHandle
                || GetForegroundWindow() == _hostedAppWindow?.WindowHandle))
        {
            _ = DispatcherQueue.TryEnqueue(
                Microsoft.UI.Dispatching.DispatcherQueuePriority.High,
                ToggleFullScreen);
            return 1;
        }

        return CallNextHookEx(_keyboardHook, code, wParam, lParam);
    }

    private void MainWindow_Closed(object sender, WindowEventArgs args)
    {
        _hostRecoveryGate.Close();
        StopSettledHostLayout();
        _adaptiveTitleBarSampleCancellation?.Cancel();
        _adaptiveTitleBarSampleCancellation?.Dispose();
        _adaptiveTitleBarSampleCancellation = null;
        TeardownHostedAppSession(forceStopAndroid: true);
        UnregisterFullScreenHotKey();
    }

    private void TeardownHostedAppSession(bool forceStopAndroid)
    {
        if (_sessionTornDown)
        {
            return;
        }

        _sessionTornDown = true;
        StopHostHealthMonitor(permanently: true);

        var packageName = _hostedAppMetadata?.PackageName;
        if (App.AppWindows.IsHostedWindowAttachedTo(_hostedAppWindow, _windowHandle)
            || _hostedAppWindow is not null)
        {
            App.AppWindows.RequestCloseHostedWindow(_hostedAppWindow);
        }

        App.AppWindows.ClearHostFocusTarget(_windowHandle);
        _hostedAppWindow = null;
        _hostedAppMetadata = null;

        if (forceStopAndroid
            && !string.IsNullOrWhiteSpace(packageName)
            && !packageName.Equals("com.android.launcher3", StringComparison.Ordinal))
        {
            // Closing runs on the WinUI thread. Start the bounded stop now, but
            // never synchronously wait for simulatorctl or the native window can
            // remain stuck in its closing message while the scrcpy HWND is gone.
            _ = StopHostedAndroidAppAsync(packageName);
        }
    }

    private static async Task StopHostedAndroidAppAsync(string packageName)
    {
        using var cancellation = new CancellationTokenSource(AppStopTimeout);
        try
        {
            _ = await SimulatorCtlService.StopAppAsync(packageName, cancellation.Token)
                .ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
        catch
        {
            // Host destruction must remain reliable even if ADB disappears.
        }
    }

    private void AppWindow_Closing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        // Close intent is final. Shut recovery/layout down before touching scrcpy
        // so no queued callback can resurrect the session while Windows closes it.
        _hostRecoveryGate.Close();
        StopSettledHostLayout();
        _hostRecoveryCancellation?.Cancel();
        _adaptiveTitleBarSampleCancellation?.Cancel();
        TeardownHostedAppSession(forceStopAndroid: true);
    }

    private void QueueHostLayout()
    {
        if (_hostedAppWindow is null || AppHostShell.Visibility != Visibility.Visible)
        {
            return;
        }

        if (_hostLayoutQueued)
        {
            return;
        }

        _hostLayoutQueued = true;
        if (!DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.High, () =>
        {
            _hostLayoutQueued = false;
            ApplyHostLayoutNow(sampleTitleBar: false);
        }))
        {
            _hostLayoutQueued = false;
        }
    }

    private void QueueSettledHostLayout()
    {
        if (_hostedAppWindow is null || AppHostShell.Visibility != Visibility.Visible)
        {
            return;
        }

        _hostLayoutSettlementTimer ??= CreateHostLayoutSettlementTimer();
        _hostLayoutSettlementTimer.Stop();
        _hostLayoutSettlementTimer.Start();
    }

    private Microsoft.UI.Dispatching.DispatcherQueueTimer CreateHostLayoutSettlementTimer()
    {
        var timer = DispatcherQueue.CreateTimer();
        timer.Interval = HostLayoutSettlementDelay;
        timer.IsRepeating = false;
        timer.Tick += (_, _) =>
        {
            RootGrid.UpdateLayout();
            ApplyHostLayoutNow(sampleTitleBar: false);
        };
        return timer;
    }

    private void StopSettledHostLayout()
    {
        _hostLayoutSettlementTimer?.Stop();
        _hostLayoutSettlementTimer = null;
    }

    private void ApplyHostLayoutNow(bool sampleTitleBar)
    {
        if (_hostedAppWindow is null || AppHostShell.Visibility != Visibility.Visible)
        {
            return;
        }

        var bounds = GetHostViewportBounds();
        if (!App.AppWindows.TryLayoutHostedWindow(_hostedAppWindow, bounds, out var failure))
        {
            ShowHostWarning("宿主布局失败", failure);
            return;
        }

        // scrcpy --flex-display continuously keeps the owned virtual display in
        // lockstep with this HWND. Do not issue parallel ADB wm size/density
        // mutations during a drag; they race the scrcpy server and add latency.
        if (sampleTitleBar)
        {
            QueueAdaptiveTitleBarColorSample(TimeSpan.FromMilliseconds(180));
        }
    }
    private RectInt32 GetHostViewportBounds()
    {
        var scale = AppHostViewport.XamlRoot?.RasterizationScale ?? 1.0;
        if (GetClientRect(_windowHandle, out var clientRect))
        {
            return WindowPlacement.CreatePhysicalHostViewport(
                clientRect.Right - clientRect.Left,
                clientRect.Bottom - clientRect.Top,
                AppHostTitleBarHeight,
                scale);
        }

        var logicalBounds = AppHostViewport
            .TransformToVisual(RootGrid)
            .TransformBounds(new Rect(0, 0, AppHostViewport.ActualWidth, AppHostViewport.ActualHeight));
        return new RectInt32(
            (int)Math.Round(logicalBounds.X * scale),
            (int)Math.Round(logicalBounds.Y * scale),
            Math.Max(1, (int)Math.Round(logicalBounds.Width * scale)),
            Math.Max(1, (int)Math.Round(logicalBounds.Height * scale)));
    }

    private SizeInt32 GetDefaultAppHostSize()
    {
        return WindowPlacement.ScaleHostSize(
            DefaultAppHostLogicalWidth,
            AppHostTitleBarHeight,
            GetTargetMonitorScale());
    }

    private double GetTargetMonitorScale()
    {
        var monitor = MonitorFromWindow(_windowHandle, MonitorDefaultToNearest);
        var scalePercent = 100;
        if (monitor != 0)
        {
            _ = GetScaleFactorForMonitor(monitor, out scalePercent);
        }

        if (TryGetNativeMonitorWorkArea(out var nativeWorkArea))
        {
            var displayArea = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary);
            if (displayArea is not null)
            {
                return WindowPlacement.ResolveMonitorScale(
                    scalePercent,
                    nativeWorkArea,
                    displayArea.WorkArea);
            }
        }

        if (scalePercent > 0)
        {
            return scalePercent / 100.0;
        }

        var dpi = GetDpiForWindow(_windowHandle);
        return dpi > 0
            ? dpi / DefaultDpi
            : 1.0;
    }

    private bool TryGetNativeMonitorWorkArea(out RectInt32 workArea)
    {
        workArea = default;
        var monitor = MonitorFromWindow(_windowHandle, MonitorDefaultToNearest);
        if (monitor == 0)
        {
            return false;
        }

        var info = new MonitorInfo
        {
            Size = (uint)Marshal.SizeOf<MonitorInfo>(),
        };
        if (!GetMonitorInfo(monitor, ref info))
        {
            return false;
        }

        workArea = new RectInt32(
            info.WorkArea.Left,
            info.WorkArea.Top,
            info.WorkArea.Right - info.WorkArea.Left,
            info.WorkArea.Bottom - info.WorkArea.Top);
        return workArea.Width > 0 && workArea.Height > 0;
    }

    private void QueueAdaptiveTitleBarColorSample(TimeSpan delay)
    {
        var target = _hostedAppWindow;
        if (target is null || AppHostShell.Visibility != Visibility.Visible || IsHighContrastEnabled())
        {
            return;
        }

        _adaptiveTitleBarSampleCancellation?.Cancel();
        _adaptiveTitleBarSampleCancellation?.Dispose();
        var cancellation = new CancellationTokenSource();
        _adaptiveTitleBarSampleCancellation = cancellation;
        _ = SampleAdaptiveTitleBarColorAsync(target, delay, cancellation);
    }

    private async Task SampleAdaptiveTitleBarColorAsync(
        HostedAppWindow target,
        TimeSpan delay,
        CancellationTokenSource cancellation)
    {
        try
        {
            await Task.Delay(delay, cancellation.Token);
            if (cancellation.IsCancellationRequested
                || _hostedAppWindow?.WindowHandle != target.WindowHandle
                || !App.AppWindows.TrySampleHostedTopEdge(
                    target,
                    _windowHandle,
                    out var edgePixels)
                || !AdaptiveTitleBarColor.TryCreatePalette(edgePixels, out var palette))
            {
                return;
            }

            _adaptiveTitleBarColor = Color.FromArgb(0xFF, palette.Red, palette.Green, palette.Blue);
            _adaptiveTitleBarUsesLightForeground = palette.UseLightForeground;
            ConfigureTitleBar();
        }
        catch (OperationCanceledException)
        {
        }
        finally
        {
            if (ReferenceEquals(_adaptiveTitleBarSampleCancellation, cancellation))
            {
                _adaptiveTitleBarSampleCancellation = null;
                cancellation.Dispose();
            }
        }
    }

    private void ApplyAdaptiveTitleBarColors(Color edgeColor, bool useLightForeground)
    {
        var foreground = useLightForeground ? Colors.White : Colors.Black;
        var secondaryForeground = Color.FromArgb(
            0xD9,
            foreground.R,
            foreground.G,
            foreground.B);
        var hover = Color.FromArgb(
            0x24,
            foreground.R,
            foreground.G,
            foreground.B);
        var pressed = Color.FromArgb(
            0x38,
            foreground.R,
            foreground.G,
            foreground.B);

        AppHostTitleBarTint.Background = new SolidColorBrush(Color.FromArgb(
            0xC4,
            edgeColor.R,
            edgeColor.G,
            edgeColor.B));

        AppHostTitleText.Foreground = new SolidColorBrush(foreground);
        AppHostFallbackIcon.Foreground = new SolidColorBrush(foreground);

        var titleBar = AppWindow.TitleBar;
        titleBar.ButtonBackgroundColor = Colors.Transparent;
        titleBar.ButtonInactiveBackgroundColor = Colors.Transparent;
        titleBar.ButtonHoverBackgroundColor = hover;
        titleBar.ButtonPressedBackgroundColor = pressed;
        titleBar.ButtonForegroundColor = foreground;
        titleBar.ButtonHoverForegroundColor = foreground;
        titleBar.ButtonPressedForegroundColor = foreground;
        titleBar.ButtonInactiveForegroundColor = secondaryForeground;
    }

    private void ApplyAppIcon(string? iconPath)
    {
        var displayIcon = HostAppIcon.ResolveDisplayIconPath(iconPath)
            ?? ResolveExistingIconPath(iconPath);
        try
        {
            AppWindow.SetIcon(displayIcon);
        }
        catch
        {
            AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico"));
        }

        if (!string.IsNullOrWhiteSpace(displayIcon)
            && File.Exists(displayIcon)
            && !IsFallbackAppIcon(displayIcon))
        {
            var bitmap = new BitmapImage();
            bitmap.CreateOptions = BitmapCreateOptions.IgnoreImageCache;
            bitmap.DecodePixelType = DecodePixelType.Logical;
            bitmap.DecodePixelWidth = 32;
            bitmap.DecodePixelHeight = 32;
            bitmap.UriSource = new Uri(Path.GetFullPath(displayIcon));
            AppHostIconImage.Source = bitmap;
            AppHostIconImage.Visibility = Visibility.Visible;
            AppHostFallbackIcon.Visibility = Visibility.Collapsed;
            return;
        }

        AppHostIconImage.Source = null;
        AppHostIconImage.Visibility = Visibility.Collapsed;
        AppHostFallbackIcon.Visibility = Visibility.Visible;
    }

    private static string ResolveExistingIconPath(string? iconPath)
    {
        var preferred = HostAppIcon.ResolveDisplayIconPath(iconPath);
        if (!string.IsNullOrWhiteSpace(preferred) && File.Exists(preferred))
        {
            return preferred;
        }

        if (!string.IsNullOrWhiteSpace(iconPath) && File.Exists(iconPath))
        {
            return iconPath;
        }

        return Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico");
    }

    private static bool IsFallbackAppIcon(string iconPath)
    {
        var fallback = Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico");
        return string.Equals(
            Path.GetFullPath(iconPath),
            Path.GetFullPath(fallback),
            StringComparison.OrdinalIgnoreCase);
    }

    private void ShowHostWarning(string title, string message)
    {
        AppHostInfoBar.Title = title;
        AppHostInfoBar.Message = message;
        AppHostInfoBar.Severity = InfoBarSeverity.Warning;
        AppHostInfoBar.IsOpen = true;
    }

    internal void ShowHostedAppWarning(string title, string message) =>
        ShowHostWarning(title, message);

    private void StartHostHealthMonitor()
    {
        if (_hostedAppMetadata is null)
        {
            return;
        }

        if (_hostHealthTimer is null)
        {
            _hostHealthTimer = DispatcherQueue.CreateTimer();
            _hostHealthTimer.Interval = AppHostHealthCheckInterval;
            _hostHealthTimer.IsRepeating = true;
            _hostHealthTimer.Tick += HostHealthTimer_Tick;
        }

        _hostHealthTimer.Start();
    }

    private void StopHostHealthMonitor(bool permanently)
    {
        _hostHealthTimer?.Stop();
        _hostRecoveryCancellation?.Cancel();
        if (permanently && _hostHealthTimer is not null)
        {
            _hostHealthTimer.Tick -= HostHealthTimer_Tick;
            _hostHealthTimer = null;
        }
    }

    private async void HostHealthTimer_Tick(
        Microsoft.UI.Dispatching.DispatcherQueueTimer sender,
        object args)
    {
        await CheckHostedWindowHealthAsync();
    }

    private async Task CheckHostedWindowHealthAsync()
    {
        var target = _hostedAppWindow;
        var metadata = _hostedAppMetadata;
        if (metadata is not null)
        {
            LatencyPriorityService.Refresh(metadata.WindowProcessId);
        }
        var isHealthy = target is not null
            && App.AppWindows.IsHostedWindowAttachedTo(target, _windowHandle);
        if (metadata is null || !_hostRecoveryGate.TryBegin(isHealthy))
        {
            return;
        }

        var cancellation = new CancellationTokenSource();
        _hostRecoveryCancellation = cancellation;
        AppHostInfoBar.Title = "应用画面正在恢复";
        AppHostInfoBar.Message = "检测到 scrcpy 进程或窗口句柄已失效，正在重新启动并嵌入应用画面…";
        AppHostInfoBar.Severity = InfoBarSeverity.Informational;
        AppHostInfoBar.IsOpen = true;

        try
        {
            var result = await SimulatorCtlService.LaunchAppAsync(
                metadata.PackageName,
                metadata.Title,
                cancellation.Token);
            cancellation.Token.ThrowIfCancellationRequested();
            if (!result.Success)
            {
                ShowHostRecoveryFailure(AppWindowService.DescribeLaunchFailure(result));
                return;
            }

            if (!AppWindowMetadata.TryParse(result.Data, out var replacement))
            {
                ShowHostRecoveryFailure("控制组件没有返回可信的独立应用窗口 metadata，宿主将在下一次健康检查时重试。");
                return;
            }

            if (!await TryEmbedRecoveredAppAsync(replacement, cancellation.Token))
            {
                ShowHostRecoveryFailure("新的 scrcpy 窗口尚未能嵌入 WinUI 宿主，宿主将在下一次健康检查时重试。");
                return;
            }

            AppHostInfoBar.Title = "应用画面已恢复";
            AppHostInfoBar.Message = $"{replacement.Title} 已重新启动并嵌入当前窗口。";
            AppHostInfoBar.Severity = InfoBarSeverity.Success;
            AppHostInfoBar.IsOpen = true;
        }
        catch (OperationCanceledException) when (cancellation.IsCancellationRequested)
        {
        }
        catch (Exception exception)
        {
            ShowHostRecoveryFailure(exception.Message);
        }
        finally
        {
            if (ReferenceEquals(_hostRecoveryCancellation, cancellation))
            {
                _hostRecoveryCancellation = null;
            }

            cancellation.Dispose();
            _hostRecoveryGate.Complete();
        }
    }

    private async Task<bool> TryEmbedRecoveredAppAsync(
        AppWindowMetadata metadata,
        CancellationToken cancellationToken)
    {
        for (var attempt = 0; attempt < 40; attempt++)
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (ShowAppHostWindow(metadata, out _))
            {
                return true;
            }

            await Task.Delay(50, cancellationToken);
        }

        return false;
    }

    private void ShowHostRecoveryFailure(string message)
    {
        AppHostInfoBar.Title = "应用画面恢复失败";
        AppHostInfoBar.Message = message;
        AppHostInfoBar.Severity = InfoBarSeverity.Error;
        AppHostInfoBar.IsOpen = true;
    }

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(nint windowHandle);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern nint SetWindowsHookEx(
        int hookId,
        LowLevelKeyboardProc hookProc,
        nint moduleHandle,
        uint threadId);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnhookWindowsHookEx(nint hookHandle);

    [DllImport("user32.dll")]
    private static extern nint CallNextHookEx(
        nint hookHandle,
        int code,
        nint wParam,
        nint lParam);

    [DllImport("user32.dll")]
    private static extern nint GetForegroundWindow();

    [DllImport("user32.dll")]
    private static extern nint GetAncestor(nint windowHandle, uint flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetClientRect(nint windowHandle, out NativeRect clientRect);

    [DllImport("user32.dll")]
    private static extern nint MonitorFromWindow(nint windowHandle, uint flags);

    [DllImport("shcore.dll")]
    private static extern int GetScaleFactorForMonitor(nint monitorHandle, out int scalePercent);

    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(nint monitorHandle, ref MonitorInfo monitorInfo);

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MonitorInfo
    {
        public uint Size;
        public NativeRect MonitorArea;
        public NativeRect WorkArea;
        public uint Flags;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct LowLevelKeyboardInput
    {
        public uint VirtualKey;
        public uint ScanCode;
        public uint Flags;
        public uint Time;
        public nuint ExtraInfo;
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate nint LowLevelKeyboardProc(
        int code,
        nint wParam,
        nint lParam);
}
