namespace AndroidSimulator.App.Models;

public enum KeyBindingActionType
{
    Tap,
    KeyEvent,
}

public sealed record KeyBinding
{
    public string Id { get; init; } = string.Empty;

    public string Name { get; init; } = string.Empty;

    /// <summary>
    /// Windows virtual-key code received by the low-level keyboard hook.
    /// </summary>
    public int HostKey { get; init; }

    public KeyBindingActionType ActionType { get; init; }

    public int TapX { get; init; }

    public int TapY { get; init; }

    public string AndroidKeyCode { get; init; } = string.Empty;

    public bool IsEnabled { get; init; } = true;

    public static KeyBinding CreateTap(string name, int hostKey, int x, int y) =>
        new()
        {
            Id = Guid.NewGuid().ToString("N"),
            Name = name,
            HostKey = hostKey,
            ActionType = KeyBindingActionType.Tap,
            TapX = x,
            TapY = y,
        };

    public static KeyBinding CreateKeyEvent(string name, int hostKey, string androidKeyCode) =>
        new()
        {
            Id = Guid.NewGuid().ToString("N"),
            Name = name,
            HostKey = hostKey,
            ActionType = KeyBindingActionType.KeyEvent,
            AndroidKeyCode = androidKeyCode,
        };
}
