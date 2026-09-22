using Windows.Graphics;

namespace AndroidSimulator.App.Services;

internal static class WindowPlacement
{
    public static SizeInt32 ScaleHostSize(
        int logicalWidth,
        int titleBarHeight,
        double scale)
    {
        var safeWidth = Math.Max(1, logicalWidth);
        var safeTitleBar = Math.Max(0, titleBarHeight);
        var safeScale = double.IsFinite(scale) && scale > 0 ? scale : 1.0;
        var videoHeight = safeWidth * 9.0 / 16.0;
        return new SizeInt32(
            Math.Max(1, (int)Math.Round(safeWidth * safeScale)),
            Math.Max(1, (int)Math.Round((videoHeight + safeTitleBar) * safeScale)));
    }

    public static double ResolveMonitorScale(
        int reportedScalePercent,
        RectInt32 nativeWorkArea,
        RectInt32 appWorkArea)
    {
        var reported = reportedScalePercent > 0
            ? Math.Clamp(reportedScalePercent / 100.0, 1.0, 4.0)
            : 1.0;
        if (nativeWorkArea.Width <= 0 || appWorkArea.Width <= 0)
        {
            return reported;
        }

        var inferred = nativeWorkArea.Width / (double)appWorkArea.Width;
        return inferred is >= 1.0 and <= 4.0
            ? Math.Max(reported, inferred)
            : reported;
    }

    public static RectInt32 CenterInWorkArea(RectInt32 workArea, SizeInt32 requestedSize)
    {
        var width = Math.Min(Math.Max(1, requestedSize.Width), Math.Max(1, workArea.Width));
        var height = Math.Min(Math.Max(1, requestedSize.Height), Math.Max(1, workArea.Height));
        return new RectInt32(
            workArea.X + ((workArea.Width - width) / 2),
            workArea.Y + ((workArea.Height - height) / 2),
            width,
            height);
    }

    public static RectInt32 CreatePhysicalHostViewport(
        int clientWidth,
        int clientHeight,
        int logicalTitleBarHeight,
        double rasterizationScale)
    {
        var width = Math.Max(1, clientWidth);
        var height = Math.Max(1, clientHeight);
        var safeScale = double.IsFinite(rasterizationScale) && rasterizationScale > 0
            ? rasterizationScale
            : 1.0;
        var titleBarHeight = Math.Clamp(
            (int)Math.Round(Math.Max(0, logicalTitleBarHeight) * safeScale),
            0,
            height - 1);
        return new RectInt32(0, titleBarHeight, width, height - titleBarHeight);
    }
}
