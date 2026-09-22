namespace AndroidSimulator.App.Services;

internal static class SimulatorCtlPathResolver
{
    private const string ExecutableName = "simulatorctl.exe";

    public static string Resolve(string baseDirectory, string? overridePath)
    {
        if (!string.IsNullOrWhiteSpace(overridePath) && File.Exists(overridePath))
        {
            return Path.GetFullPath(overridePath);
        }

        var packaged = Path.Combine(Path.GetFullPath(baseDirectory), ExecutableName);
        if (File.Exists(packaged))
        {
            return packaged;
        }

        var current = new DirectoryInfo(baseDirectory);
        while (current is not null)
        {
            foreach (var candidate in WorkspaceCandidates(current.FullName))
            {
                if (File.Exists(candidate))
                {
                    return candidate;
                }
            }

            current = current.Parent;
        }

        return packaged;
    }

    private static IEnumerable<string> WorkspaceCandidates(string directory)
    {
        yield return Path.Combine(directory, "rust", "target", "debug", ExecutableName);
        yield return Path.Combine(
            directory,
            "rust",
            "simulatorctl",
            "target",
            "debug",
            ExecutableName);
        yield return Path.Combine(
            directory,
            "project",
            "android-simulator",
            "rust",
            "target",
            "debug",
            ExecutableName);
    }
}
