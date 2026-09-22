using System.Windows;
using AndroidSimulator.Installer.Services;

namespace AndroidSimulator.Installer;

public partial class App : System.Windows.Application
{
    protected override async void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        var args = e.Args ?? Array.Empty<string>();
        if (HasFlag(args, "--silent") || HasFlag(args, "/S") || HasFlag(args, "-s"))
        {
            await RunSilentInstallAsync(args);
            Shutdown();
            return;
        }

        var window = new MainWindow();
        MainWindow = window;
        window.Show();
    }

    private static async Task RunSilentInstallAsync(string[] args)
    {
        try
        {
            var dir = GetArgValue(args, "--dir")
                ?? GetArgValue(args, "/D")
                ?? InstallService.DefaultInstallDirectory;
            var options = new InstallOptions
            {
                InstallDirectory = dir,
                CreateDesktopShortcut = !HasFlag(args, "--no-desktop"),
                CreateStartMenuShortcut = !HasFlag(args, "--no-startmenu"),
                LaunchAfterInstall = HasFlag(args, "--launch"),
            };

            if (!InstallService.HasEmbeddedPayload())
            {
                Console.Error.WriteLine("ERROR: installer payload missing");
                Environment.ExitCode = 2;
                return;
            }

            var progress = new Progress<InstallProgress>(p =>
                Console.WriteLine($"[{p.Percent:0}%] {p.Phase}: {p.Detail}"));
            await InstallService.InstallAsync(options, progress, CancellationToken.None);
            Console.WriteLine("OK: installed to " + options.InstallDirectory);
            Environment.ExitCode = 0;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine("ERROR: " + exception.Message);
            Environment.ExitCode = 1;
        }
    }

    private static bool HasFlag(string[] args, string flag) =>
        args.Any(a => string.Equals(a, flag, StringComparison.OrdinalIgnoreCase));

    private static string? GetArgValue(string[] args, string name)
    {
        for (var i = 0; i < args.Length; i++)
        {
            if (!string.Equals(args[i], name, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            if (i + 1 < args.Length)
            {
                return args[i + 1];
            }
        }

        // Support --dir=C:\path
        var prefix = name + "=";
        var match = args.FirstOrDefault(a => a.StartsWith(prefix, StringComparison.OrdinalIgnoreCase));
        return match is null ? null : match[prefix.Length..];
    }
}
