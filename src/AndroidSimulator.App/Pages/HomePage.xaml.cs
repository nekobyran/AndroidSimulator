using AndroidSimulator.App.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace AndroidSimulator.App.Pages;

public sealed partial class HomePage : Page
{
    private static bool _imagePromptShownThisSession;

    private CancellationTokenSource? _refreshCancellation;
    private CancellationTokenSource? _provisionCancellation;
    private bool _runtimeRunning;
    private bool _imageReady;
    private bool _imagePromptInFlight;

    public HomePage()
    {
        InitializeComponent();
    }

    private async void Page_Loaded(object sender, RoutedEventArgs e)
    {
        BackdropText.Text = App.MainWindow?.BackdropDescription ?? "系统自适应";
        if (!string.IsNullOrWhiteSpace(App.StartupError))
        {
            ShowStatus(InfoBarSeverity.Warning, "键位服务未启动", App.StartupError!);
        }

        await RefreshAsync();
        await MaybePromptImageDownloadAsync();
    }

    private void Page_Unloaded(object sender, RoutedEventArgs e)
    {
        _refreshCancellation?.Cancel();
        _refreshCancellation?.Dispose();
        _refreshCancellation = null;
        _provisionCancellation?.Cancel();
        _provisionCancellation?.Dispose();
        _provisionCancellation = null;
    }

    private async void RefreshButton_Click(object sender, RoutedEventArgs e)
    {
        await RefreshAsync();
        await MaybePromptImageDownloadAsync();
    }

    private async void StartRuntimeButton_Click(object sender, RoutedEventArgs e)
    {
        if (!_imageReady)
        {
            await MaybePromptImageDownloadAsync(force: true);
            if (!_imageReady)
            {
                ShowStatus(
                    InfoBarSeverity.Warning,
                    "系统镜像未就绪",
                    "请先下载并准备 BlissOS 镜像后再启动后台 Android。");
                return;
            }
        }

        SetBusy(true, "正在启动…");
        try
        {
            var result = await SimulatorCtlService.RunAsync("owned", "launch", "--instance", "android-simulator");
            if (!result.Success)
            {
                ShowStatus(InfoBarSeverity.Error, "启动失败", result.Message);
                return;
            }

            ShowStatus(InfoBarSeverity.Success, "后台 Android 已启动", result.Message);
            await RefreshAsync();
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "启动失败", exception.Message);
        }
        finally
        {
            SetBusy(false, _runtimeRunning ? "已在后台运行" : "启动后台 Android");
        }
    }

    private void OpenAppsButton_Click(object sender, RoutedEventArgs e)
    {
        App.MainWindow?.NavigateTo("apps");
    }

    private void OpenKeyMappingsButton_Click(object sender, RoutedEventArgs e)
    {
        App.MainWindow?.NavigateTo("key-mappings");
    }

    private async Task RefreshAsync()
    {
        _refreshCancellation?.Cancel();
        _refreshCancellation?.Dispose();
        _refreshCancellation = new CancellationTokenSource();
        var cancellationToken = _refreshCancellation.Token;
        RefreshButton.IsEnabled = false;
        ControlStatusText.Text = "正在检查控制组件";
        ControlStatusDetail.Text = "请稍候…";
        InstalledAppsText.Text = "—";

        try
        {
            var simulatorCtlPath = SimulatorCtlService.ResolveSimulatorCtl();
            if (!File.Exists(simulatorCtlPath))
            {
                ControlStatusText.Text = "控制组件不可用";
                ControlStatusDetail.Text = $"未找到 simulatorctl：{simulatorCtlPath}";
                _imageReady = false;
                return;
            }

            ControlStatusText.Text = "控制组件已就绪";
            ControlStatusDetail.Text = simulatorCtlPath;

            var runtimeResult = await SimulatorCtlService.RunAsync(
                cancellationToken,
                "owned",
                "status",
                "--instance",
                "android-simulator");
            _runtimeRunning = runtimeResult.Success
                && runtimeResult.Data is { } data
                && data.TryGetProperty("running", out var running)
                && running.ValueKind is System.Text.Json.JsonValueKind.True or System.Text.Json.JsonValueKind.False
                && running.GetBoolean();
            InstalledAppsText.Text = _runtimeRunning ? "运行中" : "未运行";
            StartRuntimeButtonLabel.Text = _runtimeRunning ? "已在后台运行" : "启动后台 Android";
            StartRuntimeButton.IsEnabled = !_runtimeRunning;

            var imageResult = await SimulatorCtlService.ImageCheckAsync(cancellationToken);
            _imageReady = SimulatorCtlService.IsImageReady(imageResult);
            if (!_imageReady)
            {
                ControlStatusText.Text = "系统镜像未就绪";
                ControlStatusDetail.Text = string.IsNullOrWhiteSpace(imageResult.Message)
                    ? "未检测到已校验的 BlissOS 镜像，可一键下载并准备。"
                    : imageResult.Message;
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception)
        {
            ControlStatusText.Text = "状态检查失败";
            ControlStatusDetail.Text = exception.Message;
        }
        finally
        {
            RefreshButton.IsEnabled = true;
        }
    }

    private async Task MaybePromptImageDownloadAsync(bool force = false)
    {
        if (_imageReady || _imagePromptInFlight)
        {
            return;
        }

        if (!force && _imagePromptShownThisSession)
        {
            return;
        }

        if (XamlRoot is null)
        {
            return;
        }

        _imagePromptInFlight = true;
        _imagePromptShownThisSession = true;
        try
        {
            ImageDownloadDialog.XamlRoot = XamlRoot;
            var result = await ImageDownloadDialog.ShowAsync();
            if (result == ContentDialogResult.Primary)
            {
                await ProvisionImageAsync();
            }
            else
            {
                ShowStatus(
                    InfoBarSeverity.Informational,
                    "可稍后下载镜像",
                    "需要启动后台 Android 时会再次提示下载并准备镜像。");
            }
        }
        finally
        {
            _imagePromptInFlight = false;
        }
    }

    private async Task ProvisionImageAsync()
    {
        _provisionCancellation?.Cancel();
        _provisionCancellation?.Dispose();
        _provisionCancellation = new CancellationTokenSource();
        var cancellationToken = _provisionCancellation.Token;

        SetBusy(true, "正在下载镜像…");
        ShowStatus(
            InfoBarSeverity.Informational,
            "正在下载并准备镜像",
            "将下载 BlissOS ISO（约 2 GB）并校验、提取启动文件，请保持网络畅通。");

        try
        {
            var result = await SimulatorCtlService.ProvisionOwnedRuntimeAsync(cancellationToken);
            if (!result.Success)
            {
                ShowStatus(InfoBarSeverity.Error, "镜像准备失败", result.Message);
                return;
            }

            _imageReady = true;
            ShowStatus(
                InfoBarSeverity.Success,
                "镜像已就绪",
                string.IsNullOrWhiteSpace(result.Message)
                    ? "BlissOS 运行时已下载并准备完成，可启动后台 Android。"
                    : result.Message);
            await RefreshAsync();
        }
        catch (OperationCanceledException)
        {
            ShowStatus(InfoBarSeverity.Warning, "镜像准备已取消", "下载或准备过程已中断。");
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "镜像准备失败", exception.Message);
        }
        finally
        {
            SetBusy(false, _runtimeRunning ? "已在后台运行" : "启动后台 Android");
        }
    }

    private void SetBusy(bool isBusy, string label)
    {
        StartRuntimeButton.IsEnabled = !isBusy && !_runtimeRunning;
        StartRuntimeButtonLabel.Text = label;
        StartRuntimeProgressRing.IsActive = isBusy;
        StartRuntimeProgressRing.Visibility = isBusy ? Visibility.Visible : Visibility.Collapsed;
        RefreshButton.IsEnabled = !isBusy;
    }

    private void ShowStatus(InfoBarSeverity severity, string title, string message)
    {
        StatusInfoBar.Severity = severity;
        StatusInfoBar.Title = title;
        StatusInfoBar.Message = message;
        StatusInfoBar.IsOpen = true;
    }
}
