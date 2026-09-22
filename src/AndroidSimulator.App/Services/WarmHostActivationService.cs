using System.IO.Pipes;
using System.Security.Cryptography;
using System.Text;

namespace AndroidSimulator.App.Services;

internal sealed class WarmHostActivationService : IDisposable
{
    private const string ShowCommand = "SHOW";
    private static readonly Encoding WireEncoding = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false);
    private readonly CancellationTokenSource _cancellation = new();
    private readonly Action _activate;
    private readonly string _pipeName;
    private readonly Task _serverTask;

    public WarmHostActivationService(string packageName, Action activate)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(packageName);
        ArgumentNullException.ThrowIfNull(activate);
        _activate = activate;
        _pipeName = GetPipeName(packageName);
        _serverTask = RunServerAsync(_cancellation.Token);
    }

    internal static string GetPipeName(string packageName)
    {
        var hash = SHA256.HashData(Encoding.UTF8.GetBytes(packageName.Trim()));
        return $"AndroidSimulator.WarmHost.{Convert.ToHexString(hash.AsSpan(0, 12))}";
    }

    public static bool TryActivateExisting(string packageName, int timeoutMilliseconds = 40)
    {
        if (string.IsNullOrWhiteSpace(packageName) || timeoutMilliseconds < 1)
        {
            return false;
        }

        try
        {
            using var client = new NamedPipeClientStream(
                ".",
                GetPipeName(packageName),
                PipeDirection.InOut,
                PipeOptions.CurrentUserOnly);
            client.Connect(timeoutMilliseconds);
            using var writer = new StreamWriter(
                client,
                WireEncoding,
                bufferSize: 1024,
                leaveOpen: true)
            {
                AutoFlush = true,
            };
            using var reader = new StreamReader(
                client,
                WireEncoding,
                detectEncodingFromByteOrderMarks: false,
                bufferSize: 1024,
                leaveOpen: true);
            writer.WriteLine(ShowCommand);
            return reader.ReadLine()?.Equals("OK", StringComparison.Ordinal) == true;
        }
        catch (Exception exception) when (
            exception is IOException
                or TimeoutException
                or UnauthorizedAccessException)
        {
            return false;
        }
    }

    public void Dispose()
    {
        _cancellation.Cancel();
        try
        {
            _serverTask.Wait(TimeSpan.FromMilliseconds(100));
        }
        catch (AggregateException)
        {
        }
        _cancellation.Dispose();
    }

    private async Task RunServerAsync(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested)
        {
            try
            {
                await using var server = new NamedPipeServerStream(
                    _pipeName,
                    PipeDirection.InOut,
                    1,
                    PipeTransmissionMode.Byte,
                    PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
                await server.WaitForConnectionAsync(cancellationToken);
                using var reader = new StreamReader(
                    server,
                    WireEncoding,
                    detectEncodingFromByteOrderMarks: false,
                    bufferSize: 1024,
                    leaveOpen: true);
                using var writer = new StreamWriter(
                    server,
                    WireEncoding,
                    bufferSize: 1024,
                    leaveOpen: true)
                {
                    AutoFlush = true,
                };
                if ((await reader.ReadLineAsync(cancellationToken))?.Equals(
                    ShowCommand,
                    StringComparison.Ordinal) == true)
                {
                    _activate();
                    await writer.WriteLineAsync("OK");
                }
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                return;
            }
            catch (IOException)
            {
            }
        }
    }
}

internal sealed class PackageHostLease : IDisposable
{
    private readonly int _ownerThreadId;
    private Mutex? _mutex;

    private PackageHostLease(Mutex mutex)
    {
        _mutex = mutex;
        _ownerThreadId = Environment.CurrentManagedThreadId;
    }

    internal static string GetMutexName(string packageName)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(packageName);
        var hash = SHA256.HashData(Encoding.UTF8.GetBytes(packageName.Trim()));
        return $@"Local\AndroidSimulator.PackageHost.{Convert.ToHexString(hash.AsSpan(0, 12))}";
    }

    public static bool TryAcquire(string packageName, out PackageHostLease? lease)
    {
        lease = null;
        if (string.IsNullOrWhiteSpace(packageName))
        {
            return false;
        }

        Mutex? mutex = null;
        try
        {
            mutex = new Mutex(initiallyOwned: false, GetMutexName(packageName));
            var acquired = false;
            try
            {
                acquired = mutex.WaitOne(0);
            }
            catch (AbandonedMutexException)
            {
                // The previous host terminated without releasing the lease. Windows
                // transfers ownership to this caller, so recovery may safely proceed.
                acquired = true;
            }

            if (!acquired)
            {
                mutex.Dispose();
                return false;
            }

            lease = new PackageHostLease(mutex);
            return true;
        }
        catch (Exception exception) when (
            exception is UnauthorizedAccessException
                or IOException
                or WaitHandleCannotBeOpenedException)
        {
            mutex?.Dispose();
            return false;
        }
    }

    public void Dispose()
    {
        var mutex = Interlocked.Exchange(ref _mutex, null);
        if (mutex is null)
        {
            return;
        }

        try
        {
            if (Environment.CurrentManagedThreadId == _ownerThreadId)
            {
                mutex.ReleaseMutex();
            }
        }
        catch (ApplicationException)
        {
        }
        finally
        {
            mutex.Dispose();
        }
    }
}

internal sealed class HostedWindowRecoveryGate
{
    private int _closed;
    private int _recovering;

    public bool TryBegin(bool isHostedWindowHealthy)
    {
        return !isHostedWindowHealthy
            && Volatile.Read(ref _closed) == 0
            && Interlocked.CompareExchange(ref _recovering, 1, 0) == 0;
    }

    public void Complete() => Interlocked.Exchange(ref _recovering, 0);

    public void Close() => Interlocked.Exchange(ref _closed, 1);
}
