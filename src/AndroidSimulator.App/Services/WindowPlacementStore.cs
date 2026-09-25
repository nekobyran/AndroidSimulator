using System.Text.Json;
using Windows.Graphics;

namespace AndroidSimulator.App.Services;

internal sealed record SavedWindowPlacement(int X, int Y, int Width, int Height);

internal static class WindowPlacementStore
{
    private static readonly string StorePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "AndroidSimulator",
        "window-placements.json");

    public static bool TryLoad(string packageName, out RectInt32 bounds)
    {
        bounds = default;
        if (string.IsNullOrWhiteSpace(packageName) || !File.Exists(StorePath))
        {
            return false;
        }

        try
        {
            var placements = JsonSerializer.Deserialize<Dictionary<string, SavedWindowPlacement>>(
                File.ReadAllText(StorePath));
            if (placements is null
                || !placements.TryGetValue(packageName, out var saved)
                || saved.Width <= 0
                || saved.Height <= 0)
            {
                return false;
            }

            bounds = new RectInt32(saved.X, saved.Y, saved.Width, saved.Height);
            return true;
        }
        catch (Exception exception) when (
            exception is IOException or UnauthorizedAccessException or JsonException)
        {
            return false;
        }
    }

    public static bool TrySave(string packageName, RectInt32 bounds)
    {
        if (string.IsNullOrWhiteSpace(packageName) || bounds.Width <= 0 || bounds.Height <= 0)
        {
            return false;
        }

        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
            var placements = File.Exists(StorePath)
                ? JsonSerializer.Deserialize<Dictionary<string, SavedWindowPlacement>>(
                    File.ReadAllText(StorePath)) ?? new Dictionary<string, SavedWindowPlacement>()
                : new Dictionary<string, SavedWindowPlacement>();
            placements[packageName] = new SavedWindowPlacement(
                bounds.X, bounds.Y, bounds.Width, bounds.Height);
            File.WriteAllText(
                StorePath,
                JsonSerializer.Serialize(placements, new JsonSerializerOptions { WriteIndented = true }));
            return true;
        }
        catch (Exception exception) when (
            exception is IOException or UnauthorizedAccessException or JsonException)
        {
            return false;
        }
    }
}
