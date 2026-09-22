using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.Win32;
using Windows.Graphics;

namespace AndroidSimulator.App.Services;

public sealed class AppWindowService
{
    private const int SwMinimize = 6;
    private const int SwRestore = 9;
    private const long ForegroundCacheMilliseconds = 200;
    private const int GwlStyle = -16;
    private const int GwlExStyle = -20;
    private const int GwlHwndParent = -8;
    private const uint GwOwner = 4;
    private const long WsChild = 0x40000000L;
    private const long WsPopup = 0x80000000L;
    private const long WsCaption = 0x00C00000L;
    private const long WsThickFrame = 0x00040000L;
    private const long WsSysMenu = 0x00080000L;
    private const long WsMinimizeBox = 0x00020000L;
    private const long WsMaximizeBox = 0x00010000L;
    private const long WsVisible = 0x10000000L;
    private const long WsClipSiblings = 0x04000000L;
    private const long WsClipChildren = 0x02000000L;
    private const long WsExToolWindow = 0x00000080L;
    private const long WsExAppWindow = 0x00040000L;
    private const uint SwpNoZOrder = 0x0004;
    private const uint SwpNoActivate = 0x0010;
    private const uint SwpFrameChanged = 0x0020;
    private const uint SwpShowWindow = 0x0040;
    private const uint SwpNoMove = 0x0002;
    private const uint SwpNoSize = 0x0001;
    private const int WmClose = 0x0010;
    private const uint InputKeyboard = 1;
    private const uint KeyEventFKeyUp = 0x0002;
    private const ushort VkLeft = 0x25;
    private const ushort VkRight = 0x27;
    private const ushort VkMenu = 0x12;
    private const uint ColorRefInvalid = 0xFFFFFFFF;
    private const uint PwClientOnly = 0x00000001;
    private const uint PwRenderFullContent = 0x00000002;
    private const int GaRoot = 2;
    private static readonly nint HwndTop = 0;
    private static readonly nint HwndTopMost = -1;
    private static readonly nint HwndNoTopMost = -2;

    [StructLayout(LayoutKind.Sequential)]
    private struct NativePoint(int x, int y)
    {
        public int X = x;
        public int Y = y;
    }

    private readonly object _foregroundCacheGate = new();
    private readonly object _hostFocusGate = new();
    private nint _cachedForegroundWindow;
    private uint _cachedForegroundProcessId;
    private AppWindowMetadata? _cachedForegroundTarget;
    private long _foregroundCacheExpiresAt;
    private nint _hostFocusWindow;
    private AppWindowMetadata? _hostFocusTarget;

    internal static string DescribeLaunchFailure(
        SimulatorCommandResult<JsonElement?> result)
    {
        ArgumentNullException.ThrowIfNull(result);
        if (result.Status.Equals("unsupported", StringComparison.OrdinalIgnoreCase))
        {
            var actualApi = TryGetPositiveInt32(result.Data, "android_api_level");
            var requiredApi = TryGetPositiveInt32(result.Data, "required_android_api_level") ?? 30;
            var actualText = actualApi is int value
                ? $"当前镜像 API {value}"
                : "当前镜像版本";
            return $"{actualText} 不支持独立应用虚拟显示；需要 Android 11 / API {requiredApi} 或更高版本。"
                + "为保证每个应用拥有正确的独立窗口，本应用不会回退为 Android 整机或 QEMU 窗口。";
        }

        var detail = string.IsNullOrWhiteSpace(result.Message)
            ? "控制组件未能创建独立应用窗口。"
            : result.Message.Trim();
        return $"{detail} 本应用不会猜测其他窗口，也不会回退为 Android 整机或 QEMU 窗口。";
    }

    internal bool TryBringToFront(AppWindowMetadata metadata)
    {
        ArgumentNullException.ThrowIfNull(metadata);
        if (!TryResolveTrustedWindow(metadata, out var windowHandle))
        {
            return false;
        }

        try
        {
            if (IsIconic(windowHandle))
            {
                _ = ShowWindowAsync(windowHandle, SwRestore);
            }

            ApplyWindowMaterial(windowHandle);

            if (GetForegroundWindow() == windowHandle)
            {
                return true;
            }

            _ = BringWindowToTop(windowHandle);
            return SetForegroundWindow(windowHandle)
                || GetForegroundWindow() == windowHandle;
        }
        catch
        {
            return false;
        }
    }

    internal bool TryCreateHostTarget(
        AppWindowMetadata metadata,
        out HostedAppWindow target,
        out string failure)
    {
        target = null!;
        failure = string.Empty;
        ArgumentNullException.ThrowIfNull(metadata);
        if (!TryResolveTrustedWindow(metadata, out var windowHandle))
        {
            failure = "没有找到由中央 scrcpy 启动的可信独立应用窗口；不会嵌入未知 HWND。";
            return false;
        }

        target = new HostedAppWindow(
            metadata.WindowProcessId,
            windowHandle,
            metadata.DisplayId,
            metadata.PackageName,
            metadata.Title);
        return true;
    }

    internal bool TryAttachToHost(
        HostedAppWindow target,
        nint hostWindowHandle,
        RectInt32 clientBounds,
        out string failure)
    {
        failure = string.Empty;
        ArgumentNullException.ThrowIfNull(target);
        if (hostWindowHandle == 0 || target.WindowHandle == 0)
        {
            failure = "宿主窗口或 Android 应用窗口句柄无效。";
            return false;
        }

        if (!TrustedAppWindowProcess.IsTrustedProcessId(target.ProcessId)
            || !IsWindow(target.WindowHandle))
        {
            failure = "目标 Android 应用窗口已关闭或不再属于可信 scrcpy 进程。";
            return false;
        }

        try
        {
            if (IsIconic(target.WindowHandle))
            {
                _ = ShowWindowAsync(target.WindowHandle, SwRestore);
            }

            var style = GetWindowLongPtr(target.WindowHandle, GwlStyle).ToInt64();
            style &= ~(WsPopup | WsCaption | WsThickFrame | WsSysMenu | WsMinimizeBox | WsMaximizeBox);
            style &= ~WsChild;
            style |= WsPopup | WsVisible | WsClipSiblings | WsClipChildren;
            _ = SetWindowLongPtr(target.WindowHandle, GwlStyle, new nint(style));

            var exStyle = GetWindowLongPtr(target.WindowHandle, GwlExStyle).ToInt64();
            exStyle &= ~WsExAppWindow;
            exStyle |= WsExToolWindow;
            _ = SetWindowLongPtr(target.WindowHandle, GwlExStyle, new nint(exStyle));

            _ = SetParent(target.WindowHandle, 0);
            _ = SetWindowLongPtr(target.WindowHandle, GwlHwndParent, hostWindowHandle);
            if (GetWindow(target.WindowHandle, GwOwner) != hostWindowHandle)
            {
                failure = "无法把 scrcpy 画面窗口绑定到 WinUI 宿主。";
                return false;
            }

            // The previous hidden host remains as the single warm broker for
            // this scrcpy process. Closing it here can race Windows App SDK
            // ownership transfer and tear down the newly visible host.

            ApplyWindowMaterial(hostWindowHandle);
            return TryLayoutHostedWindowCore(target, clientBounds, refreshFrame: true, out failure);
        }
        catch (Exception exception) when (
            exception is InvalidOperationException
                or ExternalException
                or ArithmeticException)
        {
            failure = exception.Message;
            return false;
        }
    }

    internal bool TryLayoutHostedWindow(
        HostedAppWindow target,
        RectInt32 clientBounds,
        out string failure) =>
        TryLayoutHostedWindowCore(target, clientBounds, refreshFrame: false, out failure);

    private bool TryLayoutHostedWindowCore(
        HostedAppWindow target,
        RectInt32 clientBounds,
        bool refreshFrame,
        out string failure)
    {
        failure = string.Empty;
        ArgumentNullException.ThrowIfNull(target);
        if (clientBounds.Width <= 0 || clientBounds.Height <= 0)
        {
            failure = "宿主视频区域尚未完成布局。";
            return false;
        }

        if (!IsWindow(target.WindowHandle))
        {
            failure = "目标 Android 应用窗口已关闭。";
            return false;
        }

        var ownerWindow = GetWindow(target.WindowHandle, GwOwner);
        var origin = new NativePoint(clientBounds.X, clientBounds.Y);
        if (ownerWindow == 0 || !ClientToScreen(ownerWindow, ref origin))
        {
            failure = "无法把宿主视频区域转换为屏幕坐标。";
            return false;
        }

        // Keep scrcpy as an owned top-level window. WinUI composition is
        // always above foreign child HWNDs, which would otherwise be black.
        var flags = SwpNoActivate;
        if (refreshFrame)
        {
            flags |= SwpFrameChanged | SwpShowWindow;
        }

        var moved = SetWindowPos(
            target.WindowHandle,
            HwndTop,
            origin.X,
            origin.Y,
            clientBounds.Width,
            clientBounds.Height,
            flags);
        if (moved)
        {
            return true;
        }

        failure = "无法根据当前 DPI 和窗口尺寸调整 Android 应用客户区。";
        return false;
    }

    internal bool TrySampleHostedTopEdge(
        HostedAppWindow target,
        nint hostWindowHandle,
        out int[] edgePixels)
    {
        edgePixels = [];
        ArgumentNullException.ThrowIfNull(target);
        if (hostWindowHandle == 0
            || !TrustedAppWindowProcess.IsTrustedProcessId(target.ProcessId)
            || !IsWindow(target.WindowHandle)
            || GetAncestor(target.WindowHandle, GaRoot) != hostWindowHandle
            || !GetClientRect(target.WindowHandle, out var clientRect))
        {
            return false;
        }

        var width = clientRect.Right - clientRect.Left;
        var height = clientRect.Bottom - clientRect.Top;
        if (width < 32 || height < 16)
        {
            return false;
        }

        var screenContext = GetDC(0);
        if (screenContext == 0)
        {
            return false;
        }

        nint memoryContext = 0;
        nint captureBitmap = 0;
        nint previousObject = 0;
        try
        {
            const int captureHeight = 24;
            memoryContext = CreateCompatibleDC(screenContext);
            captureBitmap = CreateCompatibleBitmap(
                screenContext,
                width,
                Math.Min(height, captureHeight));
            if (memoryContext == 0 || captureBitmap == 0)
            {
                return false;
            }

            previousObject = SelectObject(memoryContext, captureBitmap);
            if (previousObject == 0 || previousObject == -1)
            {
                return false;
            }

            // scrcpy renders with SDL/GPU composition, so desktop GetPixel reads
            // whatever is behind the independent-flip surface. PrintWindow asks
            // DWM/SDL for the trusted child's real client pixels and also works
            // while another desktop window is in front. This is event-triggered,
            // never a continuous capture loop.
            if (!PrintWindow(
                target.WindowHandle,
                memoryContext,
                PwClientOnly | PwRenderFullContent))
            {
                return false;
            }

            const int columnCount = 40;
            var yOffsets = new[]
            {
                Math.Min(4, height - 1),
                Math.Min(10, height - 1),
                Math.Min(18, height - 1),
            };
            var pixels = new List<int>(columnCount * yOffsets.Length);
            var horizontalInset = Math.Max(2, width / 32);
            var sampleWidth = Math.Max(1, width - horizontalInset * 2 - 1);
            foreach (var yOffset in yOffsets)
            {
                for (var column = 0; column < columnCount; column++)
                {
                    var xOffset = horizontalInset
                        + (int)Math.Round(sampleWidth * column / (double)(columnCount - 1));
                    var colorRef = GetPixel(
                        memoryContext,
                        xOffset,
                        yOffset);
                    if (colorRef == ColorRefInvalid)
                    {
                        continue;
                    }

                    var red = (int)(colorRef & 0xFF);
                    var green = (int)((colorRef >> 8) & 0xFF);
                    var blue = (int)((colorRef >> 16) & 0xFF);
                    pixels.Add((red << 16) | (green << 8) | blue);
                }
            }

            edgePixels = [.. pixels];
            return edgePixels.Length >= columnCount;
        }
        finally
        {
            if (previousObject != 0 && previousObject != -1 && memoryContext != 0)
            {
                _ = SelectObject(memoryContext, previousObject);
            }

            if (captureBitmap != 0)
            {
                _ = DeleteObject(captureBitmap);
            }

            if (memoryContext != 0)
            {
                _ = DeleteDC(memoryContext);
            }

            _ = ReleaseDC(0, screenContext);
        }
    }

    internal bool TrySendScrcpyRotationShortcut(
        HostedAppWindow target,
        ScrcpyRotationDirection direction)
    {
        ArgumentNullException.ThrowIfNull(target);
        if (!TrustedAppWindowProcess.IsTrustedProcessId(target.ProcessId)
            || !IsWindow(target.WindowHandle))
        {
            return false;
        }

        var targetThreadId = GetWindowThreadProcessId(target.WindowHandle, out _);
        var currentThreadId = GetCurrentThreadId();
        var attached = targetThreadId != 0
            && targetThreadId != currentThreadId
            && AttachThreadInput(currentThreadId, targetThreadId, true);
        try
        {
            _ = SetForegroundWindow(target.WindowHandle);
            _ = SetFocus(target.WindowHandle);
            var key = direction == ScrcpyRotationDirection.Left ? VkLeft : VkRight;
            Span<Input> inputs = stackalloc Input[]
            {
                Input.KeyDown(VkMenu),
                Input.KeyDown(key),
                Input.KeyUp(key),
                Input.KeyUp(VkMenu),
            };
            return SendInput((uint)inputs.Length, ref inputs[0], Marshal.SizeOf<Input>())
                == (uint)inputs.Length;
        }
        finally
        {
            if (attached)
            {
                _ = AttachThreadInput(currentThreadId, targetThreadId, false);
            }
        }
    }

    internal bool SetHostTopMost(nint hostWindowHandle, bool topMost)
    {
        if (hostWindowHandle == 0)
        {
            return false;
        }

        return SetWindowPos(
            hostWindowHandle,
            topMost ? HwndTopMost : HwndNoTopMost,
            0,
            0,
            0,
            0,
            SwpNoMove | SwpNoSize | SwpNoActivate);
    }

    internal void RequestCloseHostedWindow(HostedAppWindow? target)
    {
        if (target is null || target.WindowHandle == 0)
        {
            return;
        }

        var processId = target.ProcessId;
        var windowHandle = target.WindowHandle;
        _ = PostMessage(windowHandle, WmClose, 0, 0);
        _ = Task.Run(async () =>
        {
            await Task.Delay(2000);
            try
            {
                if (!TrustedAppWindowProcess.IsTrustedProcessId(processId))
                {
                    return;
                }

                using var process = System.Diagnostics.Process.GetProcessById((int)processId);
                if (!process.HasExited)
                {
                    process.Kill(entireProcessTree: true);
                }
            }
            catch (Exception exception) when (
                exception is ArgumentException
                    or InvalidOperationException
                    or System.ComponentModel.Win32Exception
                    or NotSupportedException)
            {
            }
        });
    }

    internal bool TryReleaseHostedWindowToWarmCache(HostedAppWindow? target)
    {
        if (target is null
            || target.WindowHandle == 0
            || !TrustedAppWindowProcess.IsTrustedProcessId(target.ProcessId)
            || !IsWindow(target.WindowHandle))
        {
            return false;
        }

        try
        {
            var style = GetWindowLongPtr(target.WindowHandle, GwlStyle).ToInt64();
            style &= ~WsChild;
            style |= WsPopup
                | WsCaption
                | WsThickFrame
                | WsSysMenu
                | WsMinimizeBox
                | WsMaximizeBox
                | WsVisible
                | WsClipSiblings
                | WsClipChildren;
            _ = SetWindowLongPtr(target.WindowHandle, GwlStyle, new nint(style));

            var exStyle = GetWindowLongPtr(target.WindowHandle, GwlExStyle).ToInt64();
            exStyle &= ~WsExAppWindow;
            exStyle |= WsExToolWindow;
            _ = SetWindowLongPtr(target.WindowHandle, GwlExStyle, new nint(exStyle));

            _ = SetParent(target.WindowHandle, 0);
            _ = SetWindowLongPtr(target.WindowHandle, GwlHwndParent, 0);
            _ = SetWindowPos(
                target.WindowHandle,
                0,
                -32000,
                -32000,
                1,
                1,
                SwpNoZOrder | SwpNoActivate | SwpFrameChanged | SwpShowWindow);
            _ = ShowWindowAsync(target.WindowHandle, SwMinimize);
            return true;
        }
        catch (Exception exception) when (
            exception is InvalidOperationException
                or ExternalException
                or ArithmeticException)
        {
            return false;
        }
    }

    internal bool TryGetForegroundTarget(out AppWindowMetadata metadata)
    {
        metadata = null!;
        try
        {
            var windowHandle = GetForegroundWindow();
            if (windowHandle == 0 || !IsWindowVisible(windowHandle))
            {
                return false;
            }

            lock (_hostFocusGate)
            {
                if (windowHandle == _hostFocusWindow
                    && _hostFocusTarget is not null
                    && TrustedAppWindowProcess.IsTrustedProcessId(_hostFocusTarget.WindowProcessId))
                {
                    metadata = _hostFocusTarget;
                    return true;
                }
            }

            GetWindowThreadProcessId(windowHandle, out var processId);
            if (processId == 0)
            {
                return false;
            }

            var now = Environment.TickCount64;
            lock (_foregroundCacheGate)
            {
                if (windowHandle == _cachedForegroundWindow
                    && processId == _cachedForegroundProcessId
                    && now < _foregroundCacheExpiresAt)
                {
                    metadata = _cachedForegroundTarget!;
                    return metadata is not null;
                }

                AppWindowMetadata? target = null;
                if (TrustedAppWindowProcess.IsTrustedProcessId(processId)
                    && AppWindowMetadataStore.TryRead(processId, out var parsedTarget))
                {
                    target = parsedTarget;
                }

                _cachedForegroundWindow = windowHandle;
                _cachedForegroundProcessId = processId;
                _cachedForegroundTarget = target;
                _foregroundCacheExpiresAt = now + ForegroundCacheMilliseconds;
                metadata = target!;
                return target is not null;
            }
        }
        catch
        {
            metadata = null!;
            return false;
        }
    }

    internal bool IsHostedWindowAttachedTo(HostedAppWindow? target, nint hostWindowHandle)
    {
        return target is not null
            && target.WindowHandle != 0
            && hostWindowHandle != 0
            && IsWindow(target.WindowHandle)
            && GetWindow(target.WindowHandle, GwOwner) == hostWindowHandle;
    }

    internal void RegisterHostFocusTarget(nint hostWindowHandle, AppWindowMetadata metadata)
    {
        ArgumentNullException.ThrowIfNull(metadata);
        lock (_hostFocusGate)
        {
            _hostFocusWindow = hostWindowHandle;
            _hostFocusTarget = metadata;
        }
    }

    internal void ClearHostFocusTarget(nint hostWindowHandle)
    {
        lock (_hostFocusGate)
        {
            if (_hostFocusWindow == hostWindowHandle)
            {
                _hostFocusWindow = 0;
                _hostFocusTarget = null;
            }
        }
    }

    private static bool TryFindWindow(uint processId, out nint windowHandle)
    {
        nint foundWindow = 0;
        EnumWindowsProc callback = (candidate, parameter) =>
        {
            if (!IsWindowVisible(candidate))
            {
                return true;
            }

            GetWindowThreadProcessId(candidate, out var candidateProcessId);
            if (candidateProcessId != processId)
            {
                return true;
            }

            foundWindow = candidate;
            return false;
        };

        _ = EnumWindows(callback, 0);
        windowHandle = foundWindow;
        return foundWindow != 0;
    }


    private static bool TryResolveTrustedWindow(
        AppWindowMetadata metadata,
        out nint windowHandle)
    {
        windowHandle = 0;
        return TrustedAppWindowProcess.IsTrustedExecutablePath(metadata.ScrcpyPath)
            && TrustedAppWindowProcess.IsTrustedProcessId(metadata.WindowProcessId)
            && TryFindWindow(metadata.WindowProcessId, out windowHandle);
    }

    private static void ApplyWindowMaterial(nint windowHandle)
    {
        // scrcpy owns an opaque Android video surface, so backdrop material is
        // applied to the native non-client frame while the real Android pixels
        // remain untouched. Keep Windows' default edge instead of drawing a
        // decorative accent outline around the entire application window.
        var cornerPreference = 2; // DWMWCP_ROUND
        var backdropType = 3; // DWMSBT_TRANSIENTWINDOW (Acrylic-like)
        var darkMode = ShouldUseDarkMode() ? 1 : 0;
        var borderColor = unchecked((int)0xFFFFFFFF); // DWMWA_COLOR_DEFAULT
        _ = DwmSetWindowAttribute(windowHandle, 20, ref darkMode, sizeof(int));
        _ = DwmSetWindowAttribute(windowHandle, 33, ref cornerPreference, sizeof(int));
        _ = DwmSetWindowAttribute(windowHandle, 34, ref borderColor, sizeof(int));
        _ = DwmSetWindowAttribute(windowHandle, 38, ref backdropType, sizeof(int));
    }

    private static bool ShouldUseDarkMode()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(
                @"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            return key?.GetValue("AppsUseLightTheme") is int value && value == 0;
        }
        catch
        {
            return false;
        }
    }

    private static int? TryGetPositiveInt32(
        JsonElement? payload,
        string propertyName)
    {
        if (payload is not JsonElement element
            || element.ValueKind != JsonValueKind.Object)
        {
            return null;
        }

        if (element.TryGetProperty("launch", out var launch))
        {
            element = launch;
        }

        if (element.ValueKind != JsonValueKind.Object
            || !element.TryGetProperty(propertyName, out var property)
            || property.ValueKind != JsonValueKind.Number
            || !property.TryGetInt32(out var value)
            || value <= 0)
        {
            return null;
        }

        return value;
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate bool EnumWindowsProc(nint windowHandle, nint parameter);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumWindows(
        EnumWindowsProc callback,
        nint parameter);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsWindow(nint windowHandle);

    [DllImport("user32.dll")]
    private static extern nint GetAncestor(nint windowHandle, int flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetClientRect(nint windowHandle, out NativeRect clientRect);

    [DllImport("user32.dll")]
    private static extern nint GetDC(nint windowHandle);

    [DllImport("user32.dll")]
    private static extern int ReleaseDC(nint windowHandle, nint deviceContext);

    [DllImport("gdi32.dll")]
    private static extern uint GetPixel(nint deviceContext, int x, int y);

    [DllImport("gdi32.dll")]
    private static extern nint CreateCompatibleDC(nint deviceContext);

    [DllImport("gdi32.dll")]
    private static extern nint CreateCompatibleBitmap(
        nint deviceContext,
        int width,
        int height);

    [DllImport("gdi32.dll")]
    private static extern nint SelectObject(nint deviceContext, nint graphicsObject);

    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteObject(nint graphicsObject);

    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteDC(nint deviceContext);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PrintWindow(
        nint windowHandle,
        nint deviceContext,
        uint flags);

    [DllImport("user32.dll")]
    private static extern nint GetForegroundWindow();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsWindowVisible(nint windowHandle);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsIconic(nint windowHandle);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShowWindowAsync(nint windowHandle, int command);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool BringWindowToTop(nint windowHandle);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetForegroundWindow(nint windowHandle);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(
        nint windowHandle,
        out uint processId);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern nint SetParent(nint childWindow, nint newParentWindow);

    [DllImport("user32.dll")]
    private static extern nint GetParent(nint windowHandle);

    [DllImport("user32.dll")]
    private static extern nint GetWindow(nint windowHandle, uint command);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ClientToScreen(nint windowHandle, ref NativePoint point);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static extern nint GetWindowLongPtr64(nint windowHandle, int index);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongW", SetLastError = true)]
    private static extern int GetWindowLong32(nint windowHandle, int index);

    private static nint GetWindowLongPtr(nint windowHandle, int index) =>
        nint.Size == 8
            ? GetWindowLongPtr64(windowHandle, index)
            : GetWindowLong32(windowHandle, index);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern nint SetWindowLongPtr64(nint windowHandle, int index, nint value);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongW", SetLastError = true)]
    private static extern int SetWindowLong32(nint windowHandle, int index, int value);

    private static nint SetWindowLongPtr(nint windowHandle, int index, nint value) =>
        nint.Size == 8
            ? SetWindowLongPtr64(windowHandle, index, value)
            : new nint(SetWindowLong32(windowHandle, index, value.ToInt32()));

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(
        nint windowHandle,
        nint insertAfter,
        int x,
        int y,
        int width,
        int height,
        uint flags);

    [DllImport("user32.dll")]
    private static extern nint SetFocus(nint windowHandle);

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint SendInput(uint inputCount, ref Input inputs, int inputSize);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostMessage(
        nint windowHandle,
        int message,
        nint wParam,
        nint lParam);

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(
        nint windowHandle,
        int attribute,
        ref int value,
        int valueSize);

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Input
    {
        public uint Type;
        public InputUnion Union;

        public static Input KeyDown(ushort key) => new()
        {
            Type = InputKeyboard,
            Union = new InputUnion
            {
                Keyboard = new KeyboardInput { VirtualKey = key },
            },
        };

        public static Input KeyUp(ushort key) => new()
        {
            Type = InputKeyboard,
            Union = new InputUnion
            {
                Keyboard = new KeyboardInput
                {
                    VirtualKey = key,
                    Flags = KeyEventFKeyUp,
                },
            },
        };
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct InputUnion
    {
        [FieldOffset(0)]
        public KeyboardInput Keyboard;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct KeyboardInput
    {
        public ushort VirtualKey;
        public ushort Scan;
        public uint Flags;
        public uint Time;
        public nint ExtraInfo;
    }
}

internal sealed record HostedAppWindow(
    uint ProcessId,
    nint WindowHandle,
    uint DisplayId,
    string PackageName,
    string Title);

internal enum ScrcpyRotationDirection
{
    Left,
    Right,
}
