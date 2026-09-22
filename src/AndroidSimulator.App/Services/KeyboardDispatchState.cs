namespace AndroidSimulator.App.Services;

internal sealed class KeyboardDispatchState
{
    private readonly HashSet<int> _dispatchedKeys = [];
    private readonly HashSet<int> _passthroughKeys = [];

    public bool ShouldSwallowKeyDown(
        int hostKey,
        bool canDispatch,
        Func<bool> tryQueue)
    {
        ArgumentNullException.ThrowIfNull(tryQueue);
        if (_passthroughKeys.Contains(hostKey))
        {
            return false;
        }

        if (_dispatchedKeys.Contains(hostKey))
        {
            return true;
        }

        if (!canDispatch || !tryQueue())
        {
            // Once the first key-down is passed through, keep every repeat and the
            // matching key-up native. This prevents a half-native/half-mapped press.
            _passthroughKeys.Add(hostKey);
            return false;
        }

        _dispatchedKeys.Add(hostKey);
        return true;
    }

    public bool ShouldSwallowKeyUp(int hostKey)
    {
        _passthroughKeys.Remove(hostKey);
        return _dispatchedKeys.Remove(hostKey);
    }

    public void Clear()
    {
        _dispatchedKeys.Clear();
        _passthroughKeys.Clear();
    }
}
