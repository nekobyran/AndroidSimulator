using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Threading.Channels;
using AndroidSimulator.App.Models;

namespace AndroidSimulator.App.Services;

public sealed class KeyBindingDispatchEventArgs(
    KeyBinding binding,
    SimulatorCommandResult<JsonElement?> result) : EventArgs
{
    public KeyBinding Binding { get; } = binding;

    public SimulatorCommandResult<JsonElement?> Result { get; } = result;
}

public sealed class KeyboardMappingService : IDisposable, IAsyncDisposable
{
    private const int WhKeyboardLl = 13;
    private const int WmKeyDown = 0x0100;
    private const int WmKeyUp = 0x0101;
    private const int WmSysKeyDown = 0x0104;
    private const int WmSysKeyUp = 0x0105;
    private const uint LlkhfInjected = 0x00000010;
    private const string HookMutexName = @"Local\AndroidSimulator.KeyboardMappingHook";

    private readonly object _lifecycleGate = new();
    private readonly LowLevelKeyboardProc _hookCallback;
    private readonly AppWindowService _appWindowService = new();
    private readonly Channel<KeyBindingDispatchRequest> _dispatchQueue;
    private readonly CancellationTokenSource _disposeCancellation = new();
    private readonly Task _dispatchWorker;
    private readonly KeyboardDispatchState _dispatchState = new();

    private Dictionary<int, KeyBinding> _activeMappings = [];
    private Thread? _hookLeaseThread;
    private ManualResetEventSlim? _hookLeaseRelease;
    private nint _hookHandle;
    private bool _isEnabled;
    private int _disposeStarted;
    private int _disposeResourcesFinished;
    public KeyboardMappingService()
    {
        _hookCallback = HookCallback;
        _dispatchQueue = Channel.CreateBounded<KeyBindingDispatchRequest>(
            new BoundedChannelOptions(128)
            {
                SingleReader = true,
                SingleWriter = true,
                FullMode = BoundedChannelFullMode.DropOldest,
                AllowSynchronousContinuations = false,
            });
        _dispatchWorker = Task.Run(DispatchLoopAsync);
    }

    public event EventHandler<KeyBindingDispatchEventArgs>? DispatchCompleted;

    public bool IsEnabled
    {
        get => Volatile.Read(ref _isEnabled);
        set => Volatile.Write(ref _isEnabled, value);
    }

    public bool IsRunning
    {
        get
        {
            lock (_lifecycleGate)
            {
                return _hookHandle != 0;
            }
        }
    }

    public bool HasActiveMappings => Volatile.Read(ref _activeMappings).Count > 0;

    public void ReplaceMappings(IEnumerable<KeyBinding> mappings)
    {
        ArgumentNullException.ThrowIfNull(mappings);
        ThrowIfDisposed();

        var next = new Dictionary<int, KeyBinding>();
        foreach (var mapping in mappings)
        {
            if (!IsDispatchable(mapping))
            {
                continue;
            }

            // The last enabled mapping wins, matching the latest entry in the editor.
            next[mapping.HostKey] = mapping;
        }

        Volatile.Write(ref _activeMappings, next);
    }

    public void Start()
    {
        ThrowIfDisposed();
        lock (_lifecycleGate)
        {
            ThrowIfDisposed();
            if (_hookHandle != 0)
            {
                return;
            }

            // A desktop shortcut starts another WinUI process. Only one process in the
            // current Windows session may own the global hook, otherwise every key fires
            // the same Android action more than once.
            if (!TryAcquireHookLeaseCore())
            {
                return;
            }

            var moduleHandle = GetModuleHandleW(null);
            var hookHandle = SetWindowsHookExW(
                WhKeyboardLl,
                _hookCallback,
                moduleHandle,
                0);
            if (hookHandle == 0)
            {
                ReleaseHookLeaseCore();
                throw new Win32Exception(
                    Marshal.GetLastWin32Error(),
                    "Failed to install the keyboard mapping hook.");
            }

            _hookHandle = hookHandle;
        }
    }

    public void Stop()
    {
        lock (_lifecycleGate)
        {
            StopCore();
        }
    }

    public void Dispose()
    {
        BeginDispose();
        try
        {
            _dispatchWorker.GetAwaiter().GetResult();
        }
        catch (OperationCanceledException)
        {
        }
        catch
        {
            // Dispose is a shutdown boundary; hook release has already completed.
        }
        finally
        {
            DisposeCancellationSource();
        }

        GC.SuppressFinalize(this);
    }

    public async ValueTask DisposeAsync()
    {
        BeginDispose();
        try
        {
            await _dispatchWorker.ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
        finally
        {
            DisposeCancellationSource();
        }

        GC.SuppressFinalize(this);
    }

    private nint HookCallback(int code, nint messagePointer, nint dataPointer)
    {
        try
        {
            if (code < 0 || !IsKeyboardMessage(messagePointer))
            {
                return CallNextHookEx(_hookHandle, code, messagePointer, dataPointer);
            }

            var hookData = Marshal.PtrToStructure<KbdLlHookStruct>(dataPointer);
            if ((hookData.Flags & LlkhfInjected) != 0)
            {
                return CallNextHookEx(_hookHandle, code, messagePointer, dataPointer);
            }

            var hostKey = checked((int)hookData.VirtualKeyCode);
            var mappings = Volatile.Read(ref _activeMappings);
            var isKeyDown = messagePointer == WmKeyDown || messagePointer == WmSysKeyDown;
            var isKeyUp = messagePointer == WmKeyUp || messagePointer == WmSysKeyUp;
            if (isKeyUp)
            {
                return _dispatchState.ShouldSwallowKeyUp(hostKey)
                    ? 1
                    : CallNextHookEx(_hookHandle, code, messagePointer, dataPointer);
            }

            if (!isKeyDown)
            {
                return CallNextHookEx(_hookHandle, code, messagePointer, dataPointer);
            }

            KeyBinding? mapping = null;
            AppWindowMetadata? appWindow = null;
            var canDispatch = IsEnabled
                && mappings.TryGetValue(hostKey, out mapping)
                && _appWindowService.TryGetForegroundTarget(out appWindow);
            var swallow = _dispatchState.ShouldSwallowKeyDown(
                hostKey,
                canDispatch,
                () => _dispatchQueue.Writer.TryWrite(
                    new KeyBindingDispatchRequest(mapping!, appWindow!.DisplayId)));

            // scrcpy already forwards ordinary keyboard input. Only configured custom
            // mappings are swallowed, and only for the verified virtual display window.
            return swallow
                ? 1
                : CallNextHookEx(_hookHandle, code, messagePointer, dataPointer);
        }
        catch
        {
            // A low-level hook must never take down the UI or block unrelated applications.
            return CallNextHookEx(_hookHandle, code, messagePointer, dataPointer);
        }
    }

    private async Task DispatchLoopAsync()
    {
        var cancellationToken = _disposeCancellation.Token;
        try
        {
            await foreach (var request in _dispatchQueue.Reader.ReadAllAsync(cancellationToken))
            {
                var binding = request.Binding;
                SimulatorCommandResult<JsonElement?> result = binding.ActionType switch
                {
                    KeyBindingActionType.Tap => await SimulatorCtlService.SendTapAsync(
                        request.DisplayId,
                        binding.TapX,
                        binding.TapY,
                        cancellationToken),
                    KeyBindingActionType.KeyEvent => await SimulatorCtlService.SendKeyEventAsync(
                        request.DisplayId,
                        binding.AndroidKeyCode,
                        cancellationToken),
                    _ => new SimulatorCommandResult<JsonElement?>(
                        false,
                        "invalid",
                        "Unsupported key mapping action.",
                        string.Empty,
                        -1,
                        null),
                };

                try
                {
                    DispatchCompleted?.Invoke(
                        this,
                        new KeyBindingDispatchEventArgs(binding, result));
                }
                catch
                {
                    // A consumer event handler must not stop future key dispatches.
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
    }

    private static bool IsKeyboardMessage(nint messagePointer) =>
        messagePointer == WmKeyDown
        || messagePointer == WmKeyUp
        || messagePointer == WmSysKeyDown
        || messagePointer == WmSysKeyUp;

    private static bool IsDispatchable(KeyBinding? mapping) =>
        mapping is not null
        && mapping.IsEnabled
        && mapping.HostKey is >= 1 and <= 255
        && mapping.ActionType switch
        {
            KeyBindingActionType.Tap => mapping.TapX >= 0 && mapping.TapY >= 0,
            KeyBindingActionType.KeyEvent => !string.IsNullOrWhiteSpace(mapping.AndroidKeyCode),
            _ => false,
        };

    private void BeginDispose()
    {
        if (Interlocked.Exchange(ref _disposeStarted, 1) != 0)
        {
            return;
        }

        IsEnabled = false;
        lock (_lifecycleGate)
        {
            StopCore();
        }

        _dispatchQueue.Writer.TryComplete();
        _disposeCancellation.Cancel();
    }

    private void StopCore()
    {
        if (_hookHandle == 0)
        {
            return;
        }

        var hookHandle = _hookHandle;
        _hookHandle = 0;
        _dispatchState.Clear();
        UnhookWindowsHookEx(hookHandle);
        ReleaseHookLeaseCore();
    }

    private bool TryAcquireHookLeaseCore()
    {
        var releaseSignal = new ManualResetEventSlim(false);
        var acquiredSource = new TaskCompletionSource<bool>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        var ownerThread = new Thread(
            () => RunHookLeaseOwner(releaseSignal, acquiredSource))
        {
            IsBackground = true,
            Name = "Android Simulator keyboard hook lease",
        };

        try
        {
            ownerThread.Start();
            var acquired = acquiredSource.Task.GetAwaiter().GetResult();
            if (!acquired)
            {
                ownerThread.Join();
                releaseSignal.Dispose();
                return false;
            }

            _hookLeaseThread = ownerThread;
            _hookLeaseRelease = releaseSignal;
            return true;
        }
        catch
        {
            releaseSignal.Set();
            if (ownerThread.IsAlive)
            {
                ownerThread.Join();
            }

            releaseSignal.Dispose();
            return false;
        }
    }

    private static void RunHookLeaseOwner(
        ManualResetEventSlim releaseSignal,
        TaskCompletionSource<bool> acquiredSource)
    {
        Mutex? mutex = null;
        var acquired = false;
        try
        {
            mutex = new Mutex(initiallyOwned: false, HookMutexName);
            try
            {
                acquired = mutex.WaitOne(0);
            }
            catch (AbandonedMutexException)
            {
                // The previous shortcut process ended without disposing; Windows gives
                // ownership to this waiter, so it is safe to install the replacement hook.
                acquired = true;
            }

            acquiredSource.TrySetResult(acquired);
            if (!acquired)
            {
                return;
            }

            releaseSignal.Wait();
        }
        catch
        {
            acquiredSource.TrySetResult(false);
        }
        finally
        {
            if (acquired && mutex is not null)
            {
                try
                {
                    mutex.ReleaseMutex();
                }
                catch (ApplicationException)
                {
                }
            }

            mutex?.Dispose();
        }
    }

    private void ReleaseHookLeaseCore()
    {
        var ownerThread = _hookLeaseThread;
        var releaseSignal = _hookLeaseRelease;
        _hookLeaseThread = null;
        _hookLeaseRelease = null;
        if (ownerThread is null || releaseSignal is null)
        {
            return;
        }

        releaseSignal.Set();
        ownerThread.Join();
        releaseSignal.Dispose();
    }

    private void ThrowIfDisposed() =>
        ObjectDisposedException.ThrowIf(
            Volatile.Read(ref _disposeStarted) != 0,
            this);

    private void DisposeCancellationSource()
    {
        if (Interlocked.Exchange(ref _disposeResourcesFinished, 1) == 0)
        {
            _disposeCancellation.Dispose();
        }
    }

    [StructLayout(LayoutKind.Sequential)]
    private readonly struct KbdLlHookStruct
    {
        public readonly uint VirtualKeyCode;
        public readonly uint ScanCode;
        public readonly uint Flags;
        public readonly uint Time;
        public readonly nuint ExtraInfo;
    }

    private sealed record KeyBindingDispatchRequest(
        KeyBinding Binding,
        uint DisplayId);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate nint LowLevelKeyboardProc(int code, nint wParam, nint lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern nint SetWindowsHookExW(
        int hookId,
        LowLevelKeyboardProc callback,
        nint moduleHandle,
        uint threadId);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnhookWindowsHookEx(nint hookHandle);

    [DllImport("user32.dll")]
    private static extern nint CallNextHookEx(
        nint hookHandle,
        int code,
        nint wParam,
        nint lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern nint GetModuleHandleW(string? moduleName);
}
