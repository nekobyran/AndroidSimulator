using System.Runtime.InteropServices;

namespace AndroidSimulator.App.Services;

internal static class LatencyPriorityService
{
    private const string NativeLibrary = "AndroidSimulator.Core.dll";

    private enum NativePerformanceMode : uint
    {
        Eco = 0,
        Balanced = 1,
        Performance = 2,
        Custom = 3,
    }

    private enum NativeSchedulerActivity : uint
    {
        Foreground = 0,
        Background = 1,
    }

    public static void PromoteCurrentProcess()
        => ApplyCurrentProcess("balanced", foreground: true);

    public static void SetCurrentProcessActivity(string performanceMode, bool foreground)
        => ApplyCurrentProcess(performanceMode, foreground);

    private static void ApplyCurrentProcess(string performanceMode, bool foreground)
    {
        var mode = performanceMode switch
        {
            "eco" => NativePerformanceMode.Eco,
            "balanced" => NativePerformanceMode.Balanced,
            "performance" => NativePerformanceMode.Performance,
            "custom" => NativePerformanceMode.Custom,
            _ => throw new InvalidOperationException(
                $"Unsupported performance mode '{performanceMode}'."),
        };
        var activity = foreground
            ? NativeSchedulerActivity.Foreground
            : NativeSchedulerActivity.Background;

        var result = AndroidSimulatorSetCurrentProcessActivity((uint)mode, (uint)activity);
        if (result != 0)
        {
            throw new InvalidOperationException(
                $"Rust scheduler core rejected the UI process activity update with code {result}.");
        }
    }

    [DllImport(
        NativeLibrary,
        EntryPoint = "AndroidSimulatorSetCurrentProcessActivity",
        ExactSpelling = true,
        CallingConvention = CallingConvention.Winapi)]
    private static extern int AndroidSimulatorSetCurrentProcessActivity(uint mode, uint activity);
}
