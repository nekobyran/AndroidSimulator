using AndroidSimulator.App.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace AndroidSimulator.App.Pages;

public sealed partial class SettingsPage : Page
{
    private bool _isLoaded;
    private bool _isApplyingRuntimeSettings;
    private SimulatorRuntimeSettings _runtimeSettings = SimulatorRuntimeSettings.Defaults;

    public SettingsPage()
    {
        InitializeComponent();
    }

    private void BackButton_Click(object sender, RoutedEventArgs e)
    {
        App.MainWindow?.NavigateTo("apps");
    }

    private void OpenKeyMappingsButton_Click(object sender, RoutedEventArgs e)
    {
        App.MainWindow?.NavigateTo("key-mappings");
    }

    private async void Page_Loaded(object sender, RoutedEventArgs e)
    {
        BackdropValueText.Text = App.MainWindow?.BackdropDescription ?? "系统自适应";
        var simulatorCtlPath = SimulatorCtlService.ResolveSimulatorCtl();
        SimulatorCtlPathText.Text = simulatorCtlPath;
        SimulatorCtlStateText.Text = File.Exists(simulatorCtlPath) ? "已就绪" : "未找到";
        ScrcpyPathText.Text = TrustedAppWindowProcess.ExecutablePath;
        ScrcpyStateText.Text = File.Exists(TrustedAppWindowProcess.ExecutablePath)
            ? "已就绪"
            : "未找到";

        await RefreshRuntimeSettingsAsync();
        await RefreshApkAssociationAsync();
        _isLoaded = true;
    }

    private async Task RefreshRuntimeSettingsAsync()
    {
        SetRuntimeSettingsBusy(true);
        try
        {
            var result = await SimulatorCtlService.GetSettingsAsync();
            if (!result.Success)
            {
                ShowRuntimeSettingsMessage(
                    InfoBarSeverity.Error,
                    "无法读取模拟器设置",
                    result.Message);
                return;
            }

            _runtimeSettings = result.Data;
            ApplyRuntimeSettingsToUi(_runtimeSettings);
        }
        catch (Exception exception)
        {
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Error,
                "无法读取模拟器设置",
                exception.Message);
        }
        finally
        {
            SetRuntimeSettingsBusy(false);
        }
    }

    private void ApplyRuntimeSettingsToUi(SimulatorRuntimeSettings settings)
    {
        _isApplyingRuntimeSettings = true;
        try
        {
            SelectComboBoxTag(PerformanceModeComboBox, settings.PerformanceMode);
            CpuCoresNumberBox.Value = settings.CpuCores;
            MemoryMbNumberBox.Value = settings.MemoryMb;

            SelectComboBoxTag(RendererModeComboBox, settings.RendererMode);
            SelectComboBoxTag(GraphicsStrategyComboBox, settings.GraphicsStrategy);
            SelectComboBoxTag(ResolutionModeComboBox, settings.ResolutionMode);
            CustomWidthNumberBox.Value = settings.CustomWidth;
            CustomHeightNumberBox.Value = settings.CustomHeight;
            CustomDpiNumberBox.Value = settings.CustomDpi;
            MaxFrameRateNumberBox.Value = settings.MaxFrameRate;
            DynamicFrameRateToggle.IsOn = settings.DynamicFrameRate;
            DynamicLowFrameRateNumberBox.Value = settings.DynamicLowFrameRate;
            SuperResolutionToggle.IsOn = settings.SuperResolution;
            SuperResolutionScaleNumberBox.Value = settings.SuperResolutionScalePercent;
            FrameInterpolationToggle.IsOn = settings.FrameInterpolation;
            VerticalSyncToggle.IsOn = settings.VerticalSync;
            FixedWindowSizeToggle.IsOn = settings.FixedWindowSize;
            SystemAudioToggle.IsOn = settings.SystemAudio;
            KeepAppAliveToggle.IsOn = settings.KeepAppAlive;
            RememberWindowPositionToggle.IsOn = settings.RememberWindowPosition;
            AutoRotateToggle.IsOn = settings.AutoRotate;
            QuitConfirmToggle.IsOn = settings.QuitConfirm;
            UpdateRuntimeSettingsControlState();
        }
        finally
        {
            _isApplyingRuntimeSettings = false;
        }
    }

    private SimulatorRuntimeSettings ReadRuntimeSettingsFromUi()
    {
        var maxFrameRate = ReadUInt(MaxFrameRateNumberBox, 30, 240);
        var frameInterpolation = FrameInterpolationToggle.IsOn && maxFrameRate > 60;
        return _runtimeSettings with
        {
            PerformanceMode = SelectedComboBoxTag(PerformanceModeComboBox, "balanced"),
            CpuCores = checked((byte)ReadUInt(CpuCoresNumberBox, 2, 12)),
            MemoryMb = ReadUInt(MemoryMbNumberBox, 3072, 16384),
            RendererMode = SelectedComboBoxTag(RendererModeComboBox, "direct3d"),
            GraphicsStrategy = SelectedComboBoxTag(GraphicsStrategyComboBox, "balanced"),
            ResolutionMode = SelectedComboBoxTag(ResolutionModeComboBox, "adaptive"),
            CustomWidth = ReadUInt(CustomWidthNumberBox, 640, 3840),
            CustomHeight = ReadUInt(CustomHeightNumberBox, 360, 2160),
            CustomDpi = ReadUInt(CustomDpiNumberBox, 120, 640),
            MaxFrameRate = maxFrameRate,
            DynamicFrameRate = DynamicFrameRateToggle.IsOn,
            DynamicLowFrameRate = ReadUInt(DynamicLowFrameRateNumberBox, 10, 60),
            SuperResolution = SuperResolutionToggle.IsOn,
            SuperResolutionScalePercent = ReadUInt(SuperResolutionScaleNumberBox, 100, 200),
            FrameInterpolation = frameInterpolation,
            VerticalSync = VerticalSyncToggle.IsOn,
            FixedWindowSize = FixedWindowSizeToggle.IsOn,
            SystemAudio = SystemAudioToggle.IsOn,
            KeepAppAlive = KeepAppAliveToggle.IsOn,
            RememberWindowPosition = RememberWindowPositionToggle.IsOn,
            AutoRotate = AutoRotateToggle.IsOn,
            QuitConfirm = QuitConfirmToggle.IsOn,
        };
    }

    private async void SaveRuntimeSettingsButton_Click(object sender, RoutedEventArgs e)
    {
        RuntimeSettingsInfoBar.IsOpen = false;
        SimulatorRuntimeSettings requested;
        try
        {
            requested = ReadRuntimeSettingsFromUi();
        }
        catch (Exception exception)
        {
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Error,
                "设置值无效",
                exception.Message);
            return;
        }

        SetRuntimeSettingsBusy(true);
        try
        {
            var result = await SimulatorCtlService.SaveSettingsAsync(requested);
            if (!result.Success)
            {
                ShowRuntimeSettingsMessage(
                    InfoBarSeverity.Error,
                    "无法保存模拟器设置",
                    result.Message);
                return;
            }

            _runtimeSettings = result.Data;
            ApplyRuntimeSettingsToUi(_runtimeSettings);
            App.MainWindow?.RefreshRuntimeSettingsPolicy();
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Success,
                "设置已保存",
                "调度、窗口锁定与退出策略立即刷新；QEMU CPU/内存会在下次冷启动使用，新建或复用的应用窗口会应用显示、动态帧率、超分、插帧、VSync 与自动旋转设置。");
        }
        catch (Exception exception)
        {
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Error,
                "无法保存模拟器设置",
                exception.Message);
        }
        finally
        {
            SetRuntimeSettingsBusy(false);
        }
    }

    private async void ResetRuntimeSettingsButton_Click(object sender, RoutedEventArgs e)
    {
        RuntimeSettingsInfoBar.IsOpen = false;
        SetRuntimeSettingsBusy(true);
        try
        {
            var result = await SimulatorCtlService.ResetSettingsAsync();
            if (!result.Success)
            {
                ShowRuntimeSettingsMessage(
                    InfoBarSeverity.Error,
                    "无法恢复默认设置",
                    result.Message);
                return;
            }

            _runtimeSettings = result.Data;
            ApplyRuntimeSettingsToUi(_runtimeSettings);
            App.MainWindow?.RefreshRuntimeSettingsPolicy();
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Success,
                "已恢复默认设置",
                "默认使用均衡 4 核 / 6 GB、60 FPS、Direct3D，超分与插帧默认关闭。");
        }
        catch (Exception exception)
        {
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Error,
                "无法恢复默认设置",
                exception.Message);
        }
        finally
        {
            SetRuntimeSettingsBusy(false);
        }
    }

    private async void BootstrapRuntimeButton_Click(object sender, RoutedEventArgs e)
    {
        BootstrapRuntimeButton.IsEnabled = false;
        BootstrapProgressRing.IsActive = true;
        BootstrapProgressRing.Visibility = Visibility.Visible;
        BootstrapRuntimeStateText.Text = "正在 Provision、启动 QEMU 并等待 Android 完成启动…";
        RuntimeSettingsInfoBar.IsOpen = false;

        try
        {
            var result = await SimulatorCtlService.BootstrapOwnedRuntimeAsync();
            if (!result.Success)
            {
                BootstrapRuntimeStateText.Text = "启动失败";
                ShowRuntimeSettingsMessage(
                    InfoBarSeverity.Error,
                    "Android 一键启动失败",
                    result.Message);
                return;
            }

            var elapsedText = string.Empty;
            if (result.Data is System.Text.Json.JsonElement data
                && data.TryGetProperty("elapsed_ms", out var elapsed)
                && elapsed.TryGetUInt64(out var elapsedMs))
            {
                elapsedText = $"，耗时 {elapsedMs / 1000.0:F1} 秒";
            }

            BootstrapRuntimeStateText.Text = $"Android 已完成启动{elapsedText}";
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Success,
                "Android 已就绪",
                "系统/工具链已完成 Provision，QEMU 已运行，ADB boot_completed 已确认。");
        }
        catch (Exception exception)
        {
            BootstrapRuntimeStateText.Text = "启动失败";
            ShowRuntimeSettingsMessage(
                InfoBarSeverity.Error,
                "Android 一键启动失败",
                exception.Message);
        }
        finally
        {
            BootstrapRuntimeButton.IsEnabled = true;
            BootstrapProgressRing.IsActive = false;
            BootstrapProgressRing.Visibility = Visibility.Collapsed;
        }
    }

    private void RuntimeSettingsControl_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_isApplyingRuntimeSettings)
        {
            return;
        }

        UpdateRuntimeSettingsControlState();
    }

    private void RuntimeSettingsToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (_isApplyingRuntimeSettings)
        {
            return;
        }

        UpdateRuntimeSettingsControlState();
    }

    private void RuntimeSettingsNumberBox_ValueChanged(object sender, NumberBoxValueChangedEventArgs e)
    {
        if (_isApplyingRuntimeSettings)
        {
            return;
        }

        UpdateRuntimeSettingsControlState();
    }

    private void UpdateRuntimeSettingsControlState()
    {
        var customPerformance = SelectedComboBoxTag(PerformanceModeComboBox, "balanced") == "custom";
        CpuCoresNumberBox.IsEnabled = customPerformance;
        MemoryMbNumberBox.IsEnabled = customPerformance;

        var customResolution = SelectedComboBoxTag(ResolutionModeComboBox, "adaptive") == "custom";
        CustomWidthNumberBox.IsEnabled = customResolution;
        CustomHeightNumberBox.IsEnabled = customResolution;
        CustomDpiNumberBox.IsEnabled = customResolution;

        SuperResolutionScaleNumberBox.IsEnabled = SuperResolutionToggle.IsOn;
        DynamicLowFrameRateNumberBox.IsEnabled = DynamicFrameRateToggle.IsOn;
        var highFrameRate = !double.IsNaN(MaxFrameRateNumberBox.Value)
            && MaxFrameRateNumberBox.Value > 60;
        FrameInterpolationToggle.IsEnabled = highFrameRate;
        if (!highFrameRate && FrameInterpolationToggle.IsOn)
        {
            _isApplyingRuntimeSettings = true;
            FrameInterpolationToggle.IsOn = false;
            _isApplyingRuntimeSettings = false;
        }

        var mode = SelectedComboBoxTag(PerformanceModeComboBox, "balanced");
        var (cores, memoryMb) = mode switch
        {
            "eco" => (2u, 3072u),
            "performance" => (6u, 8192u),
            "custom" => (
                ReadUIntOrDefault(CpuCoresNumberBox, 4),
                ReadUIntOrDefault(MemoryMbNumberBox, 6144)),
            _ => (4u, 6144u),
        };
        PerformanceSummaryText.Text = $"{cores} 核 · {memoryMb / 1024.0:F1} GB";
    }

    private void SetRuntimeSettingsBusy(bool isBusy)
    {
        SaveRuntimeSettingsButton.IsEnabled = !isBusy;
        ResetRuntimeSettingsButton.IsEnabled = !isBusy;
    }

    private void ShowRuntimeSettingsMessage(InfoBarSeverity severity, string title, string message)
    {
        RuntimeSettingsInfoBar.Severity = severity;
        RuntimeSettingsInfoBar.Title = title;
        RuntimeSettingsInfoBar.Message = message;
        RuntimeSettingsInfoBar.IsOpen = true;
    }

    private static uint ReadUInt(NumberBox numberBox, uint minimum, uint maximum)
    {
        if (double.IsNaN(numberBox.Value)
            || numberBox.Value < minimum
            || numberBox.Value > maximum)
        {
            throw new InvalidOperationException(
                $"{numberBox.Header ?? "数值"}必须在 {minimum}–{maximum} 之间。");
        }

        return checked((uint)Math.Round(numberBox.Value));
    }

    private static uint ReadUIntOrDefault(NumberBox numberBox, uint fallback)
        => double.IsNaN(numberBox.Value)
            ? fallback
            : checked((uint)Math.Round(numberBox.Value));

    private static string SelectedComboBoxTag(ComboBox comboBox, string fallback)
        => comboBox.SelectedItem is ComboBoxItem item
            && item.Tag is not null
            && !string.IsNullOrWhiteSpace(item.Tag.ToString())
            ? item.Tag.ToString()!
            : fallback;

    private static void SelectComboBoxTag(ComboBox comboBox, string tag)
    {
        foreach (var item in comboBox.Items.OfType<ComboBoxItem>())
        {
            if (string.Equals(item.Tag?.ToString(), tag, StringComparison.OrdinalIgnoreCase))
            {
                comboBox.SelectedItem = item;
                return;
            }
        }

        comboBox.SelectedIndex = 0;
    }

    private async void RepairApkAssociationButton_Click(object sender, RoutedEventArgs e)
    {
        SetAssociationBusy(true, "正在修复…");
        AssociationInfoBar.IsOpen = false;
        try
        {
            var result = await SimulatorCtlService.RegisterApkAssociationAsync();
            AssociationInfoBar.Severity = result.Success
                ? InfoBarSeverity.Success
                : InfoBarSeverity.Error;
            AssociationInfoBar.Title = result.Success ? "APK 关联已修复" : "无法修复 APK 关联";
            AssociationInfoBar.Message = result.Success
                ? "以后双击 APK 会直接进入后台安装与快捷方式创建流程。"
                : result.Message;
            AssociationInfoBar.IsOpen = true;
            await RefreshApkAssociationAsync();
        }
        catch (Exception exception)
        {
            ApkAssociationStateText.Text = "检查失败";
            AssociationInfoBar.Severity = InfoBarSeverity.Error;
            AssociationInfoBar.Title = "无法修复 APK 关联";
            AssociationInfoBar.Message = exception.Message;
            AssociationInfoBar.IsOpen = true;
        }
        finally
        {
            SetAssociationBusy(false, "修复关联");
        }
    }

    private async Task RefreshApkAssociationAsync()
    {
        SetAssociationBusy(true, "检查中");
        try
        {
            var result = await SimulatorCtlService.GetApkAssociationStatusAsync();
            ApkAssociationStateText.Text = result.Success ? "已启用（当前用户）" : "未启用";
        }
        catch (Exception exception)
        {
            ApkAssociationStateText.Text = "检查失败";
            AssociationInfoBar.Severity = InfoBarSeverity.Error;
            AssociationInfoBar.Title = "无法检查 APK 关联";
            AssociationInfoBar.Message = exception.Message;
            AssociationInfoBar.IsOpen = true;
        }
        finally
        {
            SetAssociationBusy(false, "修复关联");
        }
    }

    private void SetAssociationBusy(bool isBusy, string buttonText)
    {
        RepairApkAssociationButton.IsEnabled = !isBusy;
        RepairApkAssociationButton.Content = buttonText;
        AssociationProgressRing.IsActive = isBusy;
        AssociationProgressRing.Visibility = isBusy ? Visibility.Visible : Visibility.Collapsed;
    }

    private void ThemeComboBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_isLoaded || ThemeComboBox.SelectedItem is not ComboBoxItem item)
        {
            return;
        }

        var theme = item.Tag?.ToString() switch
        {
            "light" => ElementTheme.Light,
            "dark" => ElementTheme.Dark,
            _ => ElementTheme.Default,
        };
        App.MainWindow?.ApplyRequestedTheme(theme);
        BackdropValueText.Text = App.MainWindow?.BackdropDescription ?? "系统自适应";
    }
}
