using System.Diagnostics;
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

    private static readonly object AppliedStateGate = new();
    private static readonly Dictionary<uint, SchedulerDecision> AppliedStates = [];

    public static void PromoteCurrentProcess() => SetCurrentProcessActivity(true);

    public static void SetCurrentProcessActivity(bool foreground)
    {
        var decision = PerformanceSchedulerPolicy.Resolve(
            ReadPerformanceMode(),
            SchedulerProcessRole.Ui,
            foreground ? SchedulerActivity.Foreground : SchedulerActivity.Background);
        ApplyIfChanged((uint)Environment.ProcessId, GetCurrentProcess(), decision);
    }

    public static void Refresh(uint scrcpyProcessId, bool foreground = true)
    {
        var performanceMode = ReadPerformanceMode();
        if (scrcpyProcessId != 0)
        {
            var scrcpyDecision = PerformanceSchedulerPolicy.Resolve(
                performanceMode,
                SchedulerProcessRole.Scrcpy,
                foreground ? SchedulerActivity.Foreground : SchedulerActivity.Background);
            ApplyIfTrusted(scrcpyProcessId, TrustedAppWindowProcess.ExecutablePath, scrcpyDecision);
        }

        if (TryReadQemuPid(out var qemuProcessId))
        {
            // QEMU is shared by all application windows. A background host must
            // not demote the shared guest while another package may be active.
            // The Rust control plane owns the transition to true Idle.
            var qemuDecision = PerformanceSchedulerPolicy.Resolve(
                performanceMode,
                SchedulerProcessRole.Qemu,
                SchedulerActivity.Foreground);
            ApplyIfTrusted(qemuProcessId, QemuExecutablePath, qemuDecision);
        }
    }

    private static string ReadPerformanceMode()
    {
        if (!File.Exists(SettingsPath))
        {
            return "balanced";
        }

        try
        {
            using var document = JsonDocument.Parse(File.ReadAllText(SettingsPath));
            return document.RootElement.TryGetProperty("performance_mode", out var mode)
                ? mode.GetString() ?? "balanced"
                : "balanced";
        }
        catch (Exception exception) when (
            exception is JsonException or IOException or UnauthorizedAccessException)
        {
            return "balanced";
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

    private static void ApplyIfTrusted(
        uint processId,
        string expectedExecutablePath,
        SchedulerDecision decision)
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
            ApplyIfChanged(processId, processHandle, decision);
        }
        finally
        {
            CloseHandle(processHandle);
        }
    }

    private static void ApplyIfChanged(
        uint processId,
        nint processHandle,
        SchedulerDecision decision)
    {
        lock (AppliedStateGate)
        {
            if (AppliedStates.TryGetValue(processId, out var applied) && applied == decision)
            {
                return;
            }
        }

        if (!SetPriorityClass(processHandle, ToPriorityClass(decision.PriorityClass)))
        {
            return;
        }

        var powerState = new ProcessPowerThrottlingState
        {
            Version = ProcessPowerThrottlingCurrentVersion,
            ControlMask = ProcessPowerThrottlingExecutionSpeed,
            StateMask = decision.EcoQos ? ProcessPowerThrottlingExecutionSpeed : 0,
        };
        if (!SetProcessInformation(
            processHandle,
            ProcessPowerThrottling,
            ref powerState,
            (uint)Marshal.SizeOf<ProcessPowerThrottlingState>()))
        {
            return;
        }

        lock (AppliedStateGate)
        {
            AppliedStates[processId] = decision;
        }
    }

    private static uint ToPriorityClass(ProcessPriorityClass priorityClass) =>
        priorityClass switch
        {
            ProcessPriorityClass.AboveNormal => AboveNormalPriorityClass,
            ProcessPriorityClass.BelowNormal => BelowNormalPriorityClass,
            _ => NormalPriorityClass,
        };

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
