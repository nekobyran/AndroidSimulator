namespace AndroidSimulator.App.Services;

public sealed record DirectLaunchRequest(
    string? PackageName,
    string DisplayName,
    string? ApkPath,
    bool Background);

public sealed record DirectLaunchParseResult(
    DirectLaunchRequest? Request,
    string? Error);

public static class ApkActivationService
{
    public static string[] BuildPackageHostArguments(string packageName, string applicationName)
    {
        if (string.IsNullOrWhiteSpace(packageName))
        {
            throw new ArgumentException("Package name is required.", nameof(packageName));
        }
        if (string.IsNullOrWhiteSpace(applicationName))
        {
            throw new ArgumentException("Application name is required.", nameof(applicationName));
        }

        return
        [
            "--package",
            packageName.Trim(),
            "--app-name",
            applicationName.Trim(),
            "--background",
        ];
    }

    public static DirectLaunchParseResult ParseDirectLaunch(IReadOnlyList<string> arguments)
    {
        string? packageName = null;
        string? applicationName = null;
        string? apkPath = null;
        var background = false;
        var hasDirectArgument = false;

        for (var index = 0; index < arguments.Count; index++)
        {
            var argument = arguments[index];
            switch (argument.ToLowerInvariant())
            {
                case "--package":
                    hasDirectArgument = true;
                    if (!TryReadValue(arguments, ref index, out packageName))
                    {
                        return new(null, "启动参数 --package 缺少应用包名。");
                    }
                    break;
                case "--app-name":
                    hasDirectArgument = true;
                    if (!TryReadValue(arguments, ref index, out applicationName))
                    {
                        return new(null, "启动参数 --app-name 缺少应用名称。");
                    }
                    break;
                case "--apk":
                    hasDirectArgument = true;
                    if (!TryReadValue(arguments, ref index, out apkPath))
                    {
                        return new(null, "启动参数 --apk 缺少文件路径。");
                    }
                    break;
                case "--background":
                    hasDirectArgument = true;
                    background = true;
                    break;
                default:
                    if (argument.StartsWith("--", StringComparison.Ordinal))
                    {
                        return new(null, $"不支持的启动参数：{argument}");
                    }
                    break;
            }
        }

        if (!hasDirectArgument)
        {
            return new(null, null);
        }

        if (!string.IsNullOrWhiteSpace(packageName) && !string.IsNullOrWhiteSpace(apkPath))
        {
            return new(null, "--package 与 --apk 不能同时使用。");
        }

        if (string.IsNullOrWhiteSpace(packageName) && string.IsNullOrWhiteSpace(apkPath))
        {
            return new(null, "快捷启动参数缺少 --package 或 --apk。");
        }

        string? fullApkPath = null;
        if (!string.IsNullOrWhiteSpace(apkPath))
        {
            try
            {
                fullApkPath = Path.GetFullPath(apkPath);
            }
            catch (Exception exception) when (exception is ArgumentException or NotSupportedException or PathTooLongException)
            {
                return new(null, $"APK 路径无效：{exception.Message}");
            }
        }

        var displayName = !string.IsNullOrWhiteSpace(applicationName)
            ? applicationName.Trim()
            : packageName?.Trim() ?? Path.GetFileNameWithoutExtension(apkPath);
        if (string.IsNullOrWhiteSpace(displayName))
        {
            displayName = "Android 应用";
        }

        return new(
            new DirectLaunchRequest(
                packageName?.Trim(),
                displayName,
                fullApkPath,
                background),
            null);
    }

    private static bool TryReadValue(
        IReadOnlyList<string> arguments,
        ref int index,
        out string? value)
    {
        if (index + 1 >= arguments.Count || arguments[index + 1].StartsWith("--", StringComparison.Ordinal))
        {
            value = null;
            return false;
        }

        value = arguments[++index];
        return !string.IsNullOrWhiteSpace(value);
    }
}
