use anyhow::Result;

const PRIORITY_ABOVE_NORMAL: u32 = 0x0000_8000;
const PRIORITY_BELOW_NORMAL: u32 = 0x0000_4000;
const PRIORITY_NORMAL: u32 = 0x0000_0020;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PerformanceMode {
    Eco = 0,
    Balanced = 1,
    Performance = 2,
    Custom = 3,
}

impl TryFrom<u32> for PerformanceMode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Eco),
            1 => Ok(Self::Balanced),
            2 => Ok(Self::Performance),
            3 => Ok(Self::Custom),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SchedulerActivity {
    Foreground = 0,
    Background = 1,
    Idle = 2,
}

impl TryFrom<u32> for SchedulerActivity {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Foreground),
            1 => Ok(Self::Background),
            2 => Ok(Self::Idle),
            _ => Err(()),
        }
    }
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
        SchedulerActivity::Background => PerformancePolicy {
            priority_class: if mode == PerformanceMode::Performance {
                PRIORITY_NORMAL
            } else {
                PRIORITY_BELOW_NORMAL
            },
            eco_qos: true,
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

pub fn promote_latency_sensitive_process(pid: u32, mode: PerformanceMode) -> Result<()> {
    set_process_performance_state(pid, performance_policy(mode, SchedulerActivity::Foreground))
}

pub fn demote_idle_process(pid: u32) -> Result<()> {
    set_process_performance_state(
        pid,
        performance_policy(PerformanceMode::Balanced, SchedulerActivity::Idle),
    )
}

pub fn set_process_activity(
    pid: u32,
    mode: PerformanceMode,
    activity: SchedulerActivity,
) -> Result<()> {
    set_process_performance_state(pid, performance_policy(mode, activity))
}

#[cfg(windows)]
fn set_process_performance_state(pid: u32, policy: PerformancePolicy) -> Result<()> {
    use anyhow::{Context, bail};
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

    if pid == 0 {
        bail!("process id must not be zero");
    }

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
fn set_process_performance_state(_pid: u32, _policy: PerformancePolicy) -> Result<()> {
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "system" fn AndroidSimulatorSetCurrentProcessActivity(mode: u32, activity: u32) -> i32 {
    let Ok(mode) = PerformanceMode::try_from(mode) else {
        return 87;
    };
    let Ok(activity) = SchedulerActivity::try_from(activity) else {
        return 87;
    };

    match set_process_activity(std::process::id(), mode, activity) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_policy_matches_runtime_profiles() {
        assert_eq!(
            performance_policy(PerformanceMode::Performance, SchedulerActivity::Foreground),
            PerformancePolicy {
                priority_class: PRIORITY_ABOVE_NORMAL,
                eco_qos: false,
            }
        );
        assert_eq!(
            performance_policy(PerformanceMode::Balanced, SchedulerActivity::Foreground),
            PerformancePolicy {
                priority_class: PRIORITY_NORMAL,
                eco_qos: false,
            }
        );
        assert_eq!(
            performance_policy(PerformanceMode::Eco, SchedulerActivity::Foreground),
            PerformancePolicy {
                priority_class: PRIORITY_BELOW_NORMAL,
                eco_qos: true,
            }
        );
    }

    #[test]
    fn background_and_idle_policy_enable_ecoqos() {
        assert_eq!(
            performance_policy(PerformanceMode::Performance, SchedulerActivity::Background),
            PerformancePolicy {
                priority_class: PRIORITY_NORMAL,
                eco_qos: true,
            }
        );
        assert_eq!(
            performance_policy(PerformanceMode::Balanced, SchedulerActivity::Background),
            PerformancePolicy {
                priority_class: PRIORITY_BELOW_NORMAL,
                eco_qos: true,
            }
        );
        assert_eq!(
            performance_policy(PerformanceMode::Performance, SchedulerActivity::Idle),
            PerformancePolicy {
                priority_class: PRIORITY_BELOW_NORMAL,
                eco_qos: true,
            }
        );
    }

    #[test]
    fn ffi_contract_rejects_unknown_values() {
        assert_eq!(AndroidSimulatorSetCurrentProcessActivity(99, 0), 87);
        assert_eq!(AndroidSimulatorSetCurrentProcessActivity(1, 99), 87);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::sync::Mutex;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, GetPriorityClass, NORMAL_PRIORITY_CLASS, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };

    static PRIORITY_TEST_LOCK: Mutex<()> = Mutex::new(());

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
    fn current_process_native_policy_roundtrips() {
        let _guard = PRIORITY_TEST_LOCK.lock().unwrap();
        let pid = std::process::id();

        promote_latency_sensitive_process(pid, PerformanceMode::Performance).unwrap();
        assert_eq!(current_priority_class(), ABOVE_NORMAL_PRIORITY_CLASS);

        promote_latency_sensitive_process(pid, PerformanceMode::Balanced).unwrap();
        assert_eq!(current_priority_class(), NORMAL_PRIORITY_CLASS);
    }
}
