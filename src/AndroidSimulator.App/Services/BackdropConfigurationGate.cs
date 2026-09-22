namespace AndroidSimulator.App.Services;

internal sealed class BackdropConfigurationGate
{
    private int _active;

    public bool TryEnter() => Interlocked.CompareExchange(ref _active, 1, 0) == 0;

    public void Exit() => Volatile.Write(ref _active, 0);
}
