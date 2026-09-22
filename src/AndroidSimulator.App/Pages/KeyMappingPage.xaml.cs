using System.Collections.ObjectModel;
using AndroidSimulator.App.Models;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;
using Windows.UI.ViewManagement;

namespace AndroidSimulator.App.Pages;

public sealed partial class KeyMappingPage : Page
{
    private readonly ObservableCollection<KeyBindingItem> _items = [];
    private readonly SemaphoreSlim _saveGate = new(1, 1);
    private CancellationTokenSource? _operationCancellation;
    private int? _capturedHostKey;
    private bool _suppressToggle;

    public KeyMappingPage()
    {
        InitializeComponent();
        if (!new UISettings().AnimationsEnabled && MappingsList.ItemContainerTransitions is not null)
        {
            MappingsList.ItemContainerTransitions.Clear();
        }

        MappingsList.ItemsSource = _items;
    }

    private void BackButton_Click(object sender, RoutedEventArgs e)
    {
        App.MainWindow?.NavigateTo("settings");
    }

    private async void Page_Loaded(object sender, RoutedEventArgs e)
    {
        await LoadMappingsAsync();
    }

    private void Page_Unloaded(object sender, RoutedEventArgs e)
    {
        _operationCancellation?.Cancel();
        _operationCancellation?.Dispose();
        _operationCancellation = null;
    }

    private async void RetryButton_Click(object sender, RoutedEventArgs e)
    {
        await LoadMappingsAsync();
    }

    private async void AddMappingButton_Click(object sender, RoutedEventArgs e)
    {
        AddMappingDialog.XamlRoot = XamlRoot;
        await AddMappingDialog.ShowAsync();
    }

    private void AddMappingDialog_Opened(ContentDialog sender, ContentDialogOpenedEventArgs args)
    {
        _capturedHostKey = null;
        MappingNameBox.Text = string.Empty;
        HostKeyBox.Text = string.Empty;
        ActionTypeBox.SelectedIndex = 0;
        TapXBox.Value = double.NaN;
        TapYBox.Value = double.NaN;
        AndroidKeyCodeBox.Text = string.Empty;
        DialogValidationText.Visibility = Visibility.Collapsed;
    }

    private void HostKeyBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key is VirtualKey.Control or VirtualKey.Shift or VirtualKey.Menu or VirtualKey.LeftWindows or VirtualKey.RightWindows)
        {
            return;
        }

        _capturedHostKey = (int)e.Key;
        HostKeyBox.Text = KeyBindingItem.FormatKey(_capturedHostKey.Value);
        e.Handled = true;
    }

    private void ActionTypeBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (TapFields is null || AndroidKeyCodeBox is null)
        {
            return;
        }

        var isTap = ActionTypeBox.SelectedIndex == 0;
        TapFields.Visibility = isTap ? Visibility.Visible : Visibility.Collapsed;
        AndroidKeyCodeBox.Visibility = isTap ? Visibility.Collapsed : Visibility.Visible;
    }

    private async void AddMappingDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        var validationError = ValidateDialog();
        if (validationError is not null)
        {
            args.Cancel = true;
            DialogValidationText.Text = validationError;
            DialogValidationText.Visibility = Visibility.Visible;
            return;
        }

        var deferral = args.GetDeferral();
        try
        {
            var isTap = ActionTypeBox.SelectedIndex == 0;
            var mapping = new KeyBinding
            {
                Id = Guid.NewGuid().ToString("N"),
                Name = MappingNameBox.Text.Trim(),
                HostKey = _capturedHostKey!.Value,
                ActionType = isTap ? KeyBindingActionType.Tap : KeyBindingActionType.KeyEvent,
                TapX = isTap ? (int)TapXBox.Value : 0,
                TapY = isTap ? (int)TapYBox.Value : 0,
                AndroidKeyCode = isTap ? string.Empty : AndroidKeyCodeBox.Text.Trim(),
                IsEnabled = true,
            };

            var candidate = CurrentMappings().Append(mapping).ToArray();
            await SaveMappingsAsync(candidate);
            _items.Add(new KeyBindingItem(mapping));
            UpdateListState();
            ShowStatus(InfoBarSeverity.Success, "键位已添加", $"{KeyBindingItem.FormatKey(mapping.HostKey)} 已绑定。");
        }
        catch (Exception exception)
        {
            args.Cancel = true;
            DialogValidationText.Text = exception.Message;
            DialogValidationText.Visibility = Visibility.Visible;
        }
        finally
        {
            deferral.Complete();
        }
    }

    private async void MappingToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (_suppressToggle || (sender as FrameworkElement)?.DataContext is not KeyBindingItem item)
        {
            return;
        }

        var toggle = (ToggleSwitch)sender;
        if (toggle.IsOn == item.Mapping.IsEnabled)
        {
            return;
        }

        var previousValue = item.Mapping.IsEnabled;
        item.IsEnabled = toggle.IsOn;
        try
        {
            await SaveMappingsAsync(CurrentMappings());
        }
        catch (Exception exception)
        {
            _suppressToggle = true;
            item.IsEnabled = previousValue;
            toggle.IsOn = previousValue;
            _suppressToggle = false;
            ShowStatus(InfoBarSeverity.Error, "无法保存键位", exception.Message);
        }
    }

    private async void DeleteMappingButton_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not KeyBindingItem item)
        {
            return;
        }

        SetOperationBusy(true);
        try
        {
            var candidate = CurrentMappings().Where(mapping => !ReferenceEquals(mapping, item.Mapping)).ToArray();
            await SaveMappingsAsync(candidate);
            _items.Remove(item);
            UpdateListState();
            ShowStatus(InfoBarSeverity.Success, "键位已删除", item.DisplayName);
        }
        catch (Exception exception)
        {
            ShowStatus(InfoBarSeverity.Error, "无法删除键位", exception.Message);
        }
        finally
        {
            SetOperationBusy(false);
        }
    }

    private async Task LoadMappingsAsync()
    {
        _operationCancellation?.Cancel();
        _operationCancellation?.Dispose();
        _operationCancellation = new CancellationTokenSource();
        var cancellationToken = _operationCancellation.Token;
        SetListState(KeyMappingListState.Loading);
        try
        {
            var mappings = await App.KeyMappingStore.LoadAsync(cancellationToken);
            _items.Clear();
            foreach (var mapping in mappings)
            {
                _items.Add(new KeyBindingItem(mapping));
            }

            App.ReplaceKeyboardMappings(CurrentMappings());
            UpdateListState();
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception)
        {
            SetListState(KeyMappingListState.Error, exception.Message);
            ShowStatus(InfoBarSeverity.Error, "无法读取键位", exception.Message);
        }
    }

    private string? ValidateDialog()
    {
        if (_capturedHostKey is null)
        {
            return "请先聚焦宿主键输入框并按下一个键。";
        }

        if (_capturedHostKey is < 1 or > 255)
        {
            return "这个按键不能用作宿主键，请改用标准键盘按键。";
        }

        if (_items.Any(item => item.Mapping.HostKey == _capturedHostKey.Value))
        {
            return "这个宿主键已经有映射，请先删除原映射。";
        }

        if (ActionTypeBox.SelectedIndex == 0)
        {
            if (double.IsNaN(TapXBox.Value) || double.IsNaN(TapYBox.Value))
            {
                return "请填写完整的 X、Y 点击坐标。";
            }
        }
        else if (string.IsNullOrWhiteSpace(AndroidKeyCodeBox.Text))
        {
            return "请填写 Android keyevent，例如 KEYCODE_BACK 或 4。";
        }

        return null;
    }

    private async Task SaveMappingsAsync(IEnumerable<KeyBinding> mappings)
    {
        var snapshot = mappings.ToArray();
        await _saveGate.WaitAsync();
        try
        {
            await App.KeyMappingStore.SaveAsync(snapshot);
            App.ReplaceKeyboardMappings(snapshot);
        }
        finally
        {
            _saveGate.Release();
        }
    }

    private IEnumerable<KeyBinding> CurrentMappings()
    {
        return _items.Select(item => item.Mapping);
    }

    private void UpdateListState()
    {
        SetListState(_items.Count == 0 ? KeyMappingListState.Empty : KeyMappingListState.Content);
    }

    private void SetOperationBusy(bool isBusy)
    {
        OperationProgressRing.IsActive = isBusy;
        OperationProgressRing.Visibility = isBusy ? Visibility.Visible : Visibility.Collapsed;
        AddMappingButton.IsEnabled = !isBusy;
        MappingsList.IsEnabled = !isBusy;
    }

    private void SetListState(KeyMappingListState state, string? error = null)
    {
        MappingsList.Visibility = state == KeyMappingListState.Content ? Visibility.Visible : Visibility.Collapsed;
        LoadingPanel.Visibility = state == KeyMappingListState.Loading ? Visibility.Visible : Visibility.Collapsed;
        EmptyPanel.Visibility = state == KeyMappingListState.Empty ? Visibility.Visible : Visibility.Collapsed;
        ErrorPanel.Visibility = state == KeyMappingListState.Error ? Visibility.Visible : Visibility.Collapsed;
        ErrorMessageText.Text = error ?? string.Empty;
    }

    private void ShowStatus(InfoBarSeverity severity, string title, string message)
    {
        StatusInfoBar.Severity = severity;
        StatusInfoBar.Title = title;
        StatusInfoBar.Message = message;
        StatusInfoBar.IsOpen = true;
    }

    private enum KeyMappingListState
    {
        Loading,
        Content,
        Empty,
        Error,
    }

    private sealed class KeyBindingItem
    {
        public KeyBindingItem(KeyBinding mapping)
        {
            Mapping = mapping;
        }

        public KeyBinding Mapping { get; private set; }

        public bool IsEnabled
        {
            get => Mapping.IsEnabled;
            set => Mapping = Mapping with { IsEnabled = value };
        }

        public string HostKeyText => FormatKey(Mapping.HostKey);

        public string DisplayName => string.IsNullOrWhiteSpace(Mapping.Name)
            ? HostKeyText
            : Mapping.Name;

        public string ActionSummary => Mapping.ActionType == KeyBindingActionType.Tap
            ? $"点击 ({Mapping.TapX}, {Mapping.TapY})"
            : $"Android 按键 {Mapping.AndroidKeyCode}";

        public static string FormatKey(int keyCode)
        {
            var key = (VirtualKey)keyCode;
            return key switch
            {
                >= VirtualKey.Number0 and <= VirtualKey.Number9 => ((char)('0' + (int)key - (int)VirtualKey.Number0)).ToString(),
                >= VirtualKey.A and <= VirtualKey.Z => key.ToString(),
                _ => key.ToString(),
            };
        }
    }
}
