use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

pub const OWNED_ADB_SERIAL: &str = "127.0.0.1:15555";
const APP_LIST_PACKAGE_MARKER: &str = "__ANDROID_SIMULATOR_THIRD_PARTY_PACKAGES__";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    pub package: String,
    pub activity: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AppListReport {
    pub apps: Vec<AppInfo>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct InputReport {
    pub action: String,
    pub display_id: Option<u32>,
    pub display_target: String,
    pub executed: bool,
    pub elapsed_ms: u128,
    pub output: String,
}

#[derive(Debug, Serialize)]
pub struct AppUninstallReport {
    pub package: String,
    pub uninstalled: bool,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Tap {
        x: u32,
        y: u32,
    },
    Keyevent {
        key: String,
    },
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u64,
    },
}

pub fn device_is_online(output: &str, serial: &str) -> bool {
    output.lines().any(|line| {
        let mut columns = line.split_whitespace();
        columns.next() == Some(serial) && columns.next() == Some("device")
    })
}

pub fn boot_is_completed(output: &str) -> bool {
    output.lines().any(|line| line.trim() == "1")
}

fn boot_probe_succeeded(status_success: bool, stdout: &str) -> bool {
    status_success && boot_is_completed(stdout)
}

pub fn adb_input_args(action: &InputAction, display_id: Option<u32>) -> Result<Vec<String>> {
    let mut args = vec!["shell".to_string(), "input".to_string()];
    if let Some(display_id) = display_id {
        args.extend(["-d".to_string(), display_id.to_string()]);
    }
    match action {
        InputAction::Tap { x, y } => args.extend(["tap".to_string(), x.to_string(), y.to_string()]),
        InputAction::Keyevent { key } => {
            if key.is_empty()
                || !key
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                bail!("keyevent must contain only ASCII letters, numbers, or underscore");
            }
            args.extend(["keyevent".to_string(), key.to_string()]);
        }
        InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } => {
            if *duration_ms == 0 || *duration_ms > 60_000 {
                bail!("swipe duration must be between 1 and 60000 milliseconds");
            }
            args.extend([
                "swipe".to_string(),
                x1.to_string(),
                y1.to_string(),
                x2.to_string(),
                y2.to_string(),
                duration_ms.to_string(),
            ]);
        }
    }
    Ok(args)
}

pub fn owned_device_online(adb: &Path) -> Result<bool> {
    let _ = Command::new(adb)
        .args(["connect", OWNED_ADB_SERIAL])
        .output();
    let output = Command::new(adb)
        .arg("devices")
        .output()
        .context("failed to list ADB devices")?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(device_is_online(
        &String::from_utf8_lossy(&output.stdout),
        OWNED_ADB_SERIAL,
    ))
}

pub fn owned_device_booted(adb: &Path) -> Result<bool> {
    if let Ok(output) = adb_output(adb, &["shell", "getprop", "sys.boot_completed"])
        && boot_probe_succeeded(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
        )
    {
        return Ok(true);
    }

    if !owned_device_online(adb)? {
        return Ok(false);
    }
    let output = adb_output(adb, &["shell", "getprop", "sys.boot_completed"])?;
    Ok(boot_probe_succeeded(
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
    ))
}

pub fn wait_for_owned_device(adb: &Path, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    let mut observed_online = false;
    while started.elapsed() < timeout {
        if owned_device_online(adb)? {
            observed_online = true;
            let output = adb_output(adb, &["shell", "getprop", "sys.boot_completed"])?;
            if output.status.success()
                && boot_is_completed(&String::from_utf8_lossy(&output.stdout))
            {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(750));
    }
    if observed_online {
        bail!(
            "owned emulator ADB became online but Android did not finish booting within {} seconds",
            timeout.as_secs()
        );
    }
    bail!(
        "owned emulator ADB endpoint {OWNED_ADB_SERIAL} did not become online within {} seconds",
        timeout.as_secs()
    )
}

pub fn list_apps(adb: &Path) -> Result<AppListReport> {
    // Query launchable activities and third-party package ownership in one
    // Android shell. This removes a full ADB process/transport round trip from
    // every library refresh while keeping both result sets independently parsed.
    let script = format!(
        "cmd package query-activities --brief --components -a android.intent.action.MAIN -c android.intent.category.LAUNCHER && printf '\\n{}\\n' && pm list packages -3",
        APP_LIST_PACKAGE_MARKER
    );
    let output = adb_output(adb, &["shell", &script])?;
    let combined = combined_output(&output);
    if !output.status.success() {
        bail!("failed to query launchable Android apps: {combined}");
    }

    let (launcher_output, package_output) = split_combined_app_list_output(&combined)?;
    let third_party = parse_package_list(package_output);
    let apps = filter_user_installed_launcher_apps(
        parse_launcher_components(launcher_output),
        &third_party,
    );
    let count = apps.len();
    Ok(AppListReport { apps, count })
}

fn split_combined_app_list_output(output: &str) -> Result<(&str, &str)> {
    output
        .split_once(APP_LIST_PACKAGE_MARKER)
        .ok_or_else(|| anyhow::anyhow!("Android app list output is missing the package marker"))
}

#[derive(Debug, Serialize)]
pub struct AppStopReport {
    pub package: String,
    pub forced_stop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrcpy_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,
    pub elapsed_ms: u128,
}

impl AppStopReport {
    pub fn with_window(mut self, scrcpy_pid: Option<u32>, display_id: Option<u32>) -> Self {
        self.scrcpy_pid = scrcpy_pid;
        self.display_id = display_id;
        self
    }
}

pub fn resolve_launcher_activity(adb: &Path, package: &str) -> Result<String> {
    validate_package_id(package)?;
    for category in [
        "android.intent.category.LAUNCHER",
        "android.intent.category.LEANBACK_LAUNCHER",
    ] {
        let output = adb_output(
            adb,
            &[
                "shell",
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                category,
                package,
            ],
        )?;
        let combined = combined_output(&output);
        if output.status.success()
            && let Some(activity) = parse_launcher_components(&combined)
                .into_iter()
                .find(|app| app.package == package)
                .map(|app| app.activity)
        {
            return Ok(activity);
        }
    }

    let output = adb_output(
        adb,
        &[
            "shell",
            "cmd",
            "package",
            "query-activities",
            "--brief",
            "--components",
            "-a",
            "android.intent.action.MAIN",
            package,
        ],
    )?;
    let combined = combined_output(&output);
    if output.status.success()
        && let Some(activity) = parse_launcher_components(&combined)
            .into_iter()
            .find(|app| app.package == package)
            .map(|app| app.activity)
    {
        return Ok(activity);
    }

    bail!(
        "Android package {package} has no MAIN activity exposed through LAUNCHER, LEANBACK_LAUNCHER, or package MAIN query"
    )
}

pub fn force_stop_package(adb: &Path, package: &str) -> Result<AppStopReport> {
    validate_package_id(package)?;
    let started = Instant::now();
    let output = adb_output(adb, &["shell", "am", "force-stop", package])?;
    let combined = combined_output(&output);
    if !output.status.success() {
        bail!("failed to force-stop Android package {package}: {combined}");
    }
    Ok(AppStopReport {
        package: package.to_string(),
        forced_stop: true,
        scrcpy_pid: None,
        display_id: None,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

pub fn uninstall_package(adb: &Path, package: &str) -> Result<AppUninstallReport> {
    validate_package_id(package)?;
    let started = Instant::now();
    let output = adb_output(adb, &["uninstall", package])?;
    let combined = combined_output(&output);
    if !output.status.success() || !combined.lines().any(|line| line.trim() == "Success") {
        bail!("failed to uninstall Android package {package}: {combined}");
    }
    Ok(AppUninstallReport {
        package: package.to_string(),
        uninstalled: true,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

pub fn parse_package_list(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("package:"))
        .map(str::trim)
        .filter(|package| validate_package_id(package).is_ok())
        .map(str::to_string)
        .collect()
}

pub fn filter_user_installed_launcher_apps(
    apps: Vec<AppInfo>,
    third_party: &BTreeSet<String>,
) -> Vec<AppInfo> {
    let mut by_package = BTreeMap::new();
    for app in apps {
        if !third_party.contains(&app.package) {
            continue;
        }
        by_package.entry(app.package.clone()).or_insert(app);
    }
    by_package.into_values().collect()
}

pub fn installed_base_apk_path(adb: &Path, package: &str) -> Result<PathBuf> {
    validate_package_id(package)?;
    let output = adb_output(adb, &["shell", "cmd", "package", "path", package])?;
    let combined = combined_output(&output);
    if !output.status.success() {
        bail!("failed to resolve installed APK for {package}: {combined}");
    }

    parse_installed_apk_path(&combined)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Android did not return an APK path for {package}"))
}

fn parse_installed_apk_path(output: &str) -> Option<&str> {
    let paths = output
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("package:"))
        .filter(|path| path.ends_with(".apk"))
        .collect::<Vec<_>>();
    paths
        .iter()
        .copied()
        .find(|path| path.ends_with("/base.apk"))
        .or_else(|| paths.first().copied())
}

fn auto_rotate_setting_value(enabled: bool) -> &'static str {
    if enabled { "1" } else { "0" }
}

pub fn set_auto_rotate(adb: &Path, enabled: bool) -> Result<()> {
    let value = auto_rotate_setting_value(enabled);
    let output = adb_output(
        adb,
        &[
            "shell",
            "settings",
            "put",
            "system",
            "accelerometer_rotation",
            value,
        ],
    )?;
    if !output.status.success() {
        bail!(
            "failed to set Android auto rotation to {enabled}: {}",
            combined_output(&output)
        );
    }
    Ok(())
}

pub fn pull_file(adb: &Path, remote: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = Command::new(adb)
        .args(["-s", OWNED_ADB_SERIAL, "pull"])
        .arg(remote)
        .arg(destination)
        .output()
        .with_context(|| format!("failed to pull Android file {}", remote.display()))?;
    if !output.status.success() || !destination.is_file() {
        bail!(
            "failed to pull Android file {}: {}",
            remote.display(),
            combined_output(&output)
        );
    }
    Ok(())
}

pub fn execute_input(
    adb: &Path,
    action: InputAction,
    display_id: Option<u32>,
) -> Result<InputReport> {
    let started = Instant::now();
    let action_name = match &action {
        InputAction::Tap { .. } => "tap",
        InputAction::Keyevent { .. } => "keyevent",
        InputAction::Swipe { .. } => "swipe",
    }
    .to_string();
    let args = adb_input_args(&action, display_id)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = adb_output(adb, &refs)?;
    let combined = combined_output(&output);
    if !output.status.success() {
        bail!("ADB input {action_name} failed: {combined}");
    }
    Ok(InputReport {
        action: action_name,
        display_id,
        display_target: match display_id {
            None => "default-main-display",
            Some(0) => "main-display",
            Some(_) => "explicit-display-id",
        }
        .to_string(),
        executed: true,
        elapsed_ms: started.elapsed().as_millis(),
        output: combined,
    })
}

fn adb_output(adb: &Path, args: &[&str]) -> Result<Output> {
    Command::new(adb)
        .args(["-s", OWNED_ADB_SERIAL])
        .args(args)
        .output()
        .with_context(|| format!("failed to start ADB at {}", adb.display()))
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn parse_launcher_components(output: &str) -> Vec<AppInfo> {
    let mut apps = BTreeMap::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(component) = line
            .split_whitespace()
            .find(|token| token.chars().filter(|ch| *ch == '/').count() == 1)
        else {
            continue;
        };
        let Some((package, raw_activity)) = component.split_once('/') else {
            continue;
        };
        if validate_package_id(package).is_err() || !valid_activity_name(raw_activity) {
            continue;
        }
        let activity = if raw_activity.starts_with('.') {
            format!("{package}{raw_activity}")
        } else if raw_activity.contains('.') {
            raw_activity.to_string()
        } else {
            format!("{package}.{raw_activity}")
        };
        let app = AppInfo {
            package: package.to_string(),
            activity,
            label: fallback_label(package),
            icon_path: None,
            version: None,
        };
        apps.entry((app.package.clone(), app.activity.clone()))
            .or_insert(app);
    }
    apps.into_values().collect()
}

pub fn validate_package_id(package: &str) -> Result<()> {
    let segments = package.split('.').collect::<Vec<_>>();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || !segment
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
                || !segment
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
    {
        bail!("invalid Android package id: {package}");
    }
    Ok(())
}

fn valid_activity_name(activity: &str) -> bool {
    !activity.is_empty()
        && activity
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '$'))
}

fn fallback_label(package: &str) -> String {
    let raw = package.rsplit('.').next().unwrap_or(package);
    let mut label = String::new();
    let mut uppercase_next = true;
    for ch in raw.chars() {
        if matches!(ch, '_' | '-') {
            if !label.ends_with(' ') && !label.is_empty() {
                label.push(' ');
            }
            uppercase_next = true;
        } else if uppercase_next {
            label.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            label.push(ch);
        }
    }
    if label.is_empty() {
        package.to_string()
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_launcher_components_normalizes_and_deduplicates() {
        let output = r#"
            3 activities found:
            com.example.reader/.MainActivity
            com.example.reader/.MainActivity
            com.android.settings/com.android.settings.Settings
        "#;

        let apps = parse_launcher_components(output);

        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].package, "com.android.settings");
        assert_eq!(apps[0].activity, "com.android.settings.Settings");
        assert_eq!(apps[0].label, "Settings");
        assert_eq!(apps[1].activity, "com.example.reader.MainActivity");
        assert_eq!(apps[1].label, "Reader");
    }

    #[test]
    fn test_split_combined_app_list_output_keeps_both_sections() {
        let output = format!(
            "com.example.reader/.MainActivity\n{}\npackage:com.example.reader\n",
            APP_LIST_PACKAGE_MARKER
        );

        let (launcher_output, package_output) = split_combined_app_list_output(&output).unwrap();

        assert!(launcher_output.contains("com.example.reader/.MainActivity"));
        assert!(package_output.contains("package:com.example.reader"));
    }

    #[test]
    fn test_parse_package_list_keeps_valid_third_party_ids() {
        let packages = parse_package_list(
            "package:com.example.reader\npackage:com.android.settings\nbad\npackage:invalid\n",
        );
        assert!(packages.contains("com.example.reader"));
        assert!(packages.contains("com.android.settings"));
        assert!(!packages.contains("invalid"));
    }

    #[test]
    fn test_filter_user_installed_launcher_apps_drops_system_and_dedupes_packages() {
        let apps = parse_launcher_components(
            r#"
            com.example.reader/.MainActivity
            com.example.reader/.OtherActivity
            com.android.settings/.Settings
            me.him188.ani/.ui.main.MainActivity
            "#,
        );
        let third_party = BTreeSet::from([
            "com.example.reader".to_string(),
            "me.him188.ani".to_string(),
        ]);
        let filtered = filter_user_installed_launcher_apps(apps, &third_party);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].package, "com.example.reader");
        assert_eq!(filtered[0].activity, "com.example.reader.MainActivity");
        assert_eq!(filtered[1].package, "me.him188.ani");
        assert!(
            filtered
                .iter()
                .all(|app| app.package != "com.android.settings")
        );
    }

    #[test]
    fn test_parse_launcher_components_ignores_diagnostics_and_foreign_tokens() {
        let output = r#"
            * daemon started successfully
            No activities found
            warning: transport issue
            package/name/with/too/many/slashes
        "#;

        assert!(parse_launcher_components(output).is_empty());
    }

    #[test]
    fn test_resolve_launcher_activity_parser_accepts_single_component() {
        let apps = parse_launcher_components(
            "com.example.reader/.ui.MainActivity
",
        );
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].package, "com.example.reader");
        assert_eq!(apps[0].activity, "com.example.reader.ui.MainActivity");
    }

    #[test]
    fn test_app_stop_report_carries_window_identity() {
        let report = AppStopReport {
            package: "com.example.reader".to_string(),
            forced_stop: true,
            scrcpy_pid: None,
            display_id: None,
            elapsed_ms: 12,
        }
        .with_window(Some(4242), Some(7));
        assert_eq!(report.scrcpy_pid, Some(4242));
        assert_eq!(report.display_id, Some(7));
    }

    #[test]
    fn test_auto_rotate_setting_value_matches_android_system_contract() {
        assert_eq!(auto_rotate_setting_value(true), "1");
        assert_eq!(auto_rotate_setting_value(false), "0");
    }

    #[test]
    fn test_installed_apk_path_output_prefers_base_apk() {
        let output = "package:/data/app/com.example/split_config.apk\npackage:/data/app/com.example/base.apk\n";
        assert_eq!(
            parse_installed_apk_path(output),
            Some("/data/app/com.example/base.apk")
        );
    }

    #[test]
    fn test_validate_package_id_accepts_android_application_ids() {
        assert!(validate_package_id("me.him188.ani").is_ok());
        assert!(validate_package_id("com.example.reader_2").is_ok());
    }

    #[test]
    fn test_validate_package_id_rejects_shell_and_component_injection() {
        for invalid in [
            "",
            "single",
            ".com.example",
            "com..example",
            "com.example/Activity",
            "com.example;reboot",
            "com.example reader",
        ] {
            assert!(
                validate_package_id(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn test_device_is_online_matches_exact_serial_and_device_state() {
        let output = "List of devices attached\r\n127.0.0.1:5555\tdevice product:x\r\n127.0.0.1:5556\toffline\r\n";
        assert!(device_is_online(output, "127.0.0.1:5555"));
        assert!(!device_is_online(output, "127.0.0.1:5556"));
    }

    #[test]
    fn test_boot_is_completed_requires_exact_one_property() {
        assert!(boot_is_completed("\r\n1\r\n"));
        assert!(!boot_is_completed("10\n"));
        assert!(!boot_is_completed("\n"));
    }

    #[test]
    fn test_boot_probe_succeeded_requires_successful_completed_probe() {
        assert!(boot_probe_succeeded(true, "\r\n1\r\n"));
        assert!(!boot_probe_succeeded(false, "1\n"));
        assert!(!boot_probe_succeeded(true, "0\n"));
    }

    #[test]
    fn test_adb_input_args_builds_safe_commands_and_rejects_invalid_key() {
        assert_eq!(
            adb_input_args(&InputAction::Tap { x: 24, y: 42 }, None).unwrap(),
            ["shell", "input", "tap", "24", "42"]
        );
        assert_eq!(
            adb_input_args(
                &InputAction::Swipe {
                    x1: 1,
                    y1: 2,
                    x2: 3,
                    y2: 4,
                    duration_ms: 250,
                },
                Some(25)
            )
            .unwrap(),
            [
                "shell", "input", "-d", "25", "swipe", "1", "2", "3", "4", "250"
            ]
        );
        assert!(
            adb_input_args(
                &InputAction::Keyevent {
                    key: "KEYCODE_HOME;reboot".to_string(),
                },
                Some(25)
            )
            .is_err()
        );
    }
}
