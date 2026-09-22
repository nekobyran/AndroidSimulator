using System.Diagnostics;
using System.IO.Compression;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32;

namespace AndroidSimulator.Installer.Services;

public sealed class InstallOptions
{
    public required string InstallDirectory { get; init; }
    public bool CreateDesktopShortcut { get; init; } = true;
    public bool CreateStartMenuShortcut { get; init; } = true;
    public bool LaunchAfterInstall { get; init; } = true;
}

public sealed class InstallProgress
{
    public double Percent { get; init; }
    public string Phase { get; init; } = string.Empty;
    public string Detail { get; init; } = string.Empty;
}

public static class InstallService
{
    public const string ProductName = "Android Simulator";
    public const string ExeName = "AndroidSimulator.App.exe";
    public const string UninstallKeyName = "AndroidSimulator";
    public const string Version = "1.0.0";

    public static string DefaultInstallDirectory =>
        System.IO.Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "AndroidSimulator");

    public static bool HasEmbeddedPayload()
    {
        var assembly = Assembly.GetExecutingAssembly();
        return assembly.GetManifestResourceNames()
            .Any(name => name.EndsWith("app.zip", StringComparison.OrdinalIgnoreCase));
    }

    public static async Task InstallAsync(
        InstallOptions options,
        IProgress<InstallProgress>? progress,
        CancellationToken cancellationToken)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(options.InstallDirectory);

        var installDir = System.IO.Path.GetFullPath(options.InstallDirectory.Trim());
        Report(progress, 2, "准备安装", installDir);

        System.IO.Directory.CreateDirectory(installDir);
        var stagingZip = System.IO.Path.Combine(
            System.IO.Path.GetTempPath(),
            $"android-simulator-payload-{Guid.NewGuid():N}.zip");

        try
        {
            Report(progress, 8, "读取安装包", "正在解压内嵌应用文件…");
            await ExtractEmbeddedPayloadAsync(stagingZip, cancellationToken).ConfigureAwait(false);

            Report(progress, 18, "清理旧文件", "准备目标目录…");
            await Task.Run(() => CleanInstallDirectory(installDir), cancellationToken).ConfigureAwait(false);

            Report(progress, 28, "复制文件", "正在安装应用程序文件…");
            await Task.Run(
                    () => ZipFile.ExtractToDirectory(stagingZip, installDir, overwriteFiles: true),
                    cancellationToken)
                .ConfigureAwait(false);

            var exePath = System.IO.Path.Combine(installDir, ExeName);
            if (!System.IO.File.Exists(exePath))
            {
                throw new InvalidOperationException($"安装包缺少主程序：{ExeName}");
            }

            Report(progress, 72, "注册卸载信息", "写入卸载脚本与应用列表…");
            await Task.Run(
                    () =>
                    {
                        WriteUninstallScript(installDir, exePath);
                        WriteUninstallRegistry(installDir, exePath);
                    },
                    cancellationToken)
                .ConfigureAwait(false);

            Report(progress, 86, "写入快捷方式", "创建启动入口…");
            await Task.Run(
                    () =>
                    {
                        if (options.CreateDesktopShortcut)
                        {
                            CreateShortcut(
                                System.IO.Path.Combine(
                                    Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
                                    $"{ProductName}.lnk"),
                                exePath,
                                installDir,
                                ProductName);
                        }

                        if (options.CreateStartMenuShortcut)
                        {
                            var startMenuDir = System.IO.Path.Combine(
                                Environment.GetFolderPath(Environment.SpecialFolder.StartMenu),
                                "Programs",
                                ProductName);
                            System.IO.Directory.CreateDirectory(startMenuDir);
                            CreateShortcut(
                                System.IO.Path.Combine(startMenuDir, $"{ProductName}.lnk"),
                                exePath,
                                installDir,
                                ProductName);
                            CreateShortcut(
                                System.IO.Path.Combine(startMenuDir, $"卸载 {ProductName}.lnk"),
                                System.IO.Path.Combine(installDir, "Uninstall.cmd"),
                                installDir,
                                $"卸载 {ProductName}");
                        }
                    },
                    cancellationToken)
                .ConfigureAwait(false);

            Report(progress, 100, "安装完成", $"{ProductName} 已安装到 {installDir}");

            if (options.LaunchAfterInstall)
            {
                Process.Start(new ProcessStartInfo
                {
                    FileName = exePath,
                    WorkingDirectory = installDir,
                    UseShellExecute = true,
                });
            }
        }
        finally
        {
            try
            {
                if (System.IO.File.Exists(stagingZip))
                {
                    System.IO.File.Delete(stagingZip);
                }
            }
            catch
            {
                // ignore temp cleanup failures
            }
        }
    }

    private static async Task ExtractEmbeddedPayloadAsync(string stagingZip, CancellationToken cancellationToken)
    {
        var assembly = Assembly.GetExecutingAssembly();
        var resourceName = assembly.GetManifestResourceNames()
            .FirstOrDefault(name => name.EndsWith("app.zip", StringComparison.OrdinalIgnoreCase));
        if (resourceName is null)
        {
            throw new InvalidOperationException(
                "安装器未包含应用负载。请先运行 PackageInstaller 从 Release 打包。");
        }

        await using var stream = assembly.GetManifestResourceStream(resourceName)
            ?? throw new InvalidOperationException("无法读取内嵌安装负载。");
        await using var file = System.IO.File.Create(stagingZip);
        await stream.CopyToAsync(file, cancellationToken).ConfigureAwait(false);
    }

    private static void CleanInstallDirectory(string installDir)
    {
        if (!System.IO.Directory.Exists(installDir))
        {
            return;
        }

        foreach (var file in System.IO.Directory.EnumerateFiles(
                     installDir,
                     "*",
                     System.IO.SearchOption.AllDirectories))
        {
            try
            {
                System.IO.File.SetAttributes(file, System.IO.FileAttributes.Normal);
                System.IO.File.Delete(file);
            }
            catch
            {
                // keep going; overwrite will handle residual locks
            }
        }
    }

    private static void WriteUninstallScript(string installDir, string exePath)
    {
        var uninstallCmd = System.IO.Path.Combine(installDir, "Uninstall.cmd");
        var script = new StringBuilder();
        script.AppendLine("@echo off");
        script.AppendLine("setlocal");
        script.AppendLine($"set \"INSTALL_DIR={installDir}\"");
        script.AppendLine($"taskkill /F /IM \"{ExeName}\" >nul 2>nul");
        script.AppendLine("timeout /t 1 /nobreak >nul");
        script.AppendLine("reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\AndroidSimulator\" /f >nul 2>nul");
        script.AppendLine($"del /q \"%USERPROFILE%\\Desktop\\{ProductName}.lnk\" >nul 2>nul");
        script.AppendLine($"rmdir /s /q \"%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs\\{ProductName}\" >nul 2>nul");
        script.AppendLine("cd /d \"%TEMP%\"");
        script.AppendLine("rmdir /s /q \"%INSTALL_DIR%\" >nul 2>nul");
        script.AppendLine("echo Android Simulator has been uninstalled.");
        script.AppendLine("pause");
        System.IO.File.WriteAllText(uninstallCmd, script.ToString(), Encoding.ASCII);
        _ = exePath;
    }

    private static void WriteUninstallRegistry(string installDir, string exePath)
    {
        using var key = Registry.CurrentUser.CreateSubKey(
            $@"Software\Microsoft\Windows\CurrentVersion\Uninstall\{UninstallKeyName}");
        if (key is null)
        {
            return;
        }

        var uninstallCmd = System.IO.Path.Combine(installDir, "Uninstall.cmd");
        key.SetValue("DisplayName", ProductName);
        key.SetValue("DisplayVersion", Version);
        key.SetValue("Publisher", "Android Simulator");
        key.SetValue("InstallLocation", installDir);
        key.SetValue("DisplayIcon", exePath);
        key.SetValue("UninstallString", $"\"{uninstallCmd}\"");
        key.SetValue("NoModify", 1, RegistryValueKind.DWord);
        key.SetValue("NoRepair", 1, RegistryValueKind.DWord);
        try
        {
            var sizeKb = System.IO.Directory.EnumerateFiles(
                    installDir,
                    "*",
                    System.IO.SearchOption.AllDirectories)
                .Select(path => new System.IO.FileInfo(path).Length)
                .Sum() / 1024L;
            key.SetValue("EstimatedSize", (int)Math.Min(int.MaxValue, sizeKb), RegistryValueKind.DWord);
        }
        catch
        {
            // optional
        }
    }

    private static void CreateShortcut(string shortcutPath, string targetPath, string workingDirectory, string description)
    {
        System.IO.Directory.CreateDirectory(System.IO.Path.GetDirectoryName(shortcutPath)!);
        var shellType = Type.GetTypeFromProgID("WScript.Shell")
            ?? throw new InvalidOperationException("无法创建快捷方式：WScript.Shell 不可用。");
        dynamic shell = Activator.CreateInstance(shellType)!;
        var shortcut = shell.CreateShortcut(shortcutPath);
        shortcut.TargetPath = targetPath;
        shortcut.WorkingDirectory = workingDirectory;
        shortcut.Description = description;
        shortcut.IconLocation = targetPath;
        shortcut.Save();
        Marshal.FinalReleaseComObject(shortcut);
        Marshal.FinalReleaseComObject(shell);
    }

    private static void Report(IProgress<InstallProgress>? progress, double percent, string phase, string detail)
    {
        progress?.Report(new InstallProgress
        {
            Percent = percent,
            Phase = phase,
            Detail = detail,
        });
    }
}
