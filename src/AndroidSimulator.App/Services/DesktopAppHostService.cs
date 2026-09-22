using System.Diagnostics;

namespace AndroidSimulator.App.Services;

internal static class DesktopAppHostService
{
    public static bool TryLaunch(
        string packageName,
        string applicationName,
        out int processId,
        out string error)
    {
        processId = 0;
        error = string.Empty;
        try
        {
            var launcher = SimulatorCtlService.ResolveHostLauncher();
            if (!File.Exists(launcher))
            {
                error = $"Android Simulator 启动器不存在：{launcher}";
                return false;
            }

            var startInfo = new ProcessStartInfo
            {
                FileName = launcher,
                WorkingDirectory = Path.GetDirectoryName(launcher) ?? AppContext.BaseDirectory,
                UseShellExecute = false,
            };
            foreach (var argument in ApkActivationService.BuildPackageHostArguments(
                         packageName,
                         applicationName))
            {
                startInfo.ArgumentList.Add(argument);
            }

            using var process = Process.Start(startInfo);
            if (process is null)
            {
                error = "Windows 未能创建独立应用宿主进程。";
                return false;
            }

            processId = process.Id;
            return true;
        }
        catch (Exception exception) when (
            exception is ArgumentException
                or InvalidOperationException
                or System.ComponentModel.Win32Exception
                or NotSupportedException)
        {
            error = exception.Message;
            return false;
        }
    }
}
