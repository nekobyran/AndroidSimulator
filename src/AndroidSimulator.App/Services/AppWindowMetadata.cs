using System.Text.Json;

namespace AndroidSimulator.App.Services;

internal sealed record AppWindowMetadata(
    uint WindowProcessId,
    uint DisplayId,
    string PackageName,
    string Title,
    string ScrcpyPath,
    string? IconPath = null)
{
    private const int CurrentSchemaVersion = 1;
    private const string AndroidHomePackage = "com.android.launcher3";

    public static bool TryParse(JsonElement? payload, out AppWindowMetadata metadata)
    {
        metadata = null!;
        if (payload is not JsonElement element || element.ValueKind != JsonValueKind.Object)
        {
            return false;
        }

        // APK install --launch has one explicit launch object. No historical payload
        // shapes are accepted: a malformed or pending display must fail closed.
        if (element.TryGetProperty("launch", out var launch))
        {
            element = launch;
        }

        if (!TryGetUInt32(element, "schema_version", out var schemaVersion)
            || schemaVersion != CurrentSchemaVersion
            || !TryGetUInt32(element, "window_pid", out var processId)
            || processId == 0
            || !TryGetUInt32(element, "display_id", out var displayId)
            || !TryGetString(element, "display_status", out var displayStatus)
            || !displayStatus.Equals("ready", StringComparison.Ordinal)
            || !TryGetString(element, "package", out var packageName)
            || !TryGetString(element, "title", out var title)
            || !TryGetString(element, "fallback_mode", out var fallbackMode)
            || !fallbackMode.Equals("none", StringComparison.Ordinal)
            || !TryGetString(element, "scrcpy_path", out var scrcpyPath)
            || !TrustedAppWindowProcess.IsTrustedExecutablePath(scrcpyPath))
        {
            return false;
        }

        if (displayId == 0
            && !packageName.Equals(AndroidHomePackage, StringComparison.Ordinal))
        {
            return false;
        }

        var iconPath = TryGetString(element, "icon_path", out var parsedIconPath)
            ? parsedIconPath
            : null;

        metadata = new AppWindowMetadata(
            processId,
            displayId,
            packageName,
            title,
            scrcpyPath,
            iconPath);
        return true;
    }

    private static bool TryGetUInt32(
        JsonElement element,
        string propertyName,
        out uint value)
    {
        value = 0;
        return element.TryGetProperty(propertyName, out var property)
            && property.ValueKind == JsonValueKind.Number
            && property.TryGetUInt32(out value);
    }

    private static bool TryGetString(
        JsonElement element,
        string propertyName,
        out string value)
    {
        value = string.Empty;
        if (!element.TryGetProperty(propertyName, out var property)
            || property.ValueKind != JsonValueKind.String)
        {
            return false;
        }

        value = property.GetString()?.Trim() ?? string.Empty;
        return value.Length > 0;
    }
}

internal static class AppWindowMetadataStore
{
    private const long MaximumMetadataBytes = 64 * 1024;

    public static string MetadataDirectory { get; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "AndroidSimulator",
        "runtime",
        "app-windows");

    public static bool TryRead(uint processId, out AppWindowMetadata metadata)
    {
        metadata = null!;
        if (processId == 0)
        {
            return false;
        }

        try
        {
            var metadataPath = Path.Combine(MetadataDirectory, $"{processId}.json");
            using var stream = new FileStream(
                metadataPath,
                FileMode.Open,
                FileAccess.Read,
                FileShare.ReadWrite | FileShare.Delete);
            if (stream.Length is <= 0 or > MaximumMetadataBytes)
            {
                return false;
            }

            using var document = JsonDocument.Parse(stream);
            return AppWindowMetadata.TryParse(document.RootElement, out metadata)
                && metadata.WindowProcessId == processId;
        }
        catch (Exception exception) when (
            exception is IOException
                or UnauthorizedAccessException
                or JsonException
                or ArgumentException
                or NotSupportedException)
        {
            return false;
        }
    }
}
