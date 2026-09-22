using AndroidSimulator.App.Models;
using AndroidSimulator.App.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Storage.Pickers;

namespace AndroidSimulator.App.Pages;

public sealed partial class AppsPage : Page
{
    private static readonly TimeSpan AutomaticRefreshInterval = TimeSpan.FromSeconds(30);

    private IReadOnlyList<AndroidAppInfo> _apps = Array.Empty<AndroidAppInfo>();
    private CancellationTokenSource? _operationCancellation;
    private CancellationTokenSource? _provisionCancellation;
    private DateTimeOffset _lastLiveRefresh = DateTimeOffset.MinValue;
    private bool _runtimeRunning;
    private bool _imageReady;
    private bool _imagePromptInFlight;

    public AppsPage()
    {
        InitializeComponent();
        AppsRepeater.ItemsSource = _apps;
    }

    private async void Page_Loaded(object sender, RoutedEventArgs e)
    {
        if (_apps.Count == 0)
        {
            var operation = BeginOperation();
            var cachedApps = await SimulatorCtlService.ReadCachedAppsAsync(operation.Token);
            if (cachedApps.Count > 0)
            {
                ApplyApps(cachedApps);
                SetListState(AppListState.Content);
            }
        }

        await RefreshRuntimeStateAsync();
        if (_runtimeRunning
            && DateTimeOffset.UtcNow - _lastLiveRefresh >= AutomaticRefreshInterval)
        {
            await RefreshAppsAsync(forceRefresh: true, showLoading: _apps.Count == 0);
        }
        else
        {
            SetListState(_apps.Count == 0 ? AppListState.Empty : AppListState.Content);
        }
    }

    private void Page_Unloaded(object sender, RoutedEventArgs e)
    {
        _operationCancellation?.Cancel();
        _operationCancellation?.Dispose();
        _operationCancellation = null;
        _provisionCancellation?.Cancel();
        _provisionCancellation?.Dispose();
        _provisionCancellation = null;
    }

    private async void RefreshButton_Click(object sender, RoutedEventArgs e)
    {
        await RefreshRuntimeStateAsync();
        if (_runtimeRunning)
        {
            await RefreshAppsAsync(forceRefresh: true, showLoading: false);
            return;
        }

        SetListState(_apps.Count == 0 ? AppListState.Empty : AppListState.Content);
        ShowStatus(
            InfoBarSeverity.Informational,
            "后台 Android 未运行",
            "点击右下角的启动按钮后即可刷新已安装应用。");
    }

    private void SettingsButton_Click(object sender, RoutedEventArgs e)
    {
        App.MainWindow?.NavigateTo("settings");
    }

    private async void RuntimeButton_Click(object sender, RoutedEventArgs e)
    {
        if (_runtimeRunning)
        {
            return;
        }

        if (!_imageReady && !await EnsureImageReadyAsync())
        {
            SetRuntimeButtonState(RuntimeButtonState.Idle);
            return;
        }

        SetRuntimeButtonState(RuntimeButtonState.Starting);
        try
        {
            var result = await SimulatorCtlService.RunAsync(
                "owned",
                "launch",
                "--instance",
                "android-simulator");
            if (!result.Success)
            {
                ShowStatus(InfoBarSeverity.Error, "启动失败", result.Message);
                SetRuntimeButtonState(RuntimeButtonState.Idle);
                return;
            }

            _runtimeRunning = true;
            SetRuntimeButtonState(RuntimeButtonState.Running);
            ShowStatus(InfoBarSeverity.Success, "后台 Android 已启动", result.Message);
            await RefreshAppsAsync(forceRefresh: true, showLoading: _apps.Count == 0);
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "启动失败", exception.Message);
            SetRuntimeButtonState(RuntimeButtonState.Idle);
        }
    }

    private async void ImportButton_Click(object sender, RoutedEventArgs e)
    {
        var mainWindow = App.MainWindow;
        if (mainWindow is null)
        {
            ShowStatus(InfoBarSeverity.Error, "无法打开文件选择器", "主窗口尚未准备完成。");
            return;
        }

        try
        {
            var picker = new FileOpenPicker
            {
                SuggestedStartLocation = PickerLocationId.Downloads,
                ViewMode = PickerViewMode.List,
            };
            picker.FileTypeFilter.Add(".apk");
            var windowHandle = WinRT.Interop.WindowNative.GetWindowHandle(mainWindow);
            WinRT.Interop.InitializeWithWindow.Initialize(picker, windowHandle);
            var file = await picker.PickSingleFileAsync();
            if (file is null)
            {
                return;
            }

            var operation = BeginOperation();
            SetOperationBusy(true);
            var result = await SimulatorCtlService.InstallApkAsync(
                file.Path,
                launch: false,
                title: null,
                operation.Token);
            if (!result.Success)
            {
                ShowStatus(InfoBarSeverity.Error, "安装失败", result.Message);
                return;
            }

            ShowStatus(InfoBarSeverity.Success, "APK 已安装", string.IsNullOrWhiteSpace(result.Message) ? file.Name : result.Message);
            await RefreshAppsAsync(forceRefresh: true, showLoading: false);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "安装失败", exception.Message);
        }
        finally
        {
            SetOperationBusy(false);
        }
    }

    private void LaunchAppButton_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not AndroidAppInfo app)
        {
            return;
        }

        if (!DesktopAppHostService.TryLaunch(
                app.PackageName,
                app.Name,
                out _,
                out var error))
        {
            ShowStatus(InfoBarSeverity.Error, $"无法打开 {app.Name}", error);
        }
    }

    private async void CreateShortcutButton_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not AndroidAppInfo app)
        {
            return;
        }

        var operation = BeginOperation();
        SetOperationBusy(true);
        try
        {
            var result = await SimulatorCtlService.CreateShortcutAsync(app.PackageName, app.Name, operation.Token);
            ShowStatus(
                result.Success ? InfoBarSeverity.Success : InfoBarSeverity.Error,
                result.Success ? "桌面快捷方式已创建" : "快捷方式创建失败",
                result.Message);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "快捷方式创建失败", exception.Message);
        }
        finally
        {
            SetOperationBusy(false);
        }
    }

    private async void RenameAppMenuItem_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not AndroidAppInfo app)
        {
            return;
        }

        var nameBox = new TextBox
        {
            Text = app.Name,
            MaxLength = 80,
            SelectionStart = app.Name.Length,
        };
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "重命名应用",
            Content = nameBox,
            PrimaryButtonText = "保存",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary
            || string.IsNullOrWhiteSpace(nameBox.Text))
        {
            return;
        }

        var operation = BeginOperation();
        SetOperationBusy(true);
        try
        {
            var result = await SimulatorCtlService.RenameAppAsync(
                app.PackageName,
                nameBox.Text,
                operation.Token);
            ShowStatus(
                result.Success ? InfoBarSeverity.Success : InfoBarSeverity.Error,
                result.Success ? "应用已重命名" : "重命名失败",
                result.Message);
            if (result.Success)
            {
                await RefreshAppsAsync(forceRefresh: true, showLoading: false);
            }
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "重命名失败", exception.Message);
        }
        finally
        {
            SetOperationBusy(false);
        }
    }

    private async void UninstallAppMenuItem_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not AndroidAppInfo app)
        {
            return;
        }

        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = $"卸载 {app.Name}？",
            Content = $"将从后台 Android 中移除 {app.PackageName}。此操作无法撤销。",
            PrimaryButtonText = "卸载",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Close,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary)
        {
            return;
        }

        var operation = BeginOperation();
        SetOperationBusy(true);
        try
        {
            var result = await SimulatorCtlService.UninstallAppAsync(app.PackageName, operation.Token);
            ShowStatus(
                result.Success ? InfoBarSeverity.Success : InfoBarSeverity.Error,
                result.Success ? "应用已卸载" : "卸载失败",
                result.Message);
            if (result.Success)
            {
                await RefreshAppsAsync(forceRefresh: true, showLoading: false);
            }
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "卸载失败", exception.Message);
        }
        finally
        {
            SetOperationBusy(false);
        }
    }

    private async Task RefreshAppsAsync(bool forceRefresh, bool showLoading)
    {
        var operation = BeginOperation();
        if (showLoading)
        {
            SetListState(AppListState.Loading);
        }
        SetRefreshBusy(true);

        try
        {
            var result = await SimulatorCtlService.ListAppsAsync(
                startRuntime: false,
                forceRefresh: forceRefresh,
                cancellationToken: operation.Token);
            if (!result.Success)
            {
                if (_apps.Count == 0)
                {
                    SetListState(AppListState.Error, result.Message);
                }
                else
                {
                    SetListState(AppListState.Content);
                }
                ShowStatus(InfoBarSeverity.Error, "无法读取应用列表", result.Message);
                return;
            }

            ApplyApps(result.Data ?? Array.Empty<AndroidAppInfo>());
            _lastLiveRefresh = DateTimeOffset.UtcNow;
            SetListState(_apps.Count == 0 ? AppListState.Empty : AppListState.Content);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception)
        {
            if (_apps.Count == 0)
            {
                SetListState(AppListState.Error, exception.Message);
            }
            else
            {
                SetListState(AppListState.Content);
            }
            ShowStatus(InfoBarSeverity.Error, "无法读取应用列表", exception.Message);
        }
        finally
        {
            SetRefreshBusy(false);
        }
    }

    private void ApplyApps(IEnumerable<AndroidAppInfo> apps)
    {
        _apps = apps
            .Where(app => !string.IsNullOrWhiteSpace(app.PackageName))
            .Where(IsDesktopUsableApp)
            .Select(CreateListItem)
            .ToArray();
        AppsRepeater.ItemsSource = _apps;
    }

    private static bool IsDesktopUsableApp(AndroidAppInfo app)
    {
        var package = app.PackageName.ToLowerInvariant();
        var name = app.Name.ToLowerInvariant();
        var cameraPackage = package.Contains("opencamera", StringComparison.Ordinal)
            || package.Contains(".camera", StringComparison.Ordinal)
            || package.StartsWith("camera.", StringComparison.Ordinal)
            || package.EndsWith(".camera", StringComparison.Ordinal);
        var cameraName = name.Contains("camera", StringComparison.Ordinal)
            || name.Contains("相机", StringComparison.Ordinal)
            || name.Contains("摄像机", StringComparison.Ordinal);
        return !cameraPackage && !cameraName;
    }

    private static AndroidAppInfo CreateListItem(AndroidAppInfo app)
    {
        BitmapImage? iconImage = null;
        string? resolvedIconPath = null;
        if (app.HasIcon && !string.IsNullOrWhiteSpace(app.IconPath))
        {
            try
            {
                resolvedIconPath = Path.GetFullPath(app.IconPath);
                iconImage = new BitmapImage
                {
                    DecodePixelWidth = 48,
                    UriSource = new Uri(resolvedIconPath),
                };
            }
            catch
            {
                iconImage = null;
                resolvedIconPath = null;
            }
        }

        var hasIcon = iconImage is not null;
        return new AndroidAppInfo
        {
            PackageName = app.PackageName,
            Name = app.Name,
            ActivityName = app.ActivityName,
            Version = string.IsNullOrWhiteSpace(app.Version) ? "版本未知" : app.Version,
            IconPath = resolvedIconPath,
            HasIcon = hasIcon,
            IconImage = iconImage,
        };
    }

    private CancellationTokenSource BeginOperation()
    {
        _operationCancellation?.Cancel();
        _operationCancellation?.Dispose();
        _operationCancellation = new CancellationTokenSource();
        return _operationCancellation;
    }

    private void SetRefreshBusy(bool isBusy)
    {
        OperationProgressRing.IsActive = isBusy;
        OperationProgressRing.Visibility = isBusy ? Visibility.Visible : Visibility.Collapsed;
        RefreshButton.IsEnabled = !isBusy;
    }

    private void SetOperationBusy(bool isBusy)
    {
        OperationProgressRing.IsActive = isBusy;
        OperationProgressRing.Visibility = isBusy ? Visibility.Visible : Visibility.Collapsed;
        ImportButton.IsEnabled = !isBusy;
        RefreshButton.IsEnabled = !isBusy;
        AppsList.IsEnabled = !isBusy;
    }

    private async Task RefreshRuntimeStateAsync()
    {
        var operation = BeginOperation();
        SetRuntimeButtonState(RuntimeButtonState.Checking);
        try
        {
            var runtimeResult = await SimulatorCtlService.RunAsync(
                operation.Token,
                "owned",
                "status",
                "--instance",
                "android-simulator");
            _runtimeRunning = runtimeResult.Success
                && runtimeResult.Data is { } data
                && data.TryGetProperty("running", out var running)
                && running.ValueKind is System.Text.Json.JsonValueKind.True or System.Text.Json.JsonValueKind.False
                && running.GetBoolean();

            var imageResult = await SimulatorCtlService.ImageCheckAsync(operation.Token);
            _imageReady = SimulatorCtlService.IsImageReady(imageResult);
            SetRuntimeButtonState(
                _runtimeRunning
                    ? RuntimeButtonState.Running
                    : RuntimeButtonState.Idle);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception)
        {
            _runtimeRunning = false;
            SetRuntimeButtonState(RuntimeButtonState.Idle);
            ShowStatus(InfoBarSeverity.Error, "无法读取后台状态", exception.Message);
        }
    }

    private async Task<bool> EnsureImageReadyAsync()
    {
        if (_imageReady)
        {
            return true;
        }

        if (_imagePromptInFlight || XamlRoot is null)
        {
            return false;
        }

        _imagePromptInFlight = true;
        try
        {
            ImageDownloadDialog.XamlRoot = XamlRoot;
            var dialogResult = await ImageDownloadDialog.ShowAsync();
            if (dialogResult != ContentDialogResult.Primary)
            {
                ShowStatus(
                    InfoBarSeverity.Informational,
                    "可稍后下载镜像",
                    "需要启动后台 Android 时可再次点击右下角按钮。");
                return false;
            }

            _provisionCancellation?.Cancel();
            _provisionCancellation?.Dispose();
            _provisionCancellation = new CancellationTokenSource();
            SetRuntimeButtonState(RuntimeButtonState.Starting);
            ShowStatus(
                InfoBarSeverity.Informational,
                "正在下载并准备镜像",
                "将下载 BlissOS ISO（约 2 GB）并校验、提取启动文件，请保持网络畅通。");

            var result = await SimulatorCtlService.ProvisionOwnedRuntimeAsync(
                _provisionCancellation.Token);
            if (!result.Success)
            {
                ShowStatus(InfoBarSeverity.Error, "镜像准备失败", result.Message);
                return false;
            }

            _imageReady = true;
            ShowStatus(
                InfoBarSeverity.Success,
                "镜像已就绪",
                string.IsNullOrWhiteSpace(result.Message)
                    ? "BlissOS 运行时已下载并准备完成。"
                    : result.Message);
            return true;
        }
        catch (OperationCanceledException)
        {
            ShowStatus(InfoBarSeverity.Warning, "镜像准备已取消", "下载或准备过程已中断。");
            return false;
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "镜像准备失败", exception.Message);
            return false;
        }
        finally
        {
            _imagePromptInFlight = false;
        }
    }

    private void SetRuntimeButtonState(RuntimeButtonState state)
    {
        var busy = state is RuntimeButtonState.Checking or RuntimeButtonState.Starting;
        RuntimeProgressRing.IsActive = busy;
        RuntimeProgressRing.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        RuntimeStateIcon.Visibility = busy ? Visibility.Collapsed : Visibility.Visible;
        RuntimeStateIcon.Glyph = state == RuntimeButtonState.Running ? "\uE73E" : "\uE768";
        RuntimeButton.IsEnabled = state == RuntimeButtonState.Idle;

        var label = state switch
        {
            RuntimeButtonState.Checking => "正在检查后台 Android",
            RuntimeButtonState.Starting => "正在启动后台 Android",
            RuntimeButtonState.Running => "后台 Android 正在运行",
            _ => "启动后台 Android",
        };
        AutomationProperties.SetName(RuntimeButton, label);
        ToolTipService.SetToolTip(RuntimeButton, label);
    }

    private void SetListState(AppListState state, string? error = null)
    {
        AppsList.Visibility = state == AppListState.Content ? Visibility.Visible : Visibility.Collapsed;
        LoadingPanel.Visibility = state == AppListState.Loading ? Visibility.Visible : Visibility.Collapsed;
        EmptyPanel.Visibility = state == AppListState.Empty ? Visibility.Visible : Visibility.Collapsed;
        ErrorPanel.Visibility = state == AppListState.Error ? Visibility.Visible : Visibility.Collapsed;
        ErrorMessageText.Text = error ?? string.Empty;
    }

    private void ShowStatus(InfoBarSeverity severity, string title, string message)
    {
        StatusInfoBar.Severity = severity;
        StatusInfoBar.Title = title;
        StatusInfoBar.Message = message;
        StatusInfoBar.IsOpen = true;
    }

    private enum AppListState
    {
        Loading,
        Content,
        Empty,
        Error,
    }

    private enum RuntimeButtonState
    {
        Checking,
        Idle,
        Starting,
        Running,
    }
}
