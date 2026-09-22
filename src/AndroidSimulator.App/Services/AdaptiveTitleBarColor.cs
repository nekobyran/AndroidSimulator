namespace AndroidSimulator.App.Services;

internal static class AdaptiveTitleBarColor
{
    private const int MinimumSampleCount = 8;
    private const int MinimumDistanceSquared = 12 * 12;

    internal static bool TryCreatePalette(
        ReadOnlySpan<int> rgbPixels,
        out AdaptiveTitleBarPalette palette)
    {
        palette = default;
        if (rgbPixels.Length < MinimumSampleCount)
        {
            return false;
        }

        var red = new int[rgbPixels.Length];
        var green = new int[rgbPixels.Length];
        var blue = new int[rgbPixels.Length];
        for (var index = 0; index < rgbPixels.Length; index++)
        {
            var rgb = rgbPixels[index] & 0x00FF_FFFF;
            red[index] = (rgb >> 16) & 0xFF;
            green[index] = (rgb >> 8) & 0xFF;
            blue[index] = rgb & 0xFF;
        }

        var medianRed = Median(red);
        var medianGreen = Median(green);
        var medianBlue = Median(blue);
        var distances = new int[rgbPixels.Length];
        for (var index = 0; index < rgbPixels.Length; index++)
        {
            var deltaRed = red[index] - medianRed;
            var deltaGreen = green[index] - medianGreen;
            var deltaBlue = blue[index] - medianBlue;
            distances[index] = deltaRed * deltaRed
                + deltaGreen * deltaGreen
                + deltaBlue * deltaBlue;
        }

        var sortedDistances = (int[])distances.Clone();
        Array.Sort(sortedDistances);
        var distanceThreshold = Math.Max(
            MinimumDistanceSquared,
            sortedDistances[(sortedDistances.Length * 2) / 3]);

        long redTotal = 0;
        long greenTotal = 0;
        long blueTotal = 0;
        var included = 0;
        for (var index = 0; index < distances.Length; index++)
        {
            if (distances[index] > distanceThreshold)
            {
                continue;
            }

            redTotal += red[index];
            greenTotal += green[index];
            blueTotal += blue[index];
            included++;
        }

        if (included < MinimumSampleCount)
        {
            return false;
        }

        var representativeRed = (byte)((redTotal + included / 2) / included);
        var representativeGreen = (byte)((greenTotal + included / 2) / included);
        var representativeBlue = (byte)((blueTotal + included / 2) / included);
        palette = new AdaptiveTitleBarPalette(
            representativeRed,
            representativeGreen,
            representativeBlue,
            ShouldUseLightForeground(
                representativeRed,
                representativeGreen,
                representativeBlue));
        return true;
    }

    private static int Median(int[] values)
    {
        Array.Sort(values);
        var middle = values.Length / 2;
        return values.Length % 2 == 0
            ? (values[middle - 1] + values[middle]) / 2
            : values[middle];
    }

    private static bool ShouldUseLightForeground(byte red, byte green, byte blue)
    {
        var luminance = 0.2126 * ToLinear(red)
            + 0.7152 * ToLinear(green)
            + 0.0722 * ToLinear(blue);
        var whiteContrast = 1.05 / (luminance + 0.05);
        var blackContrast = (luminance + 0.05) / 0.05;
        return whiteContrast >= blackContrast;
    }

    private static double ToLinear(byte channel)
    {
        var normalized = channel / 255.0;
        return normalized <= 0.04045
            ? normalized / 12.92
            : Math.Pow((normalized + 0.055) / 1.055, 2.4);
    }
}

internal readonly record struct AdaptiveTitleBarPalette(
    byte Red,
    byte Green,
    byte Blue,
    bool UseLightForeground);
