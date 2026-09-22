using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Animation;
using AndroidSimulator.Installer.Services;
using WinForms = System.Windows.Forms;
using WpfBrush = System.Windows.Media.Brush;
using WpfBrushes = System.Windows.Media.Brushes;
using WpfMessageBox = System.Windows.MessageBox;

namespace AndroidSimulator.Installer;

public partial class MainWindow : Window
{
    private enum WizardPage
    {
        Welcome,
        Options,
        Progress,
        Done,
    }

    private WizardPage _page = WizardPage.Welcome;
    private CancellationTokenSource? _installCts;
    private bool _installSucceeded;

    public MainWindow()
    {
        InitializeComponent();
        Loaded += MainWindow_Loaded;
        Closed += (_, _) =>
        {
            _installCts?.Cancel();
            _installCts?.Dispose();
        };
    }

    private void MainWindow_Loaded(object sender, RoutedEventArgs e)
    {
        if (TryFindResource("IntroStoryboard") is Storyboard intro)
        {
            intro.Begin(this, true);
        }

        InstallPathBox.Text = InstallService.DefaultInstallDirectory;
        PathHintText.Text = "建议使用用户目录，避免需要管理员权限。";
        PayloadStatusText.Text = InstallService.HasEmbeddedPayload()
            ? "安装包已内嵌 Release 应用负载，可直接安装。"
            : "警告：当前安装器未内嵌负载。请先运行 PackageInstaller 从 Release 打包。";
        PayloadStatusText.Foreground = InstallService.HasEmbeddedPayload()
            ? (WpfBrush)FindResource("SuccessBrush")
            : (WpfBrush)FindResource("DangerBrush");

        ShowPage(WizardPage.Welcome);
    }

    private void BrowseButton_Click(object sender, RoutedEventArgs e)
    {
        using var dialog = new WinForms.FolderBrowserDialog
        {
            Description = "选择 Android Simulator 安装目录",
            UseDescriptionForTitle = true,
            SelectedPath = System.IO.Directory.Exists(InstallPathBox.Text)
                ? InstallPathBox.Text
                : InstallService.DefaultInstallDirectory,
        };

        if (dialog.ShowDialog() == WinForms.DialogResult.OK && !string.IsNullOrWhiteSpace(dialog.SelectedPath))
        {
            InstallPathBox.Text = dialog.SelectedPath;
        }
    }

    private async void NextButton_Click(object sender, RoutedEventArgs e)
    {
        switch (_page)
        {
            case WizardPage.Welcome:
                if (!InstallService.HasEmbeddedPayload())
                {
                    WpfMessageBox.Show(
                        this,
                        "安装器未包含应用负载。\n请先运行：android-simulator-package-installer.cmd",
                        "无法继续",
                        MessageBoxButton.OK,
                        MessageBoxImage.Warning);
                    return;
                }

                ShowPage(WizardPage.Options);
                break;

            case WizardPage.Options:
                if (!ValidateOptions(out var error))
                {
                    WpfMessageBox.Show(this, error, "安装选项无效", MessageBoxButton.OK, MessageBoxImage.Warning);
                    return;
                }

                ShowPage(WizardPage.Progress);
                await RunInstallAsync();
                break;

            case WizardPage.Done:
                Close();
                break;
        }
    }

    private void BackButton_Click(object sender, RoutedEventArgs e)
    {
        if (_page == WizardPage.Options)
        {
            ShowPage(WizardPage.Welcome);
        }
    }

    private void CancelButton_Click(object sender, RoutedEventArgs e)
    {
        if (_page == WizardPage.Progress)
        {
            _installCts?.Cancel();
            return;
        }

        Close();
    }

    private bool ValidateOptions(out string error)
    {
        error = string.Empty;
        var path = InstallPathBox.Text?.Trim() ?? string.Empty;
        if (string.IsNullOrWhiteSpace(path))
        {
            error = "请填写安装路径。";
            return false;
        }

        try
        {
            path = System.IO.Path.GetFullPath(path);
        }
        catch (Exception exception)
        {
            error = $"安装路径无效：{exception.Message}";
            return false;
        }

        if (path.Length < 4)
        {
            error = "安装路径过短。";
            return false;
        }

        InstallPathBox.Text = path;
        return true;
    }

    private async Task RunInstallAsync()
    {
        _installCts?.Cancel();
        _installCts?.Dispose();
        _installCts = new CancellationTokenSource();

        NextButton.IsEnabled = false;
        BackButton.IsEnabled = false;
        CancelButton.Content = "取消安装";
        CancelButton.IsEnabled = true;
        _installSucceeded = false;

        var options = new InstallOptions
        {
            InstallDirectory = InstallPathBox.Text.Trim(),
            CreateDesktopShortcut = DesktopShortcutCheck.IsChecked == true,
            CreateStartMenuShortcut = StartMenuShortcutCheck.IsChecked == true,
            LaunchAfterInstall = LaunchAfterCheck.IsChecked == true,
        };

        var progress = new Progress<InstallProgress>(UpdateProgress);

        try
        {
            await InstallService.InstallAsync(options, progress, _installCts.Token);
            _installSucceeded = true;
            DoneTitle.Text = "安装完成";
            DoneDetail.Text =
                $"{InstallService.ProductName} 已安装到：\n{options.InstallDirectory}\n\n" +
                (options.CreateDesktopShortcut ? "已创建桌面快捷方式。\n" : string.Empty) +
                (options.CreateStartMenuShortcut ? "已创建开始菜单快捷方式。\n" : string.Empty) +
                (options.LaunchAfterInstall ? "应用已尝试启动。" : "可从快捷方式启动应用。");
            ShowPage(WizardPage.Done);
        }
        catch (OperationCanceledException)
        {
            DoneTitle.Text = "安装已取消";
            DoneDetail.Text = "安装过程已中断。可重新打开安装器再试。";
            ShowPage(WizardPage.Done);
        }
        catch (Exception exception)
        {
            DoneTitle.Text = "安装失败";
            DoneDetail.Text = exception.Message;
            ShowPage(WizardPage.Done);
        }
        finally
        {
            NextButton.IsEnabled = true;
            CancelButton.Content = "关闭";
            CancelButton.IsEnabled = true;
            BackButton.IsEnabled = false;
            NextButton.Content = _installSucceeded ? "完成" : "关闭";
        }
    }

    private void UpdateProgress(InstallProgress progress)
    {
        ProgressTitle.Text = "正在安装…";
        ProgressPhaseText.Text = progress.Phase;
        ProgressDetail.Text = progress.Detail;
        ProgressPercentText.Text = $"{progress.Percent:0}%";

        if (ProgressFill.RenderTransform is ScaleTransform scale)
        {
            var animation = new DoubleAnimation
            {
                To = Math.Clamp(progress.Percent / 100.0, 0, 1),
                Duration = TimeSpan.FromMilliseconds(280),
                EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
            };
            scale.BeginAnimation(ScaleTransform.ScaleXProperty, animation);
        }
    }

    private void ShowPage(WizardPage page)
    {
        _page = page;
        PageWelcome.Visibility = page == WizardPage.Welcome ? Visibility.Visible : Visibility.Collapsed;
        PageOptions.Visibility = page == WizardPage.Options ? Visibility.Visible : Visibility.Collapsed;
        PageProgress.Visibility = page == WizardPage.Progress ? Visibility.Visible : Visibility.Collapsed;
        PageDone.Visibility = page == WizardPage.Done ? Visibility.Visible : Visibility.Collapsed;

        HighlightStep(StepWelcome, page == WizardPage.Welcome);
        HighlightStep(StepOptions, page == WizardPage.Options);
        HighlightStep(StepProgress, page == WizardPage.Progress);
        HighlightStep(StepDone, page == WizardPage.Done);

        BackButton.Visibility = page == WizardPage.Options ? Visibility.Visible : Visibility.Collapsed;
        BackButton.IsEnabled = page == WizardPage.Options;

        CancelButton.Visibility = page == WizardPage.Done ? Visibility.Collapsed : Visibility.Visible;
        CancelButton.Content = page == WizardPage.Progress ? "取消安装" : "取消";

        NextButton.Content = page switch
        {
            WizardPage.Welcome => "下一步",
            WizardPage.Options => "开始安装",
            WizardPage.Progress => "请稍候…",
            WizardPage.Done => "完成",
            _ => "下一步",
        };
        NextButton.IsEnabled = page != WizardPage.Progress;

        if (TryFindResource("PageEnterStoryboard") is Storyboard enter)
        {
            enter.Begin(this, true);
        }
    }

    private void HighlightStep(Border step, bool active)
    {
        step.Background = active
            ? (WpfBrush)FindResource("AccentSoftBrush")
            : WpfBrushes.Transparent;
        step.BorderBrush = active
            ? (WpfBrush)FindResource("AccentBrush")
            : WpfBrushes.Transparent;
        step.BorderThickness = active ? new Thickness(1) : new Thickness(0);
        if (step.Child is TextBlock text)
        {
            text.Foreground = active
                ? (WpfBrush)FindResource("TextBrush")
                : (WpfBrush)FindResource("MutedBrush");
            text.FontWeight = active ? FontWeights.SemiBold : FontWeights.Normal;
        }
    }
}
