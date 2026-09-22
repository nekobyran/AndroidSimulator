use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SETTINGS_SCHEMA_VERSION: u8 = 1;
const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PerformanceMode {
    Eco,
    Balanced,
    Performance,
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum RendererMode {
    Direct3d,
    Opengl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsStrategy {
    Efficiency,
    Balanced,
    Quality,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionMode {
    Adaptive,
    P720,
    P1080,
    P1440,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SimulatorSettings {
    pub schema_version: u8,
    pub performance_mode: PerformanceMode,
    pub cpu_cores: u8,
    pub memory_mb: u32,
    pub renderer_mode: RendererMode,
    pub graphics_strategy: GraphicsStrategy,
    pub resolution_mode: ResolutionMode,
    pub custom_width: u32,
    pub custom_height: u32,
    pub custom_dpi: u32,
    pub max_frame_rate: u32,
    pub dynamic_frame_rate: bool,
    pub dynamic_low_frame_rate: u32,
    pub vertical_sync: bool,
    pub super_resolution: bool,
    pub super_resolution_scale_percent: u32,
    pub frame_interpolation: bool,
    pub system_audio: bool,
    pub keep_app_alive: bool,
    pub remember_window_position: bool,
    pub fixed_window_size: bool,
    pub auto_rotate: bool,
    pub quit_confirm: bool,
}

impl Default for SimulatorSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            performance_mode: PerformanceMode::Balanced,
            cpu_cores: 4,
            memory_mb: 6144,
            renderer_mode: RendererMode::Direct3d,
            graphics_strategy: GraphicsStrategy::Balanced,
            resolution_mode: ResolutionMode::Adaptive,
            custom_width: 1920,
            custom_height: 1080,
            custom_dpi: 280,
            max_frame_rate: 60,
            dynamic_frame_rate: true,
            dynamic_low_frame_rate: 15,
            vertical_sync: true,
            super_resolution: false,
            super_resolution_scale_percent: 125,
            frame_interpolation: false,
            system_audio: false,
            keep_app_alive: false,
            remember_window_position: true,
            fixed_window_size: false,
            auto_rotate: true,
            quit_confirm: false,
        }
    }
}

impl SimulatorSettings {
    pub fn effective_cpu_cores(&self) -> u8 {
        match self.performance_mode {
            PerformanceMode::Eco => 2,
            PerformanceMode::Balanced => 4,
            PerformanceMode::Performance => 6,
            PerformanceMode::Custom => self.cpu_cores.clamp(2, 12),
        }
    }

    pub fn effective_memory_mb(&self) -> u32 {
        match self.performance_mode {
            PerformanceMode::Eco => 3072,
            PerformanceMode::Balanced => 6144,
            PerformanceMode::Performance => 8192,
            PerformanceMode::Custom => self.memory_mb.clamp(3072, 16384),
        }
    }

    pub fn capture_fps(&self) -> u32 {
        self.max_frame_rate.clamp(30, 60)
    }

    pub fn presentation_fps(&self) -> u32 {
        self.max_frame_rate.clamp(30, 240)
    }

    pub fn effective_video_bit_rate(&self) -> &'static str {
        match self.graphics_strategy {
            GraphicsStrategy::Efficiency => "4M",
            GraphicsStrategy::Balanced => "8M",
            GraphicsStrategy::Quality => "16M",
        }
    }

    pub fn effective_max_size(&self) -> u32 {
        let base = match self.graphics_strategy {
            GraphicsStrategy::Efficiency => 1280,
            GraphicsStrategy::Balanced => 1920,
            GraphicsStrategy::Quality => 2560,
        };
        if self.super_resolution {
            ((base * self.super_resolution_scale_percent.clamp(100, 200)) / 100).clamp(1280, 3840)
        } else {
            base
        }
    }

    pub fn initial_display_profile(&self, windows_dpi: u32) -> (u32, u32, u32) {
        let windows_dpi = windows_dpi.clamp(72, 384);
        let (width, height, dpi) = match self.resolution_mode {
            ResolutionMode::Adaptive => {
                let scale = |value: u32| (value * windows_dpi + 48) / 96;
                (
                    scale(1280).clamp(960, 3840),
                    scale(720).clamp(540, 2160),
                    scale(160).clamp(120, 640),
                )
            }
            ResolutionMode::P720 => (1280, 720, 240),
            ResolutionMode::P1080 => (1920, 1080, 280),
            ResolutionMode::P1440 => (2560, 1440, 360),
            ResolutionMode::Custom => (
                self.custom_width.clamp(640, 3840),
                self.custom_height.clamp(360, 2160),
                self.custom_dpi.clamp(120, 640),
            ),
        };

        if !self.super_resolution {
            return (width, height, dpi);
        }

        // Supersampling must scale density together with pixel dimensions so
        // Android keeps the same dp layout while rendering more source pixels.
        let requested_percent = self.super_resolution_scale_percent.clamp(100, 200);
        let dimension_limit_percent = 3840 * 100 / width.max(height);
        let density_limit_percent = 640 * 100 / dpi;
        let effective_percent = requested_percent
            .min(dimension_limit_percent)
            .min(density_limit_percent)
            .max(100);
        let scale = |value: u32| (value * effective_percent + 50) / 100;
        let even = |value: u32| value.max(2) & !1;
        (
            even(scale(width)).clamp(640, 3840),
            even(scale(height)).clamp(360, 3840),
            scale(dpi).clamp(120, 640),
        )
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            bail!(
                "unsupported simulator settings schema version {}; expected {}",
                self.schema_version,
                SETTINGS_SCHEMA_VERSION
            );
        }
        if !(2..=12).contains(&self.cpu_cores) {
            bail!("custom CPU cores must be between 2 and 12");
        }
        if !(3072..=16384).contains(&self.memory_mb) {
            bail!("custom memory must be between 3072 and 16384 MB");
        }
        if !(640..=3840).contains(&self.custom_width) || !(360..=2160).contains(&self.custom_height)
        {
            bail!("custom resolution must be within 640x360 and 3840x2160");
        }
        if !(120..=640).contains(&self.custom_dpi) {
            bail!("custom DPI must be between 120 and 640");
        }
        if !(30..=240).contains(&self.max_frame_rate) {
            bail!("maximum frame rate must be between 30 and 240");
        }
        if !(10..=60).contains(&self.dynamic_low_frame_rate) {
            bail!("background frame rate must be between 10 and 60");
        }
        if !(100..=200).contains(&self.super_resolution_scale_percent) {
            bail!("super-resolution scale must be between 100 and 200 percent");
        }
        Ok(())
    }
}

pub fn settings_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join(SETTINGS_FILE_NAME)
}

pub fn load(runtime_root: &Path) -> Result<SimulatorSettings> {
    let path = settings_path(runtime_root);
    if !path.is_file() {
        return Ok(SimulatorSettings::default());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read simulator settings {}", path.display()))?;
    let settings: SimulatorSettings = serde_json::from_str(&text)
        .with_context(|| format!("invalid simulator settings {}", path.display()))?;
    settings.validate()?;
    Ok(settings)
}

pub fn save(runtime_root: &Path, settings: &SimulatorSettings) -> Result<PathBuf> {
    settings.validate()?;
    fs::create_dir_all(runtime_root)?;
    let path = settings_path(runtime_root);
    let text = serde_json::to_string_pretty(settings)?;
    fs::write(&path, text.as_bytes())
        .with_context(|| format!("failed to write simulator settings {}", path.display()))?;
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_defaults_match_desktop_runtime_targets() {
        let settings = SimulatorSettings::default();
        assert_eq!(settings.effective_cpu_cores(), 4);
        assert_eq!(settings.effective_memory_mb(), 6144);
        assert_eq!(settings.capture_fps(), 60);
        assert_eq!(settings.presentation_fps(), 60);
        assert!(!settings.super_resolution);
        settings.validate().unwrap();
    }

    #[test]
    fn performance_preset_raises_resources_and_quality() {
        let settings = SimulatorSettings {
            performance_mode: PerformanceMode::Performance,
            graphics_strategy: GraphicsStrategy::Quality,
            max_frame_rate: 120,
            frame_interpolation: true,
            ..SimulatorSettings::default()
        };
        assert_eq!(settings.effective_cpu_cores(), 6);
        assert_eq!(settings.effective_memory_mb(), 8192);
        assert_eq!(settings.capture_fps(), 60);
        assert_eq!(settings.presentation_fps(), 120);
        assert_eq!(settings.effective_video_bit_rate(), "16M");
    }

    #[test]
    fn super_resolution_preserves_android_dp_layout_by_scaling_pixels_and_density() {
        let settings = SimulatorSettings {
            resolution_mode: ResolutionMode::P720,
            super_resolution: true,
            super_resolution_scale_percent: 150,
            ..SimulatorSettings::default()
        };
        assert_eq!(settings.initial_display_profile(96), (1920, 1080, 360));
    }

    #[test]
    fn super_resolution_increases_encoder_ceiling() {
        let settings = SimulatorSettings {
            graphics_strategy: GraphicsStrategy::Balanced,
            super_resolution: true,
            super_resolution_scale_percent: 150,
            ..SimulatorSettings::default()
        };
        assert_eq!(settings.effective_max_size(), 2880);
    }
}
