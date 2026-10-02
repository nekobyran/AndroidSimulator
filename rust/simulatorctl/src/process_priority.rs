use crate::runtime_settings::PerformanceMode;
use androidsimulator_core::PerformanceMode as CorePerformanceMode;
use anyhow::Result;

fn core_mode(mode: PerformanceMode) -> CorePerformanceMode {
    match mode {
        PerformanceMode::Eco => CorePerformanceMode::Eco,
        PerformanceMode::Balanced => CorePerformanceMode::Balanced,
        PerformanceMode::Performance => CorePerformanceMode::Performance,
        PerformanceMode::Custom => CorePerformanceMode::Custom,
    }
}

pub fn promote_latency_sensitive_process(pid: u32, mode: PerformanceMode) -> Result<()> {
    androidsimulator_core::promote_latency_sensitive_process(pid, core_mode(mode))
}

pub fn demote_idle_process(pid: u32) -> Result<()> {
    androidsimulator_core::demote_idle_process(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_modes_map_exactly_to_native_core_modes() {
        assert_eq!(core_mode(PerformanceMode::Eco), CorePerformanceMode::Eco);
        assert_eq!(
            core_mode(PerformanceMode::Balanced),
            CorePerformanceMode::Balanced
        );
        assert_eq!(
            core_mode(PerformanceMode::Performance),
            CorePerformanceMode::Performance
        );
        assert_eq!(
            core_mode(PerformanceMode::Custom),
            CorePerformanceMode::Custom
        );
    }
}
