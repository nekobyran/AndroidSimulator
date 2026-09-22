using System.Text.Json;
using System.Text.Json.Serialization;
using AndroidSimulator.App.Models;

namespace AndroidSimulator.App.Services;

public sealed class KeyMappingStore
{
    private const int CurrentSchemaVersion = 1;

    private static readonly JsonSerializerOptions SerializerOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        PropertyNameCaseInsensitive = false,
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.CamelCase) },
    };

    public KeyMappingStore(string? filePath = null)
    {
        FilePath = string.IsNullOrWhiteSpace(filePath)
            ? GetDefaultFilePath()
            : Path.GetFullPath(filePath);
    }

    public string FilePath { get; }

    public async Task<IReadOnlyList<KeyBinding>> LoadAsync(
        CancellationToken cancellationToken = default)
    {
        if (!File.Exists(FilePath))
        {
            return Array.Empty<KeyBinding>();
        }

        try
        {
            await using var stream = new FileStream(
                FilePath,
                FileMode.Open,
                FileAccess.Read,
                FileShare.Read,
                bufferSize: 4096,
                FileOptions.Asynchronous | FileOptions.SequentialScan);
            var document = await JsonSerializer.DeserializeAsync<KeyMappingDocument>(
                stream,
                SerializerOptions,
                cancellationToken);

            if (document is null
                || document.SchemaVersion != CurrentSchemaVersion
                || document.Mappings is null)
            {
                return Array.Empty<KeyBinding>();
            }

            return document.Mappings
                .Where(IsValid)
                .ToArray();
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception exception) when (
            exception is IOException
                or UnauthorizedAccessException
                or JsonException
                or NotSupportedException)
        {
            // A corrupt or temporarily unavailable settings file must not prevent startup.
            return Array.Empty<KeyBinding>();
        }
    }

    public async Task SaveAsync(
        IEnumerable<KeyBinding> mappings,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(mappings);

        var snapshot = mappings.ToArray();
        var invalid = snapshot.FirstOrDefault(mapping => !IsValid(mapping));
        if (invalid is not null)
        {
            throw new InvalidDataException(
                $"Key mapping '{invalid.Id}' is incomplete or contains an invalid key/action.");
        }

        var directory = Path.GetDirectoryName(FilePath)
            ?? throw new InvalidOperationException("The key mapping path has no parent directory.");
        Directory.CreateDirectory(directory);

        var temporaryPath = Path.Combine(
            directory,
            $".{Path.GetFileName(FilePath)}.{Guid.NewGuid():N}.tmp");
        try
        {
            await using (var stream = new FileStream(
                temporaryPath,
                FileMode.CreateNew,
                FileAccess.Write,
                FileShare.None,
                bufferSize: 4096,
                FileOptions.Asynchronous | FileOptions.WriteThrough))
            {
                await JsonSerializer.SerializeAsync(
                    stream,
                    new KeyMappingDocument(CurrentSchemaVersion, snapshot),
                    SerializerOptions,
                    cancellationToken);
                await stream.FlushAsync(cancellationToken);
                stream.Flush(flushToDisk: true);
            }

            cancellationToken.ThrowIfCancellationRequested();
            File.Move(temporaryPath, FilePath, overwrite: true);
        }
        finally
        {
            try
            {
                File.Delete(temporaryPath);
            }
            catch (IOException)
            {
            }
            catch (UnauthorizedAccessException)
            {
            }
        }
    }

    public static string GetDefaultFilePath()
    {
        var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        if (string.IsNullOrWhiteSpace(localAppData))
        {
            throw new InvalidOperationException("LOCALAPPDATA is unavailable.");
        }

        return Path.Combine(
            localAppData,
            "AndroidSimulator",
            "data",
            "settings",
            "keymap.json");
    }

    private static bool IsValid(KeyBinding? mapping)
    {
        if (mapping is null
            || string.IsNullOrWhiteSpace(mapping.Id)
            || mapping.HostKey is < 1 or > 255
            || !Enum.IsDefined(mapping.ActionType))
        {
            return false;
        }

        return mapping.ActionType switch
        {
            KeyBindingActionType.Tap => mapping.TapX >= 0 && mapping.TapY >= 0,
            KeyBindingActionType.KeyEvent => !string.IsNullOrWhiteSpace(mapping.AndroidKeyCode),
            _ => false,
        };
    }

    private sealed record KeyMappingDocument(
        int SchemaVersion,
        IReadOnlyList<KeyBinding>? Mappings);
}
