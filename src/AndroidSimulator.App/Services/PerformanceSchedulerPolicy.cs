using System.Diagnostics;

namespace AndroidSimulator.App.Services;

internal enum SchedulerProcessRole { Ui, Scrcpy, Qemu }
internal enum SchedulerActivity { Foreground, Background, Idle }

internal readonly record struct SchedulerDecision(
    ProcessPriorityClass PriorityClass,
    bool EcoQos);

internal static class PerformanceSchedulerPolicy
{
    public static SchedulerDecision Resolve(
        string? performanceMode,
        SchedulerProcessRole role,
        SchedulerActivity activity)
    {
        var mode = string.IsNullOrWhiteSpace(performanceMode)
            ? "balanced"
            : performanceMode.Trim().ToLowerInvariant();

        if (activity == SchedulerActivity.Idle)
        {
            return new SchedulerDecision(ProcessPriorityClass.BelowNormal, true);
        }

        if (activity == SchedulerActivity.Background)
        {
            if (role == SchedulerProcessRole.Qemu)
            {
                return mode switch
                {
                    "performance" => new SchedulerDecision(ProcessPriorityClass.AboveNormal, true),
                    "eco" => new SchedulerDecision(ProcessPriorityClass.BelowNormal, true),
                    _ => new SchedulerDecision(ProcessPriorityClass.Normal, true),
                };
            }

            return mode == "performance"
                ? new SchedulerDecision(ProcessPriorityClass.Normal, true)
                : new SchedulerDecision(ProcessPriorityClass.BelowNormal, true);
        }

        return mode switch
        {
            "eco" => new SchedulerDecision(ProcessPriorityClass.BelowNormal, true),
            "performance" => new SchedulerDecision(ProcessPriorityClass.AboveNormal, false),
            _ => new SchedulerDecision(ProcessPriorityClass.Normal, false),
        };
    }
}
