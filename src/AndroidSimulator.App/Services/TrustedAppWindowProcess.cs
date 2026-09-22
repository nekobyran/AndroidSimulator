using System.Runtime.InteropServices;
using System.Text;

namespace AndroidSimulator.App.Services;

internal static class TrustedAppWindowProcess
{
    private const uint ProcessQueryLimitedInformation = 0x1000;

    public const string ExecutablePath =
        @"D:\vibecoding\sdk\scrcpy\scrcpy.exe";

    public static bool IsTrustedProcessId(uint processId)
        => IsExpectedProcessId(processId, ExecutablePath);

    internal static bool IsExpectedProcessId(uint processId, string expectedExecutablePath)
    {
        return processId != 0
            && TryGetExecutablePath(processId, out var executablePath)
            && IsSameExecutablePath(executablePath, expectedExecutablePath);
    }

    internal static bool IsTrustedExecutablePath(string? executablePath)
        => IsSameExecutablePath(executablePath, ExecutablePath);

    private static bool IsSameExecutablePath(string? executablePath, string? expectedExecutablePath)
    {
        if (string.IsNullOrWhiteSpace(executablePath)
            || string.IsNullOrWhiteSpace(expectedExecutablePath)
            || !Path.IsPathFullyQualified(executablePath))
        {
            return false;
        }

        try
        {
            return Path.GetFullPath(executablePath).Equals(
                Path.GetFullPath(expectedExecutablePath),
                StringComparison.OrdinalIgnoreCase);
        }
        catch (Exception exception) when (
            exception is ArgumentException
                or NotSupportedException
                or PathTooLongException)
        {
            return false;
        }
    }

    private static bool TryGetExecutablePath(uint processId, out string executablePath)
    {
        executablePath = string.Empty;
        var processHandle = OpenProcess(ProcessQueryLimitedInformation, false, processId);
        if (processHandle == 0)
        {
            return false;
        }

        try
        {
            var capacity = 1024;
            var buffer = new StringBuilder(capacity);
            if (!QueryFullProcessImageNameW(processHandle, 0, buffer, ref capacity))
            {
                return false;
            }

            executablePath = buffer.ToString();
            return executablePath.Length > 0;
        }
        finally
        {
            CloseHandle(processHandle);
        }
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern nint OpenProcess(
        uint desiredAccess,
        [MarshalAs(UnmanagedType.Bool)] bool inheritHandle,
        uint processId);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool QueryFullProcessImageNameW(
        nint processHandle,
        uint flags,
        StringBuilder executableName,
        ref int size);

    [DllImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(nint handle);
}
