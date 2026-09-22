using System.Collections.Generic;
using System.IO;
using System.Linq;

namespace AndroidSimulator.App.Services;

internal static class HostAppIcon
{
    public static string? ResolveDisplayIconPath(string? iconPath)
    {
        if (string.IsNullOrWhiteSpace(iconPath))
        {
            return null;
        }

        var fullPath = Path.GetFullPath(iconPath);
        if (!File.Exists(fullPath))
        {
            return null;
        }

        var directory = Path.GetDirectoryName(fullPath);
        var stem = Path.GetFileNameWithoutExtension(fullPath);
        if (string.IsNullOrWhiteSpace(directory) || string.IsNullOrWhiteSpace(stem))
        {
            return fullPath;
        }

        if (stem.Contains("-desktop-rounded-", StringComparison.OrdinalIgnoreCase))
        {
            return fullPath;
        }

        try
        {
            var rounded = Directory.EnumerateFiles(directory, stem + "-desktop-rounded-*.ico")
                .OrderBy(path => new FileInfo(path).Length)
                .FirstOrDefault();
            if (!string.IsNullOrWhiteSpace(rounded) && File.Exists(rounded))
            {
                return rounded;
            }
        }
        catch (Exception exception) when (
            exception is IOException
                or UnauthorizedAccessException
                or ArgumentException)
        {
        }

        return fullPath;
    }
}
