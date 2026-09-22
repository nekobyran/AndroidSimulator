using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media.Imaging;

namespace AndroidSimulator.App.Models;

public sealed class AndroidAppInfo
{
    public string PackageName { get; init; } = string.Empty;

    public string Name { get; init; } = string.Empty;

    public string ActivityName { get; init; } = string.Empty;

    public string Version { get; init; } = string.Empty;

    public string? IconPath { get; init; }

    public bool HasIcon { get; init; }

    public BitmapImage? IconImage { get; init; }

    public Visibility IconVisibility => HasIcon ? Visibility.Visible : Visibility.Collapsed;

    public Visibility FallbackIconVisibility => HasIcon ? Visibility.Collapsed : Visibility.Visible;
}