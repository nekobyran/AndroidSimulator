use crate::runtime_settings::PerformanceMode;
use anyhow::Result;

const PRIORITY_ABOVE_NORMAL: u32 = 0x0000_8000;
const PRIORITY_BELOW_NORMAL: u32 = 0x0000_4000;
const PRIORITY_NORMAL: u32 = 0x0000_0020;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerActivity {
    Foreground,
    Background,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PerformancePolicy {
    priority_class: u32,
    eco_qos: bool,
}

fn performance_policy(mode: PerformanceMode, activity: SchedulerActivity) -> PerformancePolicy {
    match activity {
        SchedulerActivity::Idle => PerformancePolicy {
            priority_class: PRIORITY_BELOW_NORMAL,
            eco_qos: true,
        },
        SchedulerActivity::Background => match mode {
            PerformanceMode::Performance => PerformancePolicy {
                priority_class: PRIORITY_NORMAL,
                eco_qos: true,
            },
            PerformanceMode::Eco | PerformanceMode::Balanced | PerformanceMode::Custom => {
                PerformancePolicy {
                    priority_class: PRIORITY_BELOW_NORMAL,
                    eco_qos: true,
                }
            }
        },
        SchedulerActivity::Foreground => match mode {
            PerformanceMode::Eco => PerformancePolicy {
                priority_class: PRIORITY_BELOW_NORMAL,
                eco_qos: true,
            },
            PerformanceMode::Balanced | PerformanceMode::Custom => PerformancePolicy {
                priority_class: PRIORITY_NORMAL,
                eco_qos: false,
            },
            PerformanceMode::Performance => PerformancePolicy {
                priority_class: PRIORITY_ABOVE_NORMAL,
                eco_qos: false,
            },
        },
    }
}

#[cfg(windows)]
pub fn promote_latency_sensitive_process(pid: u32, mode: PerformanceMode) -> Result<()> {
    set_process_performance_state(pid, performance_policy(mode, SchedulerActivity::Foreground))
}

#[cfg(windows)]
pub fn demote_background_process(pid: u32, mode: PerformanceMode) -> Result<()> {
    set_process_performance_state(pid, performance_policy(mode, SchedulerActivity::Background))
}

#[cfg(windows)]
pub fn demote_idle_process(pid: u32) -> Result<()> {
    set_process_performance_state(
        pid,
        performance_policy(PerformanceMode::Balanced, SchedulerActivity::Idle),
    )
}

#[cfg(windows)]
fn set_process_performance_state(pid: u32, policy: PerformancePolicy) -> Result<()> {
    use anyhow::Context;
    use std::{
        collections::HashMap,
        ffi::c_void,
        mem::size_of,
        sync::{Mutex, OnceLock},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::Threading::{
            GetPriorityClass, OpenProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, ProcessPowerThrottling,
            SetPriorityClass, SetProcessInformation,
        },
    };

    static APPLIED: OnceLock<Mutex<HashMap<u32, PerformancePolicy>>> = OnceLock::new();
    let applied = APPLIED.get_or_init(|| Mutex::new(HashMap::new()));
    if applied.lock().unwrap().get(&pid).copied() == Some(policy) {
        return Ok(());
    }

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

    let current_priority = unsafe { GetPriorityClass(handle.0) };
    if current_priority == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read process priority for process {pid}"));
    }
    if current_priority != policy.priority_class
        && unsafe { SetPriorityClass(handle.0, policy.priority_class) } == 0
    {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to set process priority for process {pid}"));
    }

    let power_state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if policy.eco_qos {
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

    applied.lock().unwrap().insert(pid, policy);
    Ok(())
}

#[cfg(not(windows))]
pub fn promote_latency_sensitive_process(_pid: u32, _mode: PerformanceMode) -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn demote_background_process(_pid: u32, _mode: PerformanceMode) -> Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn demote_idle_process(_pid: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn scheduler_policy_distinguishes_foreground_background_and_idle() {
        assert_eq!(
            performance_policy(PerformanceMode::Performance, SchedulerActivity::Foreground),
            PerformancePolicy {
                priority_class: PRIORITY_ABOVE_NORMAL,
                eco_qos: false,
            }
        );
        assert_eq!(
            performance_policy(PerformanceMode::Performance, SchedulerActivity::Background),
            PerformancePolicy {
                priority_class: PRIORITY_NORMAL,
                eco_qos: true,
            }
        );
        assert_eq!(
            performance_policy(PerformanceMode::Balanced, SchedulerActivity::Idle),
            PerformancePolicy {
                priority_class: PRIORITY_BELOW_NORMAL,
                eco_qos: true,
            }
        );
    }

    #[test]
    fn eco_mode_uses_ecoqos_even_while_foreground() {
        assert_eq!(
            performance_policy(PerformanceMode::Eco, SchedulerActivity::Foreground),
            PerformancePolicy {
                priority_class: PRIORITY_BELOW_NORMAL,
                eco_qos: true,
            }
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::sync::Mutex;

    static PRIORITY_TEST_LOCK: Mutex<()> = Mutex::new();
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
