using System.Text.Json;
using System.Text.Json.Serialization;

namespace AndroidSimulator.App.Services;

internal sealed record SimulatorRuntimeSettings
{
    internal const string RuntimeRoot = @"D:\vibecoding\sdk\android-simulator-runtime";
    internal static readonly string SettingsPath = Path.Combine(RuntimeRoot, "settings.json");

    [JsonPropertyName("schema_version")]
    public byte SchemaVersion { get; init; } = 1;

    [JsonPropertyName("performance_mode")]
    public string PerformanceMode { get; init; } = "balanced";

    [JsonPropertyName("cpu_cores")]
    public byte CpuCores { get; init; } = 4;

    [JsonPropertyName("memory_mb")]
    public uint MemoryMb { get; init; } = 6144;

    [JsonPropertyName("renderer_mode")]
    public string RendererMode { get; init; } = "direct3d";

    [JsonPropertyName("graphics_strategy")]
    public string GraphicsStrategy { get; init; } = "balanced";

    [JsonPropertyName("resolution_mode")]
    public string ResolutionMode { get; init; } = "adaptive";

    [JsonPropertyName("custom_width")]
    public uint CustomWidth { get; init; } = 1920;

    [JsonPropertyName("custom_height")]
    public uint CustomHeight { get; init; } = 1080;

    [JsonPropertyName("custom_dpi")]
    public uint CustomDpi { get; init; } = 280;

    [JsonPropertyName("max_frame_rate")]
    public uint MaxFrameRate { get; init; } = 60;

    [JsonPropertyName("dynamic_frame_rate")]
    public bool DynamicFrameRate { get; init; } = true;

    [JsonPropertyName("dynamic_low_frame_rate")]
    public uint DynamicLowFrameRate { get; init; } = 15;

    [JsonPropertyName("vertical_sync")]
    public bool VerticalSync { get; init; } = true;

    [JsonPropertyName("super_resolution")]
        public bool SuperResolution { get; init; }

    [JsonPropertyName("super_resolution_scale_percent")]
    public uint SuperResolutionScalePercent { get; init; } = 125;

    [JsonPropertyName("frame_interpolation")]
    public bool FrameInterpolation { get; init; }

    [JsonPropertyName("system_audio")]
    public bool SystemAudio { get; init; }

    [JsonPropertyName("keep_app_alive")]
    public bool KeepAppAlive { get; init; }

    [JsonPropertyName("remember_window_position")]
    public bool RememberWindowPosition { get; init; } = true;

    [JsonPropertyName("fixed_window_size")]
    public bool FixedWindowSize { get; init; }

    [JsonPropertyName("auto_rotate")]
    public bool AutoRotate { get; init; } = true;

    [JsonPropertyName("quit_confirm")]
    public bool QuitConfirm { get; init; }

    public static SimulatorRuntimeSettings Defaults { get; } = new();

    public static SimulatorRuntimeSettings LoadFromDisk()
    {
        if (!File.Exists(SettingsPath))
        {
            return Defaults;
        }

        var text = File.ReadAllText(SettingsPath);
        return JsonSerializer.Deserialize<SimulatorRuntimeSettings>(text) ?? Defaults;
    }

    public static SimulatorRuntimeSettings FromJson(JsonElement element)
        => JsonSerializer.Deserialize<SimulatorRuntimeSettings>(element.GetRawText())
            ?? throw new JsonException("simulatorctl returned an empty settings payload.");

    public uint EffectiveCpuCores => PerformanceMode switch
    {
        "eco" => 2,
        "performance" => 6,
        "custom" => Math.Clamp((uint)CpuCores, 2u, 12u),
        _ => 4,
    };

    public uint EffectiveMemoryMb => PerformanceMode switch
    {
        "eco" => 3072,
        "performance" => 8192,
        "custom" => Math.Clamp(MemoryMb, 3072u, 16384u),
        _ => 6144,
    };

    public uint CaptureFrameRate => Math.Clamp(MaxFrameRate, 30u, 60u);

    public uint PresentationFrameRate
        => FrameInterpolation ? Math.Clamp(MaxFrameRate, 30u, 240u) : CaptureFrameRate;
}
