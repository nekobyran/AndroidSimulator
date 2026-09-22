use crate::runtime_settings::PerformanceMode;
use anyhow::Result;

#[cfg(windows)]
pub fn promote_latency_sensitive_process(pid: u32, mode: PerformanceMode) -> Result<()> {
    set_process_performance_state(pid, PerformanceState::Active(mode))
}

#[cfg(windows)]
pub fn demote_idle_process(pid: u32) -> Result<()> {
    set_process_performance_state(pid, PerformanceState::Idle)
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PerformanceState {
    Active(PerformanceMode),
    Idle,
}

#[cfg(windows)]
fn set_process_performance_state(pid: u32, state: PerformanceState) -> Result<()> {
    use anyhow::Context;
    use std::{ffi::c_void, mem::size_of};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
            OpenProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, ProcessPowerThrottling,
            SetPriorityClass, SetProcessInformation,
        },
    };

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let handle = unsafe {
        OpenProcess(
            PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to open managed performance process {pid}"));
    }
    let handle = OwnedHandle(handle);

    let priority_class = match state {
        PerformanceState::Active(PerformanceMode::Eco) => BELOW_NORMAL_PRIORITY_CLASS,
        PerformanceState::Active(PerformanceMode::Balanced | PerformanceMode::Custom) => {
            NORMAL_PRIORITY_CLASS
        }
        PerformanceState::Active(PerformanceMode::Performance) => ABOVE_NORMAL_PRIORITY_CLASS,
        // Idle priority made the guest ADB transport stall for tens of seconds.
        // BelowNormal plus EcoQoS reduces standby cost while preserving enough
        // scheduling time for a shortcut launch to wake the guest promptly.
        PerformanceState::Idle => BELOW_NORMAL_PRIORITY_CLASS,
    };
    if unsafe { SetPriorityClass(handle.0, priority_class) } == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to set process priority for process {pid}"));
    }

    let execution_speed_throttled = matches!(state, PerformanceState::Idle);
    let power_state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if execution_speed_throttled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    if unsafe {
        SetProcessInformation(
            handle.0,
            ProcessPowerThrottling,
            (&raw const power_state).cast::<c_void>(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!("failed to update execution-speed throttling for process {pid}")
        });
    }

    Ok(())
}

#[cfg(not(windows))]
pub fn promote_latency_sensitive_process(_pid: u32, _mode: PerformanceMode) -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn demote_idle_process(_pid: u32) -> Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static PRIORITY_TEST_LOCK: Mutex<()> = Mutex::new(());
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, GetPriorityClass, NORMAL_PRIORITY_CLASS, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };

    fn current_priority_class() -> u32 {
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, std::process::id()) };
        assert!(!handle.is_null());
        let priority = unsafe { GetPriorityClass(handle) };
        unsafe {
            CloseHandle(handle);
        }
        priority
    }

    #[test]
    fn balanced_mode_restores_normal_latency_priority() {
        let _guard = PRIORITY_TEST_LOCK.lock().unwrap();
        let pid = std::process::id();
        demote_idle_process(pid).unwrap();
        promote_latency_sensitive_process(pid, PerformanceMode::Balanced).unwrap();
        assert_eq!(current_priority_class(), NORMAL_PRIORITY_CLASS);
    }

    #[test]
    fn performance_mode_uses_above_normal_without_high_priority_class() {
        let _guard = PRIORITY_TEST_LOCK.lock().unwrap();
        let pid = std::process::id();
        promote_latency_sensitive_process(pid, PerformanceMode::Performance).unwrap();
        assert_eq!(current_priority_class(), ABOVE_NORMAL_PRIORITY_CLASS);
        promote_latency_sensitive_process(pid, PerformanceMode::Balanced).unwrap();
    }
}
