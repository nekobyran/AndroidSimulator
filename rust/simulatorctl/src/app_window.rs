use crate::{adb, process_priority, runtime_settings, startup_lock};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

pub const SCRCPY_VERSION: &str = "4.1";
pub const SCRCPY_ARCHIVE_NAME: &str = "scrcpy-win64-v4.1.zip";
pub const SCRCPY_ARCHIVE_URL: &str =
    "https://github.com/Genymobile/scrcpy/releases/download/v4.1/scrcpy-win64-v4.1.zip";
pub const SCRCPY_ARCHIVE_SHA256: &str =
    "5b12172b3264b2889f4583ee64752ce832e29bc8b1089dca81093459697165db";
const SCRCPY_CLIENT_FLAVOR: &str = "androidsimulator-enhanced-v1";
const ENHANCED_SCRCPY_RELATIVE_PATH: &str = r"scrcpy-enhanced\scrcpy.exe";
const CENTRAL_SDK_ROOT: &str = r"D:\vibecoding\sdk";
const CENTRAL_ADB_RELATIVE_PATH: &str = r"android\platform-tools\adb.exe";
const SCRCPY_EXTRACTED_DIRECTORY: &str = "scrcpy-win64-v4.1";
const APP_WINDOW_SCHEMA_VERSION: u8 = 2;
const CAPABILITY_CACHE_SCHEMA_VERSION: u8 = 1;
const CAPABILITY_CACHE_VERSION: u8 = 1;
pub const MIN_APP_WINDOW_ANDROID_API: u32 = 30;
pub const PER_APP_AUDIO_MODE: &str = "disabled-per-app-not-implemented";
pub const ANDROID_HOME_PACKAGE: &str = "com.android.launcher3";
pub const ANDROID_HOME_TITLE: &str = "Android()";
const SCRCPY_STARTUP_TIMEOUT: Duration = Duration::from_secs(12);
const SCRCPY_STABILITY_GRACE: Duration = Duration::from_millis(25);
const SCRCPY_STARTUP_POLL: Duration = Duration::from_millis(25);
const APP_WINDOW_LOCK_TIMEOUT: Duration = Duration::from_secs(20);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;

#[derive(Debug, Clone)]
pub struct ScrcpyLayout {
    pub sdk_root: PathBuf,
    pub install_root: PathBuf,
    pub scrcpy_path: PathBuf,
    pub provenance_path: PathBuf,
    pub archive_path: PathBuf,
    pub staging_root: PathBuf,
    pub launch_verification_cache_path: PathBuf,
}

impl ScrcpyLayout {
    pub fn from_sdk_root(sdk_root: &Path) -> Result<Self> {
        let sdk_root = validate_central_sdk_root(sdk_root)?;
        let install_root = sdk_root.join("scrcpy");
        Ok(Self {
            scrcpy_path: install_root.join("scrcpy.exe"),
            provenance_path: install_root.join("provenance.json"),
            archive_path: sdk_root
                .join(".downloads")
                .join("scrcpy")
                .join(SCRCPY_ARCHIVE_NAME),
            staging_root: sdk_root.join(".staging").join("scrcpy"),
            launch_verification_cache_path: install_root.join("launch-verification-cache.json"),
            sdk_root,
            install_root,
        })
    }

    pub fn central_adb_path(&self) -> PathBuf {
        self.sdk_root.join(CENTRAL_ADB_RELATIVE_PATH)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrcpyStatus {
    pub install_root: PathBuf,
    pub scrcpy_path: PathBuf,
    pub provenance_path: PathBuf,
    pub archive_path: PathBuf,
    pub central_adb_path: PathBuf,
    pub available: bool,
    pub version: Option<String>,
    pub blocker: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrcpyProvisionReport {
    pub schema_version: u8,
    pub version: String,
    pub source: String,
    pub archive_url: String,
    pub archive_path: PathBuf,
    pub expected_archive_sha256: String,
    pub actual_archive_sha256: String,
    pub install_root: PathBuf,
    pub scrcpy_path: PathBuf,
    pub executable_sha256: String,
    pub central_adb_path: PathBuf,
    pub version_output: String,
    pub reused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ScrcpyProvenance {
    schema_version: u8,
    version: String,
    archive_url: String,
    archive_sha256: String,
    executable_sha256: String,
    #[serde(default)]
    client_flavor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ScrcpyLaunchVerificationCache {
    schema_version: u8,
    version: String,
    archive_sha256: String,
    executable_sha256: String,
    #[serde(default)]
    client_flavor: String,
    executable_length: u64,
    executable_modified_unix_nanos: u128,
    version_output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OwnedRuntimeCapabilities {
    schema_version: u8,
    version: u8,
    adb_serial: String,
    android_api_level: u32,
    multiwindow_supported: bool,
}

impl OwnedRuntimeCapabilities {
    fn is_trusted_supported_record(&self) -> bool {
        self.schema_version == CAPABILITY_CACHE_SCHEMA_VERSION
            && self.version == CAPABILITY_CACHE_VERSION
            && self.adb_serial == adb::OWNED_ADB_SERIAL
            && self.android_api_level >= MIN_APP_WINDOW_ANDROID_API
            && self.multiwindow_supported
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppWindowMetadata {
    pub schema_version: u8,
    pub package: String,
    pub title: String,
    #[serde(default)]
    pub icon_path: Option<PathBuf>,
    pub window_pid: u32,
    pub android_api_level: u32,
    pub required_android_api_level: u32,
    pub display_id: Option<u32>,
    pub display_status: String,
    pub fallback_mode: String,
    pub audio_mode: String,
    pub scrcpy_path: PathBuf,
    pub adb_path: PathBuf,
    pub command_line: Vec<String>,
    #[serde(default)]
    pub super_resolution_scale_percent: Option<u32>,
    #[serde(default)]
    pub presentation_fps: Option<u32>,
    #[serde(default)]
    pub vertical_sync: bool,
    #[serde(default)]
    pub dynamic_frame_rate: bool,
    #[serde(default)]
    pub dynamic_low_frame_rate: u32,
    pub log_path: PathBuf,
    pub started_at_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppWindowReport {
    pub schema_version: u8,
    pub package: String,
    pub title: String,
    pub icon_path: Option<PathBuf>,
    pub window_pid: Option<u32>,
    pub window_reused: bool,
    pub android_api_level: u32,
    pub required_android_api_level: u32,
    pub display_id: Option<u32>,
    pub display_status: String,
    pub fallback_mode: String,
    pub audio_mode: String,
    pub scrcpy_path: PathBuf,
    pub adb_path: PathBuf,
    pub command_line: Vec<String>,
    pub super_resolution_scale_percent: Option<u32>,
    pub presentation_fps: Option<u32>,
    pub vertical_sync: bool,
    pub dynamic_frame_rate: bool,
    pub dynamic_low_frame_rate: u32,
    pub metadata_path: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    pub runtime_started: bool,
}

impl AppWindowReport {
    pub fn display_ready(&self) -> bool {
        self.display_id.is_some() && self.display_status == "ready"
    }

    fn from_metadata(
        metadata: AppWindowMetadata,
        metadata_path: PathBuf,
        window_reused: bool,
        runtime_started: bool,
    ) -> Self {
        Self {
            schema_version: APP_WINDOW_SCHEMA_VERSION,
            package: metadata.package,
            title: metadata.title,
            icon_path: metadata.icon_path,
            window_pid: Some(metadata.window_pid),
            window_reused,
            android_api_level: metadata.android_api_level,
            required_android_api_level: metadata.required_android_api_level,
            display_id: metadata.display_id,
            display_status: metadata.display_status,
            fallback_mode: metadata.fallback_mode,
            audio_mode: metadata.audio_mode,
            scrcpy_path: metadata.scrcpy_path,
            adb_path: metadata.adb_path,
            command_line: metadata.command_line,
            super_resolution_scale_percent: metadata.super_resolution_scale_percent,
            presentation_fps: metadata.presentation_fps,
            vertical_sync: metadata.vertical_sync,
            dynamic_frame_rate: metadata.dynamic_frame_rate,
            dynamic_low_frame_rate: metadata.dynamic_low_frame_rate,
            metadata_path: Some(metadata_path),
            log_path: Some(metadata.log_path),
            runtime_started,
        }
    }

    fn unsupported(
        package: &str,
        title: String,
        android_api_level: u32,
        spec: ScrcpyCommandSpec,
        runtime_started: bool,
    ) -> Self {
        let command_line = spec.command_line();
        Self {
            schema_version: APP_WINDOW_SCHEMA_VERSION,
            package: package.to_string(),
            title,
            icon_path: None,
            window_pid: None,
            window_reused: false,
            android_api_level,
            required_android_api_level: MIN_APP_WINDOW_ANDROID_API,
            display_id: None,
            display_status: "unsupported".to_string(),
            fallback_mode: "none".to_string(),
            audio_mode: PER_APP_AUDIO_MODE.to_string(),
            scrcpy_path: spec.program,
            adb_path: spec.adb_path,
            command_line,
            super_resolution_scale_percent: spec.super_resolution_scale_percent,
            presentation_fps: spec.presentation_fps,
            vertical_sync: spec.vertical_sync,
            dynamic_frame_rate: spec.dynamic_frame_rate,
            dynamic_low_frame_rate: spec.dynamic_low_frame_rate,
            metadata_path: None,
            log_path: None,
            runtime_started,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScrcpyCommandSpec {
    program: PathBuf,
    args: Vec<String>,
    current_dir: PathBuf,
    adb_path: PathBuf,
    super_resolution_scale_percent: Option<u32>,
    presentation_fps: Option<u32>,
    vertical_sync: bool,
    dynamic_frame_rate: bool,
    dynamic_low_frame_rate: u32,
}

impl ScrcpyCommandSpec {
    fn command_line(&self) -> Vec<String> {
        std::iter::once(self.program.to_string_lossy().to_string())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScrcpyEnvironment {
    path: OsString,
    adb: PathBuf,
}

pub fn resolve_window_title(package: &str, title: Option<&str>) -> Result<String> {
    adb::validate_package_id(package)?;
    let title = title.unwrap_or(package).trim();
    if title.is_empty() {
        bail!("app window title cannot be empty");
    }
    if title.chars().count() > 160 {
        bail!("app window title cannot exceed 160 characters");
    }
    if title.chars().any(char::is_control) {
        bail!("app window title cannot contain NUL or other control characters");
    }
    Ok(title.to_string())
}

pub fn inspect_scrcpy(sdk_root: &Path) -> Result<ScrcpyStatus> {
    let layout = ScrcpyLayout::from_sdk_root(sdk_root)?;
    let central_adb_path = layout.central_adb_path();
    let mut status = ScrcpyStatus {
        install_root: layout.install_root.clone(),
        scrcpy_path: layout.scrcpy_path.clone(),
        provenance_path: layout.provenance_path.clone(),
        archive_path: layout.archive_path.clone(),
        central_adb_path: central_adb_path.clone(),
        available: false,
        version: None,
        blocker: None,
    };

    let result = inspect_current_provision(&layout, &central_adb_path);
    match result {
        Ok(version) => {
            status.available = true;
            status.version = Some(version);
        }
        Err(error) => status.blocker = Some(format!("{error:#}")),
    }
    Ok(status)
}

pub fn provision_scrcpy(sdk_root: &Path) -> Result<ScrcpyProvisionReport> {
    let layout = ScrcpyLayout::from_sdk_root(sdk_root)?;
    let central_adb_path = layout.central_adb_path();
    if let Ok(version_output) = inspect_current_provision(&layout, &central_adb_path) {
        let provenance = read_provenance(&layout.provenance_path)?;
        return Ok(ScrcpyProvisionReport {
            schema_version: APP_WINDOW_SCHEMA_VERSION,
            version: SCRCPY_VERSION.to_string(),
            source: "official-github-release+androidsimulator-enhanced-client".to_string(),
            archive_url: SCRCPY_ARCHIVE_URL.to_string(),
            archive_path: layout.archive_path,
            expected_archive_sha256: SCRCPY_ARCHIVE_SHA256.to_string(),
            actual_archive_sha256: SCRCPY_ARCHIVE_SHA256.to_string(),
            install_root: layout.install_root,
            scrcpy_path: layout.scrcpy_path,
            executable_sha256: provenance.executable_sha256,
            central_adb_path,
            version_output,
            reused: true,
        });
    }

    if let Some(parent) = layout.archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(&layout.staging_root)?;
    let actual_archive_sha256 = download_verified_archive(
        SCRCPY_ARCHIVE_URL,
        &layout.archive_path,
        SCRCPY_ARCHIVE_SHA256,
    )?;
    let staging =
        layout
            .staging_root
            .join(format!("extract-{}-{}", std::process::id(), unix_millis()?));
    fs::create_dir(&staging)?;

    let install_result = (|| -> Result<String> {
        extract_official_archive(&layout.archive_path, &staging)?;
        let extracted_root = staging.join(SCRCPY_EXTRACTED_DIRECTORY);
        let extracted_scrcpy = extracted_root.join("scrcpy.exe");
        if !extracted_scrcpy.is_file() {
            bail!(
                "verified scrcpy archive did not contain {}/scrcpy.exe",
                SCRCPY_EXTRACTED_DIRECTORY
            );
        }
        let enhanced_scrcpy = layout.sdk_root.join(ENHANCED_SCRCPY_RELATIVE_PATH);
        if !enhanced_scrcpy.is_file() {
            bail!(
                "Android Simulator enhanced scrcpy client is missing: {}. Run command/Build-EnhancedScrcpy.ps1 first.",
                enhanced_scrcpy.display()
            );
        }
        fs::copy(&enhanced_scrcpy, &extracted_scrcpy).with_context(|| {
            format!(
                "failed to install Android Simulator enhanced scrcpy client from {}",
                enhanced_scrcpy.display()
            )
        })?;
        let executable_sha256 = sha256_file(&extracted_scrcpy)?;
        let provenance = ScrcpyProvenance {
            schema_version: APP_WINDOW_SCHEMA_VERSION,
            version: SCRCPY_VERSION.to_string(),
            archive_url: SCRCPY_ARCHIVE_URL.to_string(),
            archive_sha256: SCRCPY_ARCHIVE_SHA256.to_string(),
            executable_sha256: executable_sha256.clone(),
            client_flavor: SCRCPY_CLIENT_FLAVOR.to_string(),
        };
        atomic_write_json(&extracted_root.join("provenance.json"), &provenance)?;
        install_extracted_directory(&layout, &staging, &extracted_root)?;
        Ok(executable_sha256)
    })();
    let _ = fs::remove_dir_all(&staging);
    let executable_sha256 = install_result?;
    let version_output = inspect_current_provision(&layout, &central_adb_path)?;

    Ok(ScrcpyProvisionReport {
        schema_version: APP_WINDOW_SCHEMA_VERSION,
        version: SCRCPY_VERSION.to_string(),
        source: "official-github-release+androidsimulator-enhanced-client".to_string(),
        archive_url: SCRCPY_ARCHIVE_URL.to_string(),
        archive_path: layout.archive_path,
        expected_archive_sha256: SCRCPY_ARCHIVE_SHA256.to_string(),
        actual_archive_sha256,
        install_root: layout.install_root,
        scrcpy_path: layout.scrcpy_path,
        executable_sha256,
        central_adb_path,
        version_output,
        reused: false,
    })
}

pub fn launch_app_window(
    runtime_root: &Path,
    sdk_root: &Path,
    adb_path: &Path,
    package: &str,
    title: &str,
    icon_path: Option<&Path>,
    runtime_started: bool,
) -> Result<AppWindowReport> {
    adb::validate_package_id(package)?;
    let title = if package == ANDROID_HOME_PACKAGE {
        ANDROID_HOME_TITLE.to_string()
    } else {
        resolve_window_title(package, Some(title))?
    };
    let layout = ScrcpyLayout::from_sdk_root(sdk_root)?;
    inspect_current_provision_for_launch(&layout, adb_path).context(
        "official scrcpy 4.1 is not provisioned; run `simulatorctl owned provision` first",
    )?;
    let settings = runtime_settings::load(runtime_root)?;
    adb::set_auto_rotate(adb_path, settings.auto_rotate)
        .context("failed to apply Android auto-rotation setting")?;
    let spec = build_scrcpy_command(&layout, adb_path, package, &title, &settings)?;
    let app_windows_root = runtime_root.join("app-windows");
    fs::create_dir_all(app_windows_root.join("locks"))?;
    fs::create_dir_all(app_windows_root.join("logs"))?;
    let capability_cache_path = app_windows_root.join("owned-runtime-capabilities.json");
    let android_api_level = match read_supported_capability_cache(&capability_cache_path) {
        Some(capabilities) => capabilities.android_api_level,
        None => {
            let android_api_level = query_android_api_level(adb_path)?;
            if require_virtual_display_api_level(android_api_level).is_err() {
                return Ok(AppWindowReport::unsupported(
                    package,
                    title,
                    android_api_level,
                    spec,
                    runtime_started,
                ));
            }
            let multiwindow_supported = query_multiwindow_support(adb_path)?;
            require_multiwindow_support(multiwindow_supported)?;
            let capabilities =
                build_supported_capability_cache(android_api_level, multiwindow_supported)?;
            atomic_write_json(&capability_cache_path, &capabilities)
                .context("failed to persist verified owned-runtime capabilities")?;
            android_api_level
        }
    };
    let lock_path = package_lock_path(&app_windows_root, package)?;
    let _lock = startup_lock::StartupLock::acquire(&lock_path, APP_WINDOW_LOCK_TIMEOUT)?;

    if package == ANDROID_HOME_PACKAGE {
        activate_android_main_display(adb_path)?;
    }

    if let Some((mut metadata, metadata_path)) =
        find_live_window(&app_windows_root, package, &layout.scrcpy_path, adb_path)?
    {
        if !scrcpy_command_contract_matches(&metadata.command_line, &spec)
            || metadata.super_resolution_scale_percent != spec.super_resolution_scale_percent
            || metadata.presentation_fps != spec.presentation_fps
            || metadata.vertical_sync != spec.vertical_sync
            || metadata.dynamic_frame_rate != spec.dynamic_frame_rate
            || metadata.dynamic_low_frame_rate != spec.dynamic_low_frame_rate
            || visible_scrcpy_sdl_window(metadata.window_pid)?.is_none()
        {
            retire_verified_scrcpy_process(metadata.window_pid, &layout.scrcpy_path)?;
            fs::remove_file(&metadata_path).with_context(|| {
                format!(
                    "failed to remove stale app-window metadata {}",
                    metadata_path.display()
                )
            })?;
        } else {
            process_priority::promote_latency_sensitive_process(
                metadata.window_pid,
                settings.performance_mode,
            )?;
            if metadata.icon_path.is_none() && icon_path.is_some() {
                metadata.icon_path = icon_path.map(Path::to_path_buf);
                atomic_write_json_fast(&metadata_path, &metadata)?;
            }
            if metadata.display_id.is_none()
                && let Ok(log) = read_lossy(&metadata.log_path)
                && let Some(display_id) = parse_new_display_id(&log)
            {
                metadata.display_id = Some(display_id);
                metadata.display_status = "ready".to_string();
                atomic_write_json_fast(&metadata_path, &metadata)?;
            }
            if package != ANDROID_HOME_PACKAGE
                && let Some(display_id) = metadata.display_id
            {
                // scrcpy's --start-app may race its own virtual-display registration
                // and leave a live video process showing only a black surface. Always
                // re-assert the launcher activity after the display id is known. This
                // also repairs warm hosts created by an older build that hit the race.
                launch_android_app_on_display(adb_path, package, display_id)?;
            }
            let _ = apply_scrcpy_window_chrome(metadata.window_pid, metadata.icon_path.as_deref());
            return Ok(AppWindowReport::from_metadata(
                metadata,
                metadata_path,
                true,
                runtime_started,
            ));
        }
    }

    let started_at_unix_ms = unix_millis()?;
    let log_path = app_windows_root
        .join("logs")
        .join(format!("{}-{started_at_unix_ms}.log", package));
    let log_file = fs::File::create(&log_path)?;
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    configure_scrcpy_command(&mut command, &spec)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .with_context(|| {
            format!(
                "failed to start official scrcpy at {}",
                spec.program.display()
            )
        })?;
    let metadata_path = window_metadata_path(&app_windows_root, child.id());
    let mut metadata = AppWindowMetadata {
        schema_version: APP_WINDOW_SCHEMA_VERSION,
        package: package.to_string(),
        title,
        icon_path: icon_path.map(Path::to_path_buf),
        window_pid: child.id(),
        android_api_level,
        required_android_api_level: MIN_APP_WINDOW_ANDROID_API,
        display_id: None,
        display_status: "pending".to_string(),
        fallback_mode: "none".to_string(),
        audio_mode: PER_APP_AUDIO_MODE.to_string(),
        scrcpy_path: spec.program.clone(),
        adb_path: spec.adb_path.clone(),
        command_line: spec.command_line(),
        super_resolution_scale_percent: spec.super_resolution_scale_percent,
        presentation_fps: spec.presentation_fps,
        vertical_sync: spec.vertical_sync,
        dynamic_frame_rate: spec.dynamic_frame_rate,
        dynamic_low_frame_rate: spec.dynamic_low_frame_rate,
        log_path,
        started_at_unix_ms,
    };
    if let Err(error) = atomic_write_json_fast(&metadata_path, &metadata) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context("failed to persist scrcpy app-window metadata");
    }

    let fixed_display = (package == ANDROID_HOME_PACKAGE).then_some(0);
    match wait_for_scrcpy_startup_on_display(
        &mut child,
        &metadata.log_path,
        SCRCPY_STARTUP_TIMEOUT,
        fixed_display,
    ) {
        Ok(Some(display_id)) => {
            if package != ANDROID_HOME_PACKAGE
                && let Err(error) = launch_android_app_on_display(adb_path, package, display_id)
            {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&metadata_path);
                return Err(error).context(
                    "virtual display was created but the Android application did not start",
                );
            }
            metadata.display_id = Some(display_id);
            metadata.display_status = "ready".to_string();
            atomic_write_json_fast(&metadata_path, &metadata)?;
        }
        Ok(None) => {}
        Err(error) => {
            let _ = fs::remove_file(&metadata_path);
            return Err(error);
        }
    }
    if let Err(error) = process_priority::promote_latency_sensitive_process(
        metadata.window_pid,
        settings.performance_mode,
    ) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&metadata_path);
        return Err(error).context("failed to pin scrcpy latency priority");
    }
    let _ = apply_scrcpy_window_chrome(metadata.window_pid, metadata.icon_path.as_deref());

    Ok(AppWindowReport::from_metadata(
        metadata,
        metadata_path,
        false,
        runtime_started,
    ))
}

fn validate_central_sdk_root(candidate: &Path) -> Result<PathBuf> {
    let text = candidate.to_string_lossy().replace('/', "\\");
    if text
        .split('\\')
        .any(|component| matches!(component, "." | ".."))
    {
        bail!("scrcpy SDK root must already be normalized to {CENTRAL_SDK_ROOT}");
    }
    if !normalized_windows_path_key(&text).eq_ignore_ascii_case(CENTRAL_SDK_ROOT) {
        bail!(
            "scrcpy SDK root must resolve exactly to {CENTRAL_SDK_ROOT}; got {}",
            candidate.display()
        );
    }
    Ok(PathBuf::from(CENTRAL_SDK_ROOT))
}

fn normalized_windows_path_key(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .unwrap_or(path)
        .trim_end_matches('\\')
        .to_string()
}

fn validate_central_adb(layout: &ScrcpyLayout, adb_path: &Path) -> Result<PathBuf> {
    let expected = layout.central_adb_path();
    let actual = normalized_windows_path_key(&adb_path.to_string_lossy().replace('/', "\\"));
    let expected_key = normalized_windows_path_key(&expected.to_string_lossy());
    if !actual.eq_ignore_ascii_case(&expected_key) {
        bail!(
            "scrcpy must reuse the central Android SDK adb at {}; got {}",
            expected.display(),
            adb_path.display()
        );
    }
    if !adb_path.is_file() {
        bail!("central Android SDK adb is missing: {}", adb_path.display());
    }
    Ok(expected)
}

fn build_scrcpy_command(
    layout: &ScrcpyLayout,
    adb_path: &Path,
    package: &str,
    title: &str,
    settings: &runtime_settings::SimulatorSettings,
) -> Result<ScrcpyCommandSpec> {
    build_scrcpy_command_for_windows_profile(
        layout,
        adb_path,
        package,
        title,
        settings,
        host_windows_dpi(),
        host_display_refresh_rate(),
    )
}

fn scrcpy_command_contract_matches(
    recorded_command_line: &[String],
    expected: &ScrcpyCommandSpec,
) -> bool {
    recorded_command_line == expected.command_line()
}

fn build_scrcpy_command_for_windows_profile(
    layout: &ScrcpyLayout,
    adb_path: &Path,
    package: &str,
    title: &str,
    settings: &runtime_settings::SimulatorSettings,
    windows_dpi: u32,
    host_refresh_hz: u32,
) -> Result<ScrcpyCommandSpec> {
    adb::validate_package_id(package)?;
    settings.validate()?;
    let title = resolve_window_title(package, Some(title))?;
    let adb_path = validate_central_adb(layout, adb_path)?;
    let mut args = vec!["--serial".to_string(), adb::OWNED_ADB_SERIAL.to_string()];
    if package != ANDROID_HOME_PACKAGE {
        let (display_width, display_height, android_density) =
            settings.initial_display_profile(windows_dpi);
        args.push(format!(
            "--new-display={display_width}x{display_height}/{android_density}"
        ));
        if !settings.fixed_window_size {
            args.push("--flex-display".to_string());
        }
        args.push("--no-window-aspect-ratio-lock".to_string());
    }
    args.extend(["--window-title".to_string(), title]);
    if package != ANDROID_HOME_PACKAGE {
        args.push("--no-vd-system-decorations".to_string());
    }
    if !settings.system_audio || package != ANDROID_HOME_PACKAGE {
        args.push("--no-audio".to_string());
    }
    args.push(format!(
        "--render-driver={}",
        match settings.renderer_mode {
            runtime_settings::RendererMode::Direct3d => "direct3d",
            runtime_settings::RendererMode::Opengl => "opengl",
        }
    ));
    if settings.renderer_mode == runtime_settings::RendererMode::Direct3d
        || !settings.super_resolution
    {
        args.push("--no-mipmaps".to_string());
    }
    args.extend([
        format!(
            "--max-fps={}",
            effective_capture_refresh_rate(host_refresh_hz).min(settings.capture_fps())
        ),
        format!("--max-size={}", settings.effective_max_size()),
        format!("--video-bit-rate={}", settings.effective_video_bit_rate()),
        "--video-codec=h264".to_string(),
        "--video-buffer=0".to_string(),
    ]);
    let super_resolution_scale_percent = (package != ANDROID_HOME_PACKAGE
        && settings.super_resolution)
        .then_some(settings.super_resolution_scale_percent.clamp(100, 200));
    let presentation_fps = (settings.frame_interpolation
        && settings.presentation_fps() > settings.capture_fps())
    .then_some(settings.presentation_fps());
    Ok(ScrcpyCommandSpec {
        program: layout.scrcpy_path.clone(),
        args,
        current_dir: layout.install_root.clone(),
        adb_path,
        super_resolution_scale_percent,
        presentation_fps,
        vertical_sync: settings.vertical_sync,
        dynamic_frame_rate: settings.dynamic_frame_rate,
        dynamic_low_frame_rate: settings.dynamic_low_frame_rate,
    })
}
// BlissOS exposes a software H.264 encoder and its virtual-display source has
// repeatedly measured at 60 Hz. Requesting 120 fps doubles encoder pressure
// without producing additional source frames, which increases frame drops.
const ANDROID_VIRTUAL_DISPLAY_MAX_FPS: u32 = 60;
const FALLBACK_DISPLAY_REFRESH_HZ: u32 = 60;

fn effective_capture_refresh_rate(host_refresh_hz: u32) -> u32 {
    let host_refresh_hz = if (1..=240).contains(&host_refresh_hz) {
        host_refresh_hz
    } else {
        FALLBACK_DISPLAY_REFRESH_HZ
    };
    host_refresh_hz.min(ANDROID_VIRTUAL_DISPLAY_MAX_FPS)
}

#[cfg(windows)]
fn host_display_refresh_rate() -> u32 {
    use std::mem::size_of;
    use windows_sys::Win32::Graphics::Gdi::{
        DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW,
    };

    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let available =
        unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode) } != 0;
    if available {
        mode.dmDisplayFrequency
    } else {
        0
    }
}

#[cfg(not(windows))]
fn host_display_refresh_rate() -> u32 {
    FALLBACK_DISPLAY_REFRESH_HZ
}

fn android_app_launch_args(package: &str, activity: &str, display_id: u32) -> Result<Vec<String>> {
    adb::validate_package_id(package)?;
    if display_id == 0 {
        bail!("independent Android applications require a non-main display id");
    }
    if activity.is_empty()
        || activity.contains('/')
        || !activity
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '$'))
    {
        bail!("invalid Android launcher activity: {activity}");
    }
    Ok(vec![
        "-s".to_string(),
        adb::OWNED_ADB_SERIAL.to_string(),
        "shell".to_string(),
        "am".to_string(),
        "start".to_string(),
        "--display".to_string(),
        display_id.to_string(),
        "-n".to_string(),
        format!("{package}/{activity}"),
        "--activity-reorder-to-front".to_string(),
        "--activity-clear-top".to_string(),
    ])
}

fn launch_android_app_on_display(adb_path: &Path, package: &str, display_id: u32) -> Result<()> {
    let activity = adb::resolve_launcher_activity(adb_path, package)?;
    let args = android_app_launch_args(package, &activity, display_id)?;
    let mut command = Command::new(adb_path);
    command.args(args);
    configure_background_process(&mut command);
    let output = command
        .output()
        .with_context(|| format!("failed to start {package} on Android display {display_id}"))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() || combined.contains("Error:") {
        bail!(
            "failed to start {package} on Android display {display_id}: {}",
            combined.trim()
        );
    }
    Ok(())
}

fn android_main_display_home_args() -> [&'static str; 15] {
    [
        "-s",
        adb::OWNED_ADB_SERIAL,
        "shell",
        "am",
        "start",
        "--display",
        "0",
        "-a",
        "android.intent.action.MAIN",
        "-c",
        "android.intent.category.HOME",
        "-n",
        "com.android.launcher3/.uioverrides.QuickstepLauncher",
        "--activity-reorder-to-front",
        "--activity-clear-top",
    ]
}

fn activate_android_main_display(adb_path: &Path) -> Result<()> {
    let mut command = Command::new(adb_path);
    command.args(android_main_display_home_args());
    configure_background_process(&mut command);
    let output = command
        .output()
        .context("failed to activate the Android main display")?;
    if !output.status.success() {
        bail!(
            "failed to activate the Android main display: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(windows)]
fn host_windows_dpi() -> u32 {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDpiForSystem() -> u32;
        fn SetThreadDpiAwarenessContext(dpi_context: isize) -> isize;
    }

    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
    // Query from a per-monitor-aware context so a non-manifested control CLI
    // does not receive DPI-virtualized 96 on a scaled desktop.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let dpi = GetDpiForSystem();
        if previous != 0 {
            let _ = SetThreadDpiAwarenessContext(previous);
        }
        dpi.max(72)
    }
}

#[cfg(not(windows))]
fn host_windows_dpi() -> u32 {
    96
}

fn system_default_border_color() -> u32 {
    // DWMWA_BORDER_COLOR accepts this sentinel to restore Windows' own edge.
    // Material orange remains an in-app accent, never a full-window outline.
    0xFFFF_FFFF
}

#[cfg(windows)]
fn visible_scrcpy_sdl_window(process_id: u32) -> Result<Option<isize>> {
    #[repr(C)]
    struct WindowSearchContext {
        process_id: u32,
        window_handle: isize,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
            parameter: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
        fn IsWindowVisible(window: isize) -> i32;
        fn GetClassNameW(window: isize, class_name: *mut u16, maximum: i32) -> i32;
    }

    unsafe extern "system" fn find_scrcpy_window(window: isize, parameter: isize) -> i32 {
        let context = unsafe { &mut *(parameter as *mut WindowSearchContext) };
        if unsafe { IsWindowVisible(window) } == 0 {
            return 1;
        }
        let mut candidate_process_id = 0;
        unsafe { GetWindowThreadProcessId(window, &mut candidate_process_id) };
        if candidate_process_id != context.process_id {
            return 1;
        }
        let mut class_name = [0u16; 64];
        let length = unsafe {
            GetClassNameW(
                window,
                class_name.as_mut_ptr(),
                i32::try_from(class_name.len()).unwrap_or(64),
            )
        };
        let is_scrcpy_window =
            length > 0 && String::from_utf16_lossy(&class_name[..length as usize]) == "SDL_app";
        if !is_scrcpy_window {
            return 1;
        }
        context.window_handle = window;
        0
    }

    let mut context = WindowSearchContext {
        process_id,
        window_handle: 0,
    };
    unsafe {
        EnumWindows(
            Some(find_scrcpy_window),
            (&mut context as *mut WindowSearchContext) as isize,
        )
    };
    Ok((context.window_handle != 0).then_some(context.window_handle))
}

#[cfg(not(windows))]
fn visible_scrcpy_sdl_window(_process_id: u32) -> Result<Option<isize>> {
    Ok(None)
}

#[cfg(windows)]
fn apply_scrcpy_window_chrome(process_id: u32, icon_path: Option<&Path>) -> Result<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};

    #[link(name = "user32")]
    unsafe extern "system" {
        fn LoadImageW(
            instance: isize,
            name: *const u16,
            image_type: u32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> isize;
        fn SendMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
    }
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            window: isize,
            attribute: u32,
            value: *const c_void,
            value_size: u32,
        ) -> i32;
    }

    let Some(window_handle) = visible_scrcpy_sdl_window(process_id)? else {
        bail!("scrcpy SDL window was not ready for native styling");
    };

    let dark_mode = windows_apps_use_dark_mode() as i32;
    let corner_preference = 2i32;
    let border_color = system_default_border_color();
    let backdrop_type = 3i32;
    for (attribute, value, size) in [
        (20, (&dark_mode as *const i32).cast::<c_void>(), 4),
        (33, (&corner_preference as *const i32).cast::<c_void>(), 4),
        (34, (&border_color as *const u32).cast::<c_void>(), 4),
        (38, (&backdrop_type as *const i32).cast::<c_void>(), 4),
    ] {
        let result = unsafe { DwmSetWindowAttribute(window_handle, attribute, value, size) };
        if result < 0 {
            bail!("DwmSetWindowAttribute({attribute}) failed with HRESULT {result:#x}");
        }
    }

    if let Some(icon_path) = icon_path.filter(|path| path.is_file()) {
        const IMAGE_ICON: u32 = 1;
        const LR_LOADFROMFILE: u32 = 0x0010;
        const LR_DEFAULTSIZE: u32 = 0x0040;
        const LR_SHARED: u32 = 0x8000;
        const WM_SETICON: u32 = 0x0080;
        let wide = icon_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let icon = unsafe {
            LoadImageW(
                0,
                wide.as_ptr(),
                IMAGE_ICON,
                0,
                0,
                LR_LOADFROMFILE | LR_DEFAULTSIZE | LR_SHARED,
            )
        };
        if icon != 0 {
            unsafe {
                SendMessageW(window_handle, WM_SETICON, 0, icon);
                SendMessageW(window_handle, WM_SETICON, 1, icon);
                SendMessageW(window_handle, WM_SETICON, 2, icon);
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn apply_scrcpy_window_chrome(_process_id: u32, _icon_path: Option<&Path>) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn windows_apps_use_dark_mode() -> bool {
    let mut command = Command::new("reg.exe");
    command.args([
        "QUERY",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "/v",
        "AppsUseLightTheme",
    ]);
    configure_background_process(&mut command);
    command.output().ok().is_some_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .to_ascii_lowercase()
                .lines()
                .any(|line| line.contains("appsuselighttheme") && line.trim_end().ends_with("0x0"))
    })
}

fn scrcpy_environment(
    spec: &ScrcpyCommandSpec,
    inherited_path: Option<&OsStr>,
) -> Result<ScrcpyEnvironment> {
    let adb_root = spec
        .adb_path
        .parent()
        .ok_or_else(|| anyhow!("central adb path has no parent directory"))?;
    let mut entries = vec![adb_root.to_path_buf(), spec.current_dir.clone()];
    if let Some(inherited) = inherited_path {
        entries.extend(env::split_paths(inherited));
    }
    Ok(ScrcpyEnvironment {
        path: env::join_paths(entries)?,
        adb: spec.adb_path.clone(),
    })
}

fn configure_scrcpy_command(command: &mut Command, spec: &ScrcpyCommandSpec) -> Result<()> {
    let inherited_path = env::var_os("PATH");
    let environment = scrcpy_environment(spec, inherited_path.as_deref())?;
    command
        .current_dir(&spec.current_dir)
        .env("ADB", &environment.adb)
        .env("PATH", environment.path);
    if let Some(scale) = spec.super_resolution_scale_percent {
        command.env(
            "ANDROID_SIMULATOR_SUPER_RESOLUTION_SCALE_PERCENT",
            scale.to_string(),
        );
    }
    if let Some(fps) = spec.presentation_fps {
        command.env("ANDROID_SIMULATOR_PRESENTATION_FPS", fps.to_string());
    }
    command.env(
        "ANDROID_SIMULATOR_VSYNC",
        if spec.vertical_sync { "1" } else { "0" },
    );
    command.env(
        "ANDROID_SIMULATOR_DYNAMIC_FRAME_RATE",
        if spec.dynamic_frame_rate { "1" } else { "0" },
    );
    command.env(
        "ANDROID_SIMULATOR_DYNAMIC_LOW_FRAME_RATE",
        spec.dynamic_low_frame_rate.to_string(),
    );
    configure_background_process(command);
    Ok(())
}

fn inspect_current_provision_for_launch(layout: &ScrcpyLayout, adb_path: &Path) -> Result<String> {
    validate_central_adb(layout, adb_path)?;
    if !layout.scrcpy_path.is_file() {
        bail!(
            "official scrcpy executable is missing: {}",
            layout.scrcpy_path.display()
        );
    }
    let provenance = read_provenance(&layout.provenance_path)?;
    validate_pinned_scrcpy_provenance(&provenance)?;
    let (executable_length, executable_modified_unix_nanos) =
        scrcpy_file_identity(&layout.scrcpy_path)?;
    if let Ok(bytes) = fs::read(&layout.launch_verification_cache_path)
        && let Ok(cache) = serde_json::from_slice::<ScrcpyLaunchVerificationCache>(&bytes)
        && cache.schema_version == APP_WINDOW_SCHEMA_VERSION
        && cache.version == SCRCPY_VERSION
        && cache.client_flavor == SCRCPY_CLIENT_FLAVOR
        && cache
            .archive_sha256
            .eq_ignore_ascii_case(SCRCPY_ARCHIVE_SHA256)
        && cache
            .executable_sha256
            .eq_ignore_ascii_case(&provenance.executable_sha256)
        && cache.executable_length == executable_length
        && cache.executable_modified_unix_nanos == executable_modified_unix_nanos
        && cache
            .version_output
            .starts_with(&format!("scrcpy {SCRCPY_VERSION} "))
    {
        return Ok(cache.version_output);
    }

    inspect_current_provision(layout, adb_path)
}

fn validate_pinned_scrcpy_provenance(provenance: &ScrcpyProvenance) -> Result<()> {
    if provenance.schema_version != APP_WINDOW_SCHEMA_VERSION
        || provenance.version != SCRCPY_VERSION
        || provenance.archive_url != SCRCPY_ARCHIVE_URL
        || provenance.client_flavor != SCRCPY_CLIENT_FLAVOR
        || !provenance
            .archive_sha256
            .eq_ignore_ascii_case(SCRCPY_ARCHIVE_SHA256)
    {
        bail!("scrcpy provenance is not the pinned official v{SCRCPY_VERSION} release");
    }
    Ok(())
}

fn scrcpy_file_identity(path: &Path) -> Result<(u64, u128)> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    Ok((metadata.len(), modified))
}

fn cache_verified_scrcpy_launch(
    layout: &ScrcpyLayout,
    provenance: &ScrcpyProvenance,
    version_output: &str,
) -> Result<()> {
    let (executable_length, executable_modified_unix_nanos) =
        scrcpy_file_identity(&layout.scrcpy_path)?;
    let cache = ScrcpyLaunchVerificationCache {
        schema_version: APP_WINDOW_SCHEMA_VERSION,
        version: SCRCPY_VERSION.to_string(),
        archive_sha256: SCRCPY_ARCHIVE_SHA256.to_string(),
        executable_sha256: provenance.executable_sha256.clone(),
        client_flavor: SCRCPY_CLIENT_FLAVOR.to_string(),
        executable_length,
        executable_modified_unix_nanos,
        version_output: version_output.to_string(),
    };
    atomic_write_json_fast(&layout.launch_verification_cache_path, &cache)
}

fn inspect_current_provision(layout: &ScrcpyLayout, adb_path: &Path) -> Result<String> {
    validate_central_adb(layout, adb_path)?;
    if !layout.scrcpy_path.is_file() {
        bail!(
            "official scrcpy executable is missing: {}",
            layout.scrcpy_path.display()
        );
    }
    let provenance = read_provenance(&layout.provenance_path)?;
    validate_pinned_scrcpy_provenance(&provenance)?;
    let actual_executable_sha256 = sha256_file(&layout.scrcpy_path)?;
    if !actual_executable_sha256.eq_ignore_ascii_case(&provenance.executable_sha256) {
        bail!("scrcpy executable hash does not match its verified installation provenance");
    }
    let version_output = scrcpy_version(&layout.scrcpy_path, &layout.install_root)?;
    cache_verified_scrcpy_launch(layout, &provenance, &version_output)?;
    Ok(version_output)
}

fn read_provenance(path: &Path) -> Result<ScrcpyProvenance> {
    let bytes = fs::read(path).with_context(|| {
        format!(
            "scrcpy provenance is missing or unreadable: {}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).context("scrcpy provenance JSON is invalid")
}

fn scrcpy_version(scrcpy_path: &Path, install_root: &Path) -> Result<String> {
    let mut command = Command::new(scrcpy_path);
    command.arg("--version").current_dir(install_root);
    configure_background_process(&mut command);
    let output = command
        .output()
        .context("failed to inspect scrcpy version")?;
    if !output.status.success() {
        bail!(
            "scrcpy --version failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let version_line = combined
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| anyhow!("scrcpy --version returned no output"))?;
    if !version_line.starts_with(&format!("scrcpy {SCRCPY_VERSION} ")) {
        bail!("unexpected scrcpy version: {version_line}");
    }
    Ok(version_line.to_string())
}

fn download_verified_archive(url: &str, path: &Path, expected_sha256: &str) -> Result<String> {
    if path.is_file() {
        let actual = sha256_file(path)?;
        if actual.eq_ignore_ascii_case(expected_sha256) {
            return Ok(actual);
        }
        fs::remove_file(path).with_context(|| {
            format!(
                "failed to remove corrupt scrcpy archive: {}",
                path.display()
            )
        })?;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut response = client
        .get(url)
        .header("User-Agent", "android-simulator-scrcpy-provisioner")
        .send()
        .with_context(|| format!("failed to download official scrcpy release: {url}"))?;
    if !response.status().is_success() {
        bail!(
            "official scrcpy download failed with HTTP {}",
            response.status()
        );
    }
    let temporary = sibling_temporary_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    response.copy_to(&mut file)?;
    file.flush()?;
    file.sync_all()?;
    let actual = sha256_file(&temporary)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = fs::remove_file(&temporary);
        bail!("official scrcpy archive SHA256 mismatch: expected {expected_sha256}, got {actual}");
    }
    atomic_replace_file(&temporary, path)?;
    Ok(actual)
}

fn extract_official_archive(archive: &Path, destination: &Path) -> Result<()> {
    let output = Command::new("tar.exe")
        .args(["-xf"])
        .arg(archive)
        .args(["-C"])
        .arg(destination)
        .output()
        .context("Windows tar.exe is required to extract the verified scrcpy archive")?;
    if !output.status.success() {
        bail!(
            "failed to extract verified scrcpy archive: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn install_extracted_directory(
    layout: &ScrcpyLayout,
    staging_session: &Path,
    extracted_root: &Path,
) -> Result<()> {
    ensure_direct_child(&layout.staging_root, staging_session)?;
    ensure_direct_child(staging_session, extracted_root)?;
    ensure_direct_child(&layout.sdk_root, &layout.install_root)?;
    let backup = layout.sdk_root.join(format!(
        ".scrcpy-replaced-{}-{}",
        std::process::id(),
        unix_millis()?
    ));
    let had_existing = layout.install_root.exists();
    if had_existing {
        fs::rename(&layout.install_root, &backup)
            .context("failed to stage existing scrcpy install")?;
    }
    if let Err(error) = fs::rename(extracted_root, &layout.install_root) {
        if had_existing {
            let _ = fs::rename(&backup, &layout.install_root);
        }
        return Err(error).context("failed to activate verified scrcpy install");
    }
    if had_existing {
        fs::remove_dir_all(&backup).context("failed to remove replaced scrcpy install")?;
    }
    Ok(())
}

fn ensure_direct_child(parent: &Path, child: &Path) -> Result<()> {
    if child.parent() != Some(parent) || child == parent {
        bail!(
            "unsafe scrcpy filesystem target: {} is not a direct child of {}",
            child.display(),
            parent.display()
        );
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_new_display_id(log: &str) -> Option<u32> {
    log.lines().find_map(|line| {
        let line = line.trim();
        if !line.starts_with("[server]") || !line.contains(" INFO: New display:") {
            return None;
        }
        let marker = "(id=";
        let start = line.rfind(marker)? + marker.len();
        let end = line[start..].find(')')? + start;
        let id = line[start..end].parse::<u32>().ok()?;
        (id > 0).then_some(id)
    })
}

fn read_supported_capability_cache(path: &Path) -> Option<OwnedRuntimeCapabilities> {
    let bytes = fs::read(path).ok()?;
    parse_supported_capability_cache(&bytes)
}

fn parse_supported_capability_cache(bytes: &[u8]) -> Option<OwnedRuntimeCapabilities> {
    let capabilities = serde_json::from_slice::<OwnedRuntimeCapabilities>(bytes).ok()?;
    capabilities
        .is_trusted_supported_record()
        .then_some(capabilities)
}

fn build_supported_capability_cache(
    android_api_level: u32,
    multiwindow_supported: bool,
) -> Result<OwnedRuntimeCapabilities> {
    require_virtual_display_api_level(android_api_level)?;
    require_multiwindow_support(multiwindow_supported)?;
    Ok(OwnedRuntimeCapabilities {
        schema_version: CAPABILITY_CACHE_SCHEMA_VERSION,
        version: CAPABILITY_CACHE_VERSION,
        adb_serial: adb::OWNED_ADB_SERIAL.to_string(),
        android_api_level,
        multiwindow_supported,
    })
}

fn query_android_api_level(adb_path: &Path) -> Result<u32> {
    let mut command = Command::new(adb_path);
    command.args([
        "-s",
        adb::OWNED_ADB_SERIAL,
        "shell",
        "getprop",
        "ro.build.version.sdk",
    ]);
    configure_background_process(&mut command);
    let output = command
        .output()
        .context("failed to query the owned Android API level")?;
    if !output.status.success() {
        bail!(
            "failed to query the owned Android API level: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    parse_android_api_level(&String::from_utf8_lossy(&output.stdout))
}

fn parse_android_api_level(output: &str) -> Result<u32> {
    let values = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if values.len() != 1 {
        bail!("Android API query returned an ambiguous value: {output:?}");
    }
    let api = values[0]
        .parse::<u32>()
        .with_context(|| format!("invalid Android API level: {:?}", values[0]))?;
    if api == 0 || api > 10_000 {
        bail!("invalid Android API level: {api}");
    }
    Ok(api)
}

fn require_virtual_display_api_level(api: u32) -> Result<()> {
    if api < MIN_APP_WINDOW_ANDROID_API {
        bail!(
            "Per-app virtual display windows require Android 11+ (API 30+); the owned image reports API {api}. There is no whole-device/QEMU-window fallback."
        );
    }
    Ok(())
}

fn query_multiwindow_support(adb_path: &Path) -> Result<bool> {
    let mut command = Command::new(adb_path);
    command.args([
        "-s",
        adb::OWNED_ADB_SERIAL,
        "shell",
        "cmd",
        "activity",
        "supports-multiwindow",
    ]);
    configure_background_process(&mut command);
    let output = command
        .output()
        .context("failed to query Android secondary-display support")?;
    if !output.status.success() {
        bail!(
            "failed to query Android secondary-display support: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    parse_multiwindow_support(&String::from_utf8_lossy(&output.stdout))
}

fn parse_multiwindow_support(output: &str) -> Result<bool> {
    match output.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("Android multi-window query returned an invalid value: {output:?}"),
    }
}

fn require_multiwindow_support(supported: bool) -> Result<()> {
    if !supported {
        bail!(
            "Android secondary displays are disabled because the guest is in low-RAM mode; the owned runtime must use at least 3 GiB of guest memory."
        );
    }
    Ok(())
}

#[cfg(test)]
fn wait_for_scrcpy_startup(
    child: &mut Child,
    log_path: &Path,
    timeout: Duration,
) -> Result<Option<u32>> {
    wait_for_scrcpy_startup_on_display(child, log_path, timeout, None)
}

fn wait_for_scrcpy_startup_on_display(
    child: &mut Child,
    log_path: &Path,
    timeout: Duration,
    fixed_display: Option<u32>,
) -> Result<Option<u32>> {
    let started = Instant::now();
    let mut observed: Option<(u32, Instant)> = None;
    loop {
        if let Some(status) = child.try_wait()? {
            let log = read_lossy(log_path).unwrap_or_default();
            bail!("scrcpy exited during startup with {status}. Log: {log}");
        }
        if let Ok(log) = read_lossy(log_path) {
            if scrcpy_log_has_fatal_video_error(&log) {
                return fail_scrcpy_startup(
                    child,
                    format!("scrcpy video pipeline failed during startup. Log: {log}"),
                );
            }
            let window_visible = visible_scrcpy_sdl_window(child.id())?.is_some();
            match scrcpy_ready_display_id(&log, fixed_display, window_visible) {
                Some(display_id) if observed.is_none() => {
                    observed = Some((display_id, Instant::now()));
                }
                Some(display_id)
                    if observed.is_some_and(|(observed_id, _)| observed_id != display_id) =>
                {
                    observed = Some((display_id, Instant::now()));
                }
                Some(_) => {}
                None => observed = None,
            }
        }
        if let Some(display_id) =
            stable_display_id(observed, Instant::now(), SCRCPY_STABILITY_GRACE)
        {
            return Ok(Some(display_id));
        }
        if started.elapsed() >= timeout {
            let log = read_lossy(log_path).unwrap_or_default();
            return fail_scrcpy_startup(
                child,
                format!(
                    "timed out after {} ms waiting for a visible scrcpy SDL window. Log: {log}",
                    timeout.as_millis()
                ),
            );
        }
        thread::sleep(SCRCPY_STARTUP_POLL);
    }
}

fn scrcpy_ready_display_id(
    log: &str,
    fixed_display: Option<u32>,
    sdl_window_visible: bool,
) -> Option<u32> {
    if !sdl_window_visible {
        return None;
    }
    fixed_display
        .filter(|_| log.contains("INFO: Texture:"))
        .or_else(|| parse_new_display_id(log))
}

fn fail_scrcpy_startup<T>(child: &mut Child, message: String) -> Result<T> {
    match terminate_starting_scrcpy(child) {
        Ok(()) => Err(anyhow!(message)),
        Err(cleanup_error) => Err(anyhow!(
            "{message}; additionally failed to terminate scrcpy: {cleanup_error:#}"
        )),
    }
}

fn terminate_starting_scrcpy(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child
            .kill()
            .context("failed to terminate starting scrcpy")?;
        child
            .wait()
            .context("failed to reap terminated scrcpy process")?;
    }
    Ok(())
}

fn stable_display_id(
    observed: Option<(u32, Instant)>,
    now: Instant,
    stability_grace: Duration,
) -> Option<u32> {
    observed.and_then(|(display_id, observed_at)| {
        (now.saturating_duration_since(observed_at) >= stability_grace).then_some(display_id)
    })
}

fn scrcpy_log_has_fatal_video_error(log: &str) -> bool {
    log.contains("Capture/encoding error")
        || log.contains("MediaCodec$CodecException")
        || log.contains("Device disconnected")
}

fn window_metadata_path(app_windows_root: &Path, pid: u32) -> PathBuf {
    app_windows_root.join(format!("{pid}.json"))
}

fn package_lock_path(app_windows_root: &Path, package: &str) -> Result<PathBuf> {
    adb::validate_package_id(package)?;
    Ok(app_windows_root
        .join("locks")
        .join(format!("{package}.lock")))
}

pub fn stop_app_session(
    runtime_root: &Path,
    sdk_root: &Path,
    adb_path: &Path,
    package: &str,
) -> Result<adb::AppStopReport> {
    adb::validate_package_id(package)?;
    let layout = ScrcpyLayout::from_sdk_root(sdk_root)?;
    let app_windows_root = runtime_root.join("app-windows");
    let mut scrcpy_pid = None;
    let mut display_id = None;
    if let Some((metadata, metadata_path)) =
        find_live_window(&app_windows_root, package, &layout.scrcpy_path, adb_path)?
    {
        scrcpy_pid = Some(metadata.window_pid);
        display_id = metadata.display_id;
        let _ = retire_verified_scrcpy_process(metadata.window_pid, &metadata.scrcpy_path);
        let _ = fs::remove_file(metadata_path);
    }

    let report = if adb::owned_device_online(adb_path).unwrap_or(false) {
        adb::force_stop_package(adb_path, package)?
    } else {
        adb::AppStopReport {
            package: package.to_string(),
            forced_stop: false,
            scrcpy_pid: None,
            display_id: None,
            elapsed_ms: 0,
        }
    };
    Ok(report.with_window(scrcpy_pid, display_id))
}

fn live_app_window_process_ids(
    runtime_root: &Path,
    sdk_root: &Path,
    adb_path: &Path,
) -> Result<Vec<u32>> {
    let layout = ScrcpyLayout::from_sdk_root(sdk_root)?;
    let app_windows_root = runtime_root.join("app-windows");
    if !app_windows_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut process_ids = Vec::new();
    for entry in fs::read_dir(&app_windows_root)? {
        let entry = entry?;
        let path = entry.path();
        let Some(file_pid) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u32>().ok())
        else {
            continue;
        };
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Some(metadata) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AppWindowMetadata>(&bytes).ok())
        else {
            continue;
        };
        if metadata.schema_version != APP_WINDOW_SCHEMA_VERSION
            || metadata.window_pid != file_pid
            || !exact_windows_executable_path(&metadata.scrcpy_path, &layout.scrcpy_path)
            || !exact_windows_executable_path(&metadata.adb_path, adb_path)
        {
            continue;
        }
        match process_matches_exact_executable(metadata.window_pid, &layout.scrcpy_path) {
            Ok(true) => process_ids.push(metadata.window_pid),
            Ok(false) | Err(_) => {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(process_ids)
}

pub fn has_live_app_windows(runtime_root: &Path, sdk_root: &Path, adb_path: &Path) -> Result<bool> {
    Ok(!live_app_window_process_ids(runtime_root, sdk_root, adb_path)?.is_empty())
}

pub fn refresh_live_app_window_priorities(
    runtime_root: &Path,
    sdk_root: &Path,
    adb_path: &Path,
    mode: runtime_settings::PerformanceMode,
) -> Result<usize> {
    let process_ids = live_app_window_process_ids(runtime_root, sdk_root, adb_path)?;
    for process_id in &process_ids {
        process_priority::promote_latency_sensitive_process(*process_id, mode)?;
    }
    Ok(process_ids.len())
}

fn find_live_window(
    app_windows_root: &Path,
    package: &str,
    expected_scrcpy: &Path,
    expected_adb: &Path,
) -> Result<Option<(AppWindowMetadata, PathBuf)>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(app_windows_root)? {
        let entry = entry?;
        let path = entry.path();
        let Some(file_pid) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u32>().ok())
        else {
            continue;
        };
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Ok(mut metadata) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AppWindowMetadata>(&bytes).ok())
            .ok_or(())
        else {
            continue;
        };
        if metadata.schema_version != APP_WINDOW_SCHEMA_VERSION
            || metadata.window_pid != file_pid
            || metadata.package != package
        {
            continue;
        }
        if !exact_windows_executable_path(&metadata.scrcpy_path, expected_scrcpy)
            || !exact_windows_executable_path(&metadata.adb_path, expected_adb)
        {
            bail!(
                "refusing unsafe app-window metadata for package {package}: tool path mismatch at {}",
                path.display()
            );
        }
        match process_matches_exact_executable(metadata.window_pid, expected_scrcpy) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                // A dead scrcpy PID may already have been reused by a protected
                // Windows process. It is stale metadata, not a reason to block
                // launching the package again.
                let _ = fs::remove_file(&path);
                continue;
            }
        }
        if metadata.display_id.is_none()
            && let Ok(log) = read_lossy(&metadata.log_path)
            && let Some(display_id) = parse_new_display_id(&log)
        {
            metadata.display_id = Some(display_id);
            metadata.display_status = "ready".to_string();
            atomic_write_json_fast(&path, &metadata)?;
        }
        found.push((metadata, path));
    }
    if found.len() > 1 {
        bail!(
            "multiple verified scrcpy windows exist for package {package}; refusing ambiguous reuse"
        );
    }
    Ok(found.pop())
}

pub(crate) fn exact_windows_executable_path(left: &Path, right: &Path) -> bool {
    let left = normalized_windows_path_key(&left.to_string_lossy().replace('/', "\\"));
    let right = normalized_windows_path_key(&right.to_string_lossy().replace('/', "\\"));
    left.eq_ignore_ascii_case(&right)
}

#[cfg(windows)]
fn retire_verified_scrcpy_process(pid: u32, expected: &Path) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
        },
    };

    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;

    if !process_matches_exact_executable(pid, expected)? {
        bail!("refusing to retire process {pid}: executable identity changed");
    }
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE_ACCESS, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error())
            .context("failed to open stale verified scrcpy process");
    }
    let terminated = unsafe { TerminateProcess(handle, 0) };
    let terminate_error = (terminated == 0).then(std::io::Error::last_os_error);
    let wait_result = if terminated != 0 {
        unsafe { WaitForSingleObject(handle, 5_000) }
    } else {
        u32::MAX
    };
    unsafe {
        CloseHandle(handle);
    }
    if let Some(error) = terminate_error {
        return Err(error).context("failed to retire stale verified scrcpy process");
    }
    if wait_result != WAIT_OBJECT_0 {
        bail!("timed out retiring stale verified scrcpy process {pid}");
    }
    Ok(())
}

#[cfg(not(windows))]
fn retire_verified_scrcpy_process(_pid: u32, _expected: &Path) -> Result<()> {
    bail!("retiring a stale scrcpy process is only supported on Windows")
}

fn atomic_write_json_fast<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = sibling_temporary_path(path)?;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, value)?;
        file.write_all(b"\n")?;
        file.flush()?;
        atomic_replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = sibling_temporary_path(path)?;
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
        atomic_replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sibling_temporary_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("file has no parent directory: {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("file has no name: {}", path.display()))?
        .to_string_lossy();
    Ok(parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        unix_millis()?
    )))
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                destination.display(),
                source.display()
            )
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn process_matches_exact_executable(pid: u32, expected: &Path) -> Result<bool> {
    let Some(actual) = process_executable_path(pid)? else {
        return Ok(false);
    };
    Ok(exact_windows_executable_path(&actual, expected))
}

#[cfg(not(windows))]
pub(crate) fn process_matches_exact_executable(pid: u32, expected: &Path) -> Result<bool> {
    let path = PathBuf::from(format!("/proc/{pid}/exe"));
    match fs::read_link(path) {
        Ok(actual) => Ok(actual == expected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn process_executable_path(pid: u32) -> Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(87) {
            return Ok(None);
        }
        return Err(error).context("failed to securely inspect scrcpy process");
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut size = buffer.len() as u32;
    let queried = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    let query_error = (queried == 0).then(std::io::Error::last_os_error);
    unsafe {
        CloseHandle(handle);
    }
    if let Some(error) = query_error {
        return Err(error).context("failed to query scrcpy process image path");
    }
    buffer.truncate(size as usize);
    Ok(Some(PathBuf::from(OsString::from_wide(&buffer))))
}

fn read_lossy(path: &Path) -> Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

fn unix_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_millis())
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW | NORMAL_PRIORITY_CLASS);
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
        time::Duration,
    };

    #[test]
    fn test_scrcpy_layout_is_only_central_d_sdk() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();

        assert_eq!(
            layout.install_root,
            PathBuf::from(r"D:\vibecoding\sdk\scrcpy")
        );
        assert_eq!(
            layout.scrcpy_path,
            PathBuf::from(r"D:\vibecoding\sdk\scrcpy\scrcpy.exe")
        );
        assert!(layout.archive_path.starts_with(r"D:\vibecoding\sdk"));
        assert!(layout.staging_root.starts_with(r"D:\vibecoding\sdk"));
        assert!(ScrcpyLayout::from_sdk_root(Path::new(r"C:\vibecoding\sdk")).is_err());
        assert!(ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk\..\sdk")).is_err());
    }

    #[test]
    fn test_scrcpy_staging_allows_only_the_expected_two_level_extract_tree() {
        let staging_root = Path::new(r"D:\vibecoding\sdk\.staging\scrcpy");
        let session = staging_root.join("extract-100-200");
        let extracted = session.join(SCRCPY_EXTRACTED_DIRECTORY);
        assert!(ensure_direct_child(staging_root, &session).is_ok());
        assert!(ensure_direct_child(&session, &extracted).is_ok());
        assert!(
            ensure_direct_child(staging_root, &extracted).is_err(),
            "nested extracted root must not masquerade as a direct staging child"
        );
        assert!(
            ensure_direct_child(&session, &staging_root.join("outside")).is_err(),
            "an extracted root outside the unique staging session must be rejected"
        );
    }

    #[test]
    fn test_scrcpy_official_release_contract_is_pinned() {
        assert_eq!(SCRCPY_VERSION, "4.1");
        assert_eq!(SCRCPY_ARCHIVE_NAME, "scrcpy-win64-v4.1.zip");
        assert_eq!(
            SCRCPY_ARCHIVE_URL,
            "https://github.com/Genymobile/scrcpy/releases/download/v4.1/scrcpy-win64-v4.1.zip"
        );
        assert_eq!(
            SCRCPY_ARCHIVE_SHA256,
            "5b12172b3264b2889f4583ee64752ce832e29bc8b1089dca81093459697165db"
        );
    }

    #[test]
    fn test_scrcpy_command_maps_one_package_to_one_virtual_display_window() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();
        let adb = Path::new(r"D:\vibecoding\sdk\android\platform-tools\adb.exe");
        let settings = runtime_settings::SimulatorSettings::default();

        let spec = build_scrcpy_command_for_windows_profile(
            &layout,
            adb,
            "com.example.reader",
            "Reader's Desk",
            &settings,
            96,
            144,
        )
        .unwrap();
        let joined = spec.command_line().join(" ");

        assert_eq!(spec.program, layout.scrcpy_path);
        assert_eq!(spec.adb_path, adb);
        assert!(joined.contains("--serial 127.0.0.1:15555"));
        assert!(joined.contains("--new-display=1280x720/160"));
        assert!(joined.contains("--flex-display"));
        assert!(!joined.contains("--render-fit=stretched"));
        assert!(!joined.contains("--start-app"));
        assert!(joined.contains("--window-title Reader's Desk"));
        assert!(joined.contains("--no-vd-system-decorations"));
        assert!(joined.contains("--no-audio"));
        assert!(joined.contains("--render-driver=direct3d"));
        assert!(joined.contains("--no-mipmaps"));
        assert!(joined.contains("--max-fps=60"));
        assert!(joined.contains("--max-size=1920"));
        assert!(joined.contains("--video-bit-rate=8M"));
        assert!(joined.contains("--video-buffer=0"));
        assert!(!joined.to_ascii_lowercase().contains("qemu"));
        assert!(!joined.to_ascii_lowercase().contains("shell"));

        let current = spec.command_line();
        assert!(scrcpy_command_contract_matches(&current, &spec));
        let mut stale = current;
        stale.retain(|argument| argument != "--flex-display");
        assert!(!scrcpy_command_contract_matches(&stale, &spec));
    }

    #[test]
    fn test_fixed_window_size_disables_flex_display() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();
        let adb = Path::new(r"D:\vibecoding\sdk\android\platform-tools\adb.exe");
        let settings = runtime_settings::SimulatorSettings {
            fixed_window_size: true,
            ..runtime_settings::SimulatorSettings::default()
        };

        let spec = build_scrcpy_command_for_windows_profile(
            &layout,
            adb,
            "com.example.reader",
            "Reader",
            &settings,
            96,
            60,
        )
        .unwrap();
        assert!(!spec.command_line().contains(&"--flex-display".to_string()));
    }
    #[test]
    fn test_enhanced_scrcpy_receives_real_supersampling_and_interpolation_contract() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();
        let adb = Path::new(r"D:\vibecoding\sdk\android\platform-tools\adb.exe");
        let settings = runtime_settings::SimulatorSettings {
            super_resolution: true,
            super_resolution_scale_percent: 150,
            frame_interpolation: true,
            max_frame_rate: 120,
            renderer_mode: runtime_settings::RendererMode::Opengl,
            ..runtime_settings::SimulatorSettings::default()
        };

        let spec = build_scrcpy_command_for_windows_profile(
            &layout,
            adb,
            "com.example.reader",
            "Reader",
            &settings,
            96,
            144,
        )
        .unwrap();
        assert_eq!(spec.super_resolution_scale_percent, Some(150));
        assert_eq!(spec.presentation_fps, Some(120));
        assert!(
            spec.command_line()
                .join(" ")
                .contains("--new-display=1920x1080/240")
        );
        assert!(!spec.command_line().contains(&"--no-mipmaps".to_string()));

        let mut command = Command::new(&spec.program);
        configure_scrcpy_command(&mut command, &spec).unwrap();
        let environment = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            environment.get(OsStr::new(
                "ANDROID_SIMULATOR_SUPER_RESOLUTION_SCALE_PERCENT"
            )),
            Some(&OsString::from("150"))
        );
        assert_eq!(
            environment.get(OsStr::new("ANDROID_SIMULATOR_PRESENTATION_FPS")),
            Some(&OsString::from("120"))
        );
        assert_eq!(
            environment.get(OsStr::new("ANDROID_SIMULATOR_DYNAMIC_FRAME_RATE")),
            Some(&OsString::from("1"))
        );
        assert_eq!(
            environment.get(OsStr::new("ANDROID_SIMULATOR_DYNAMIC_LOW_FRAME_RATE")),
            Some(&OsString::from("15"))
        );
    }

    #[test]
    fn test_host_refresh_rate_is_sanitized_for_low_latency_capture() {
        assert_eq!(effective_capture_refresh_rate(0), 60);
        assert_eq!(effective_capture_refresh_rate(24), 24);
        assert_eq!(effective_capture_refresh_rate(59), 59);
        assert_eq!(effective_capture_refresh_rate(60), 60);
        assert_eq!(effective_capture_refresh_rate(90), 60);
        assert_eq!(effective_capture_refresh_rate(144), 60);
        assert_eq!(effective_capture_refresh_rate(500), 60);
    }

    #[test]
    fn test_android_app_is_started_only_after_the_virtual_display_is_ready() {
        assert_eq!(
            android_app_launch_args("com.example.reader", "cc.reader.ui.MainActivity", 25).unwrap(),
            [
                "-s",
                "127.0.0.1:15555",
                "shell",
                "am",
                "start",
                "--display",
                "25",
                "-n",
                "com.example.reader/cc.reader.ui.MainActivity",
                "--activity-reorder-to-front",
                "--activity-clear-top"
            ]
        );
        assert!(android_app_launch_args("com.example.reader", "bad/activity;reboot", 25).is_err());
    }

    #[test]
    fn test_android_home_mirrors_the_persistent_main_display() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();
        let adb = Path::new(r"D:\vibecoding\sdk\android\platform-tools\adb.exe");
        let settings = runtime_settings::SimulatorSettings::default();

        let spec = build_scrcpy_command_for_windows_profile(
            &layout,
            adb,
            ANDROID_HOME_PACKAGE,
            ANDROID_HOME_TITLE,
            &settings,
            96,
            144,
        )
        .unwrap();
        let joined = spec.command_line().join(" ");

        assert!(!joined.contains("--new-display"));
        assert!(!joined.contains("--flex-display"));
        assert!(!joined.contains("--no-vd-system-decorations"));
        assert!(joined.contains("--window-title Android()"));
        assert!(!joined.contains("--start-app"));
        assert!(joined.contains("--max-size=1920"));
        assert_eq!(
            android_main_display_home_args(),
            [
                "-s",
                "127.0.0.1:15555",
                "shell",
                "am",
                "start",
                "--display",
                "0",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.HOME",
                "-n",
                "com.android.launcher3/.uioverrides.QuickstepLauncher",
                "--activity-reorder-to-front",
                "--activity-clear-top"
            ]
        );
    }
    #[test]
    fn test_android_virtual_display_scales_with_desktop_dpi() {
        let settings = runtime_settings::SimulatorSettings::default();
        assert_eq!(settings.initial_display_profile(96), (1280, 720, 160));
        assert_eq!(settings.initial_display_profile(144), (1920, 1080, 240));
        assert_eq!(settings.initial_display_profile(192), (2560, 1440, 320));
    }

    #[test]
    fn test_scrcpy_window_border_uses_windows_default_instead_of_accent_outline() {
        assert_eq!(system_default_border_color(), 0xFFFF_FFFF);
    }

    #[test]
    fn test_scrcpy_command_rejects_title_and_path_injection() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();
        let adb = Path::new(r"D:\vibecoding\sdk\android\platform-tools\adb.exe");
        let settings = runtime_settings::SimulatorSettings::default();
        assert!(
            build_scrcpy_command(&layout, adb, "com.example.reader", "bad\0title", &settings)
                .is_err()
        );
        assert!(
            build_scrcpy_command(
                &layout,
                adb,
                "com.example.reader",
                &"x".repeat(161),
                &settings,
            )
            .is_err()
        );
        assert!(
            build_scrcpy_command(
                &layout,
                Path::new(r"C:\adb.exe"),
                "com.example.reader",
                "Reader",
                &settings,
            )
            .is_err()
        );
    }
    #[test]
    fn test_owned_process_path_identity_requires_exact_executable() {
        assert!(exact_windows_executable_path(
            Path::new(r"D:\vibecoding\sdk\msys64\ucrt64\bin\qemu-system-x86_64.exe"),
            Path::new(r"d:\VIBECODING\sdk\msys64\ucrt64\bin\qemu-system-x86_64.exe")
        ));
        assert!(!exact_windows_executable_path(
            Path::new(r"D:\other\qemu-system-x86_64.exe"),
            Path::new(r"D:\vibecoding\sdk\msys64\ucrt64\bin\qemu-system-x86_64.exe")
        ));
    }

    #[test]
    fn test_scrcpy_environment_forces_the_central_android_sdk_adb() {
        let layout = ScrcpyLayout::from_sdk_root(Path::new(r"D:\vibecoding\sdk")).unwrap();
        let adb = Path::new(r"D:\vibecoding\sdk\android\platform-tools\adb.exe");
        let settings = runtime_settings::SimulatorSettings::default();
        let spec =
            build_scrcpy_command(&layout, adb, "com.example.reader", "Reader", &settings).unwrap();

        let environment =
            scrcpy_environment(&spec, Some(OsStr::new(r"C:\Windows\System32"))).unwrap();
        assert_eq!(environment.adb, adb);
        assert_eq!(
            std::env::split_paths(&environment.path).next(),
            adb.parent().map(Path::to_path_buf)
        );
    }
    #[test]
    fn test_parse_new_display_id_accepts_only_scrcpy_server_record() {
        let log = "scrcpy 4.1\n[server] INFO: New display: 1280x720/240 (id=25)\nINFO: Renderer: direct3d\n";
        assert_eq!(parse_new_display_id(log), Some(25));
        assert_eq!(parse_new_display_id("display id=25"), None);
        assert_eq!(
            parse_new_display_id("[server] INFO: New display: 1x1/1 (id=4294967296)"),
            None
        );
    }

    #[test]
    fn test_scrcpy_fatal_video_errors_are_never_reported_ready() {
        assert!(scrcpy_log_has_fatal_video_error(
            "[server] ERROR: Capture/encoding error: java.lang.IllegalStateException"
        ));
        assert!(scrcpy_log_has_fatal_video_error(
            "android.media.MediaCodec$CodecException: Error 0x80000000"
        ));
        assert!(!scrcpy_log_has_fatal_video_error(
            "[server] INFO: New display: 1280x720/240 (id=2)"
        ));
    }

    #[test]
    fn test_android_api_gate_requires_android_11_for_app_windows() {
        assert_eq!(parse_android_api_level("30\r\n").unwrap(), 30);
        assert!(require_virtual_display_api_level(30).is_ok());
        let error = require_virtual_display_api_level(28)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Android 11+"));
        assert!(error.contains("API 30+"));
        assert!(error.contains("no whole-device/QEMU-window fallback"));
        assert!(parse_android_api_level("unknown").is_err());
    }

    #[test]
    fn test_multiwindow_gate_rejects_low_ram_runtime() {
        assert!(parse_multiwindow_support("true\r\n").unwrap());
        assert!(!parse_multiwindow_support("false\n").unwrap());
        assert!(parse_multiwindow_support("unknown").is_err());

        let error = require_multiwindow_support(false).unwrap_err().to_string();
        assert!(error.contains("secondary displays"));
        assert!(error.contains("low-RAM"));
        assert!(error.contains("3 GiB"));
    }

    #[test]
    fn test_capability_cache_parses_only_verified_owned_runtime_support() {
        let cached = parse_supported_capability_cache(
            br#"{
                "schema_version": 1,
                "version": 1,
                "adb_serial": "127.0.0.1:15555",
                "android_api_level": 35,
                "multiwindow_supported": true
            }"#,
        )
        .unwrap();

        assert_eq!(cached.android_api_level, 35);
        assert_eq!(cached.adb_serial, adb::OWNED_ADB_SERIAL);
    }

    #[test]
    fn test_capability_cache_atomic_write_roundtrips_as_a_cache_hit() {
        let test_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join(format!("capability-cache-test-{}", std::process::id()));
        let cache_path = test_root.join("owned-runtime-capabilities.json");
        let capabilities = build_supported_capability_cache(35, true).unwrap();

        atomic_write_json(&cache_path, &capabilities).unwrap();
        let cached = read_supported_capability_cache(&cache_path).unwrap();

        assert_eq!(cached, capabilities);
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn test_capability_cache_invalidates_untrusted_or_unsupported_records() {
        for json in [
            r#"{"schema_version":2,"version":1,"adb_serial":"127.0.0.1:15555","android_api_level":35,"multiwindow_supported":true}"#,
            r#"{"schema_version":1,"version":2,"adb_serial":"127.0.0.1:15555","android_api_level":35,"multiwindow_supported":true}"#,
            r#"{"schema_version":1,"version":1,"adb_serial":"emulator-5554","android_api_level":35,"multiwindow_supported":true}"#,
            r#"{"schema_version":1,"version":1,"adb_serial":"127.0.0.1:15555","android_api_level":29,"multiwindow_supported":true}"#,
            r#"{"schema_version":1,"version":1,"adb_serial":"127.0.0.1:15555","android_api_level":35,"multiwindow_supported":false}"#,
            r#"not-json"#,
        ] {
            assert!(
                parse_supported_capability_cache(json.as_bytes()).is_none(),
                "unexpected cache hit for {json}"
            );
        }
        assert!(build_supported_capability_cache(29, true).is_err());
        assert!(build_supported_capability_cache(35, false).is_err());
    }

    #[test]
    fn test_scrcpy_startup_uses_short_stability_window_without_early_ready() {
        assert_eq!(SCRCPY_STABILITY_GRACE, Duration::from_millis(25));
        assert_eq!(SCRCPY_STARTUP_POLL, Duration::from_millis(25));

        let observed_at = Instant::now();
        let observed = Some((25, observed_at));
        assert_eq!(
            stable_display_id(
                observed,
                observed_at + SCRCPY_STABILITY_GRACE - Duration::from_millis(1),
                SCRCPY_STABILITY_GRACE,
            ),
            None
        );
        assert_eq!(
            stable_display_id(
                observed,
                observed_at + SCRCPY_STABILITY_GRACE,
                SCRCPY_STABILITY_GRACE,
            ),
            Some(25)
        );
    }

    #[test]
    fn test_scrcpy_display_log_requires_a_visible_sdl_window() {
        let log = "[server] INFO: New display: 1280x720/240 (id=25)\nINFO: Renderer: direct3d\n";

        assert_eq!(scrcpy_ready_display_id(log, None, false), None);
        assert_eq!(scrcpy_ready_display_id(log, None, true), Some(25));
        assert_eq!(
            scrcpy_ready_display_id("INFO: Texture: 1280x720", Some(0), false),
            None
        );
        assert_eq!(
            scrcpy_ready_display_id("INFO: Texture: 1280x720", Some(0), true),
            Some(0)
        );
    }

    #[test]
    fn test_scrcpy_fatal_after_display_observation_still_fails_immediately() {
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target");
        fs::create_dir_all(&target).unwrap();
        let log_path = target.join(format!("scrcpy-fatal-{}.log", std::process::id()));
        fs::write(
            &log_path,
            "[server] INFO: New display: 1280x720/240 (id=25)\n[server] ERROR: Capture/encoding error",
        )
        .unwrap();
        let mut child = Command::new("cmd.exe")
            .args(["/D", "/C", "ping -n 3 127.0.0.1 >nul"])
            .spawn()
            .unwrap();

        let error = wait_for_scrcpy_startup(&mut child, &log_path, Duration::from_secs(2))
            .unwrap_err()
            .to_string();

        assert!(error.contains("video pipeline failed"));
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(log_path);
    }

    #[test]
    fn test_app_window_contract_marks_per_app_audio_unimplemented() {
        assert_eq!(PER_APP_AUDIO_MODE, "disabled-per-app-not-implemented");
    }

    #[test]
    fn test_app_window_report_serializes_schema_version_two() {
        let report = AppWindowReport {
            schema_version: APP_WINDOW_SCHEMA_VERSION,
            package: "com.example.reader".to_string(),
            title: "Reader".to_string(),
            icon_path: None,
            window_pid: None,
            window_reused: false,
            android_api_level: 28,
            required_android_api_level: 30,
            display_id: None,
            display_status: "unsupported".to_string(),
            fallback_mode: "none".to_string(),
            audio_mode: PER_APP_AUDIO_MODE.to_string(),
            scrcpy_path: PathBuf::from(r"D:\vibecoding\sdk\scrcpy\scrcpy.exe"),
            adb_path: PathBuf::from(r"D:\vibecoding\sdk\android\platform-tools\adb.exe"),
            command_line: Vec::new(),
            super_resolution_scale_percent: None,
            presentation_fps: None,
            vertical_sync: true,
            dynamic_frame_rate: false,
            dynamic_low_frame_rate: 15,
            metadata_path: None,
            log_path: None,
            runtime_started: false,
        };

        let json = serde_json::to_value(report).unwrap();

        assert_eq!(json["schema_version"], APP_WINDOW_SCHEMA_VERSION);
    }

    #[test]
    fn test_app_window_metadata_paths_isolate_pid_and_package_locks() {
        let root = Path::new(r"D:\runtime\app-windows");
        assert_eq!(window_metadata_path(root, 101), root.join("101.json"));
        assert_ne!(
            window_metadata_path(root, 101),
            window_metadata_path(root, 202)
        );
        assert_ne!(
            package_lock_path(root, "com.example.reader").unwrap(),
            package_lock_path(root, "com.example.player").unwrap()
        );
    }

    #[test]
    fn test_scrcpy_instant_exit_is_an_error() {
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target");
        std::fs::create_dir_all(&target).unwrap();
        let log_path = target.join(format!("scrcpy-exit-{}.log", std::process::id()));
        std::fs::write(&log_path, "synthetic scrcpy startup failure").unwrap();
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "exit", "23"])
            .spawn()
            .unwrap();

        let error = wait_for_scrcpy_startup(&mut child, &log_path, Duration::from_secs(2))
            .unwrap_err()
            .to_string();

        assert!(error.contains("exited during startup"));
        assert!(error.contains("synthetic scrcpy startup failure"));
        let _ = std::fs::remove_file(log_path);
    }

    #[test]
    fn test_scrcpy_startup_timeout_is_an_error_and_terminates_the_child() {
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target");
        std::fs::create_dir_all(&target).unwrap();
        let log_path = target.join(format!("scrcpy-timeout-{}.log", std::process::id()));
        std::fs::write(
            &log_path,
            "[server] INFO: New display: 1280x720/240 (id=25)",
        )
        .unwrap();
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "for /L %i in (1,1,2147483647) do @set X=%i"])
            .spawn()
            .unwrap();

        let error = wait_for_scrcpy_startup(&mut child, &log_path, Duration::from_millis(100))
            .unwrap_err()
            .to_string();

        assert!(error.contains("visible scrcpy SDL window"));
        assert!(child.try_wait().unwrap().is_some());
        let _ = std::fs::remove_file(log_path);
    }
}
