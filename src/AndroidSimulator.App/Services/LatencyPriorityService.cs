using System.Globalization;
using System.Runtime.InteropServices;
using System.Text.Json;

namespace AndroidSimulator.App.Services;

internal static class LatencyPriorityService
{
    private const string QemuExecutablePath =
        @"D:\vibecoding\sdk\msys64\ucrt64\bin\qemu-system-x86_64.exe";
    private const string QemuPidPath =
        @"D:\vibecoding\sdk\android-simulator-runtime\instances\android-simulator\qemu.pid";
    private const string SettingsPath =
        @"D:\vibecoding\sdk\android-simulator-runtime\settings.json";
    private const uint ProcessSetInformation = 0x0200;
    private const uint ProcessQueryLimitedInformation = 0x1000;
    private const uint AboveNormalPriorityClass = 0x00008000;
    private const uint BelowNormalPriorityClass = 0x00004000;
    private const uint NormalPriorityClass = 0x00000020;
    private const int ProcessPowerThrottling = 4;
    private const uint ProcessPowerThrottlingCurrentVersion = 1;
    private const uint ProcessPowerThrottlingExecutionSpeed = 1;

    public static void PromoteCurrentProcess()
    {
        var processHandle = GetCurrentProcess();
        if (processHandle == 0)
        {
            return;
        }

        ApplyInteractiveState(processHandle, NormalPriorityClass);
    }

    public static void Refresh(uint scrcpyProcessId)
    {
        var activePriorityClass = ReadActivePriorityClass();
        PromoteIfTrusted(scrcpyProcessId, TrustedAppWindowProcess.ExecutablePath, activePriorityClass);

        if (TryReadQemuPid(out var qemuProcessId))
        {
            PromoteIfTrusted(qemuProcessId, QemuExecutablePath, activePriorityClass);
        }
    }

    private static uint ReadActivePriorityClass()
    {
        if (!File.Exists(SettingsPath))
        {
            return NormalPriorityClass;
        }

        try
        {
            using var document = JsonDocument.Parse(File.ReadAllText(SettingsPath));
            if (!document.RootElement.TryGetProperty("performance_mode", out var mode))
            {
                return NormalPriorityClass;
            }

            return mode.GetString() switch
            {
                "eco" => BelowNormalPriorityClass,
                "performance" => AboveNormalPriorityClass,
                _ => NormalPriorityClass,
            };
        }
        catch (JsonException)
        {
            return NormalPriorityClass;
        }
        catch (IOException)
        {
            return NormalPriorityClass;
        }
        catch (UnauthorizedAccessException)
        {
            return NormalPriorityClass;
        }
    }

    private static bool TryReadQemuPid(out uint processId)
    {
        processId = 0;
        try
        {
            var text = File.ReadAllText(QemuPidPath).Trim();
            return text.Length is > 0 and <= 10
                && uint.TryParse(text, NumberStyles.None, CultureInfo.InvariantCulture, out processId)
                && processId != 0;
        }
        catch (Exception exception) when (
            exception is IOException
                or UnauthorizedAccessException
                or ArgumentException
                or NotSupportedException)
        {
            return false;
        }
    }

    private static void PromoteIfTrusted(
        uint processId,
        string expectedExecutablePath,
        uint priorityClass)
    {
        if (!TrustedAppWindowProcess.IsExpectedProcessId(processId, expectedExecutablePath))
        {
            return;
        }

        var processHandle = OpenProcess(
            ProcessSetInformation | ProcessQueryLimitedInformation,
            false,
            processId);
        if (processHandle == 0)
        {
            return;
        }

        try
        {
            ApplyInteractiveState(processHandle, priorityClass);
        }
        finally
        {
            CloseHandle(processHandle);
        }
    }

    private static void ApplyInteractiveState(nint processHandle, uint priorityClass)
    {
        _ = SetPriorityClass(processHandle, priorityClass);
        var powerState = new ProcessPowerThrottlingState
        {
            Version = ProcessPowerThrottlingCurrentVersion,
            ControlMask = ProcessPowerThrottlingExecutionSpeed,
            StateMask = 0,
        };
        _ = SetProcessInformation(
            processHandle,
            ProcessPowerThrottling,
            ref powerState,
            (uint)Marshal.SizeOf<ProcessPowerThrottlingState>());
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessPowerThrottlingState
    {
        public uint Version;
        public uint ControlMask;
        public uint StateMask;
    }

    [DllImport("kernel32.dll")]
    private static extern nint GetCurrentProcess();

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern nint OpenProcess(
        uint desiredAccess,
        [MarshalAs(UnmanagedType.Bool)] bool inheritHandle,
        uint processId);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetPriorityClass(nint processHandle, uint priorityClass);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetProcessInformation(
        nint processHandle,
        int processInformationClass,
        ref ProcessPowerThrottlingState processInformation,
        uint processInformationSize);

    [DllImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(nint handle);
}
