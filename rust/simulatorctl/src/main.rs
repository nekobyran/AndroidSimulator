mod adb;
mod app_window;
mod bliss;
mod integration;
mod process_priority;
mod runtime_settings;
mod sdk;
mod shortcut;
mod startup_lock;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const ANIMEKO_VERSION: &str = "v5.6.0";
const ANIMEKO_APK: &str = "ani-5.6.0-x86_64.apk";
const ANIMEKO_SHA1: &str = "ani-5.6.0-x86_64.apk.sha1";
const OWNED_INSTANCE: &str = "android-simulator";
const OWNED_IMAGE_ID: &str = bliss::IMAGE_ID;
const OWNED_BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const OWNED_START_LOCK_TIMEOUT: Duration = Duration::from_secs(210);
const OWNED_QEMU_STARTUP_GRACE: Duration = Duration::from_secs(5);
const OWNED_QEMU_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(test)]
const WINDOWS_BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
const WINDOWS_NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
const WINDOWS_BACKGROUND_CREATION_FLAGS: u32 =
    WINDOWS_CREATE_NO_WINDOW | WINDOWS_NORMAL_PRIORITY_CLASS;
const GITHUB_RELEASE_BASE: &str = "https://github.com/open-ani/animeko/releases/download";
const OWNED_SDK_ROOT: &str = r"D:\vibecoding\sdk";
const OWNED_QEMU_PACKAGE: &str = "mingw-w64-ucrt-x86_64-qemu";
const OWNED_ADB_HOST_PORT: u16 = 15555;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstalledAppIdentity {
    package: String,
    name: String,
    icon_path: Option<PathBuf>,
}

#[derive(Parser)]
#[command(name = "simulatorctl")]
#[command(about = "Android Simulator control plane")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },
    Animeko {
        #[command(subcommand)]
        command: AnimekoCommand,
    },
    Owned {
        #[command(subcommand)]
        command: OwnedCommand,
    },
    Apk {
        #[command(subcommand)]
        command: ApkCommand,
    },
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    Shortcut {
        #[command(subcommand)]
        command: ShortcutCommand,
    },
    Integration {
        #[command(subcommand)]
        command: IntegrationCommand,
    },
    Input {
        #[command(subcommand)]
        command: InputCommand,
    },
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
}

#[derive(Subcommand)]
enum EnvCommand {
    Doctor,
}

#[derive(Subcommand)]
enum AnimekoCommand {
    Download(DownloadArgs),
}

#[derive(Args)]
struct DownloadArgs {
    #[arg(long, default_value = ANIMEKO_VERSION)]
    version: String,
}

#[derive(Subcommand)]
enum OwnedCommand {
    Status(OwnedArgs),
    Provision(OwnedArgs),
    Bootstrap(OwnedArgs),
    Ensure(OwnedArgs),
    ImageCheck(OwnedArgs),
    LaunchPlan(OwnedArgs),
    Launch(OwnedArgs),
}

#[derive(Args)]
struct OwnedArgs {
    #[arg(long, default_value = OWNED_INSTANCE)]
    instance: String,
    #[arg(long, default_value = OWNED_IMAGE_ID)]
    image: String,
}

#[derive(Subcommand)]
enum ApkCommand {
    Install(InstallArgs),
    ShellIcon(ShellIconArgs),
}

#[derive(Args)]
struct InstallArgs {
    #[arg(long)]
    apk: Option<PathBuf>,
    #[arg(long)]
    package: Option<String>,
    #[arg(long, default_value_t = false)]
    launch: bool,
    #[arg(long)]
    title: Option<String>,
}

#[derive(Args)]
struct ShellIconArgs {
    #[arg(long)]
    apk: PathBuf,
}

#[derive(Subcommand)]
enum AppCommand {
    List(ListArgs),
    Launch(LaunchArgs),
    Stop(StopArgs),
    Rename(RenameArgs),
    Uninstall(UninstallArgs),
}

#[derive(Args)]
struct ListArgs {
    /// Start the owned Android runtime when it is currently offline.
    #[arg(long, default_value_t = false)]
    start: bool,
}

#[derive(Args)]
struct LaunchArgs {
    #[arg(long)]
    package: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    icon: Option<PathBuf>,
}

#[derive(Args)]
struct StopArgs {
    #[arg(long)]
    package: String,
}

#[derive(Args)]
struct RenameArgs {
    #[arg(long)]
    package: String,
    #[arg(long)]
    name: String,
}

#[derive(Args)]
struct UninstallArgs {
    #[arg(long)]
    package: String,
}

#[derive(Subcommand)]
enum ShortcutCommand {
    Create(ShortcutArgs),
}

#[derive(Subcommand)]
enum IntegrationCommand {
    RegisterApk(IntegrationArgs),
    Status(IntegrationArgs),
}

#[derive(Args)]
struct IntegrationArgs {
    #[arg(long)]
    launcher: PathBuf,
}

#[derive(Args)]
struct ShortcutArgs {
    #[arg(long)]
    package: String,
    #[arg(long)]
    name: String,
    #[arg(long)]
    launcher: PathBuf,
    #[arg(long)]
    icon: Option<PathBuf>,
}

#[derive(Subcommand)]
enum InputCommand {
    Tap(TapArgs),
    Keyevent(KeyeventArgs),
    Swipe(SwipeArgs),
}

#[derive(Subcommand)]
enum SettingsCommand {
    Show,
    Set(SettingsSetArgs),
    Reset,
}

#[derive(Args, Debug)]
struct SettingsSetArgs {
    #[arg(long)]
    performance_mode: Option<runtime_settings::PerformanceMode>,
    #[arg(long)]
    cpu_cores: Option<u8>,
    #[arg(long)]
    memory_mb: Option<u32>,
    #[arg(long)]
    renderer_mode: Option<runtime_settings::RendererMode>,
    #[arg(long)]
    graphics_strategy: Option<runtime_settings::GraphicsStrategy>,
    #[arg(long)]
    resolution_mode: Option<runtime_settings::ResolutionMode>,
    #[arg(long)]
    custom_width: Option<u32>,
    #[arg(long)]
    custom_height: Option<u32>,
    #[arg(long)]
    custom_dpi: Option<u32>,
    #[arg(long)]
    max_frame_rate: Option<u32>,
    #[arg(long)]
    dynamic_frame_rate: Option<bool>,
    #[arg(long)]
    dynamic_low_frame_rate: Option<u32>,
    #[arg(long)]
    vertical_sync: Option<bool>,
    #[arg(long)]
    super_resolution: Option<bool>,
    #[arg(long)]
    super_resolution_scale_percent: Option<u32>,
    #[arg(long)]
    frame_interpolation: Option<bool>,
    #[arg(long)]
    system_audio: Option<bool>,
    #[arg(long)]
    keep_app_alive: Option<bool>,
    #[arg(long)]
    remember_window_position: Option<bool>,
    #[arg(long)]
    fixed_window_size: Option<bool>,
    #[arg(long)]
    auto_rotate: Option<bool>,
    #[arg(long)]
    quit_confirm: Option<bool>,
}

#[derive(Args)]
struct TapArgs {
    #[arg(long)]
    x: u32,
    #[arg(long)]
    y: u32,
    #[arg(long)]
    display_id: Option<u32>,
}

#[derive(Args)]
struct KeyeventArgs {
    #[arg(long)]
    key: String,
    #[arg(long)]
    display_id: Option<u32>,
}

#[derive(Args)]
struct SwipeArgs {
    #[arg(long)]
    x1: u32,
    #[arg(long)]
    y1: u32,
    #[arg(long)]
    x2: u32,
    #[arg(long)]
    y2: u32,
    #[arg(long, default_value_t = 300)]
    duration_ms: u64,
    #[arg(long)]
    display_id: Option<u32>,
}

#[derive(Serialize)]
struct CommandResult<T: Serialize> {
    ok: bool,
    status: &'static str,
    message: String,
    data: T,
}

#[derive(Serialize)]
struct DoctorReport {
    sdk_root: Option<PathBuf>,
    data_root: PathBuf,
    adb: sdk::ToolStatus,
    apkanalyzer: sdk::ToolStatus,
    windows_features: Vec<FeatureStatus>,
    hypervisor_ready: bool,
    devices: Vec<String>,
    owned_runtime: OwnedRuntimeReport,
}

#[derive(Clone, Serialize)]
struct FeatureStatus {
    name: String,
    state: String,
}

#[derive(Serialize)]
struct RuntimeProfile {
    backend: String,
    instance_id: String,
    image_id: String,
    image_format: String,
    cpu_model: String,
    cpu_cores: u8,
    memory_mb: u32,
    data_partition_mb: u32,
    gpu_backend: String,
    acceleration: String,
    adb_endpoint: String,
    snapshot: bool,
    root_expected: bool,
    owned_system_image: bool,
    os_name: String,
    os_version: String,
    android_api: u32,
    security_patch: String,
    control_plane: Vec<String>,
}

#[derive(Serialize)]
struct DownloadReport {
    version: String,
    apk_path: PathBuf,
    sha1_path: PathBuf,
    expected_sha1: String,
    actual_sha1: String,
    bytes: u64,
}

#[derive(Serialize)]
struct OwnedProvisionReport {
    schema_version: u8,
    toolchain_source: String,
    sdk_root: PathBuf,
    pacman_path: PathBuf,
    qemu_package: String,
    pacman_command: Vec<String>,
    qemu_path: PathBuf,
    qemu_img_path: PathBuf,
    qemu_data_root: PathBuf,
    qemu_version: String,
    qemu_img_version: String,
    image_download_path: PathBuf,
    image_path: PathBuf,
    image_sha256: String,
    boot_bundle: bliss::BootBundleReport,
    scrcpy: app_window::ScrcpyProvisionReport,
    runtime: OwnedRuntimeReport,
}

#[derive(Serialize)]
struct OwnedBootstrapReport {
    provision: OwnedProvisionReport,
    launch: OwnedLaunchReport,
    boot_completed: bool,
    elapsed_ms: u128,
}
#[derive(Serialize)]
struct OwnedRuntimeReport {
    runtime_root: PathBuf,
    sdk_root: PathBuf,
    toolchain_root: PathBuf,
    qemu_data_root: PathBuf,
    image_root: PathBuf,
    instance_root: PathBuf,
    profile_path: PathBuf,
    qemu_path: PathBuf,
    qemu_img_path: PathBuf,
    scrcpy: app_window::ScrcpyStatus,
    iso_path: PathBuf,
    kernel_path: PathBuf,
    initrd_path: PathBuf,
    image_manifest_path: PathBuf,
    userdata_path: PathBuf,
    profile_exists: bool,
    qemu_available: bool,
    qemu_img_available: bool,
    qemu_data_available: bool,
    mke2fs_available: bool,
    whpx_ready: bool,
    image_ready: bool,
    image_mode: String,
    launch_ready: bool,
    blockers: Vec<String>,
    profile: RuntimeProfile,
}

#[derive(Serialize)]
struct OwnedEnsureReport {
    created_paths: Vec<PathBuf>,
    runtime: OwnedRuntimeReport,
}

#[derive(Serialize)]
struct OwnedLaunchPlan {
    runtime: OwnedRuntimeReport,
    command_line: Vec<String>,
}

#[derive(Serialize)]
struct OwnedLaunchReport {
    runtime: OwnedRuntimeReport,
    command_line: Vec<String>,
    pid: u32,
    log_path: PathBuf,
    userdata_created: bool,
    reused: bool,
}

#[derive(Serialize)]
struct OwnedStatusReport {
    backend: &'static str,
    instance_id: String,
    running: bool,
    pid: Option<u32>,
    presentation: &'static str,
}

#[derive(Serialize)]
struct InstallReport {
    apk_path: PathBuf,
    package: String,
    application_name: String,
    installed: bool,
    launched: bool,
    launch: Option<app_window::AppWindowReport>,
    elapsed_ms: u128,
    adb_output: String,
}

#[derive(Serialize)]
struct ShellIconReport {
    schema_version: u8,
    apk: PathBuf,
    icon_path: Option<PathBuf>,
    cached: bool,
}

fn main() {
    if let Err(error) = run() {
        let result = CommandResult {
            ok: false,
            status: "error",
            message: format!("{error:#}"),
            data: serde_json::json!({}),
        };
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Env {
            command: EnvCommand::Doctor,
        } => print_ok("ready", "Environment inspected.", doctor()?),
        Commands::Animeko {
            command: AnimekoCommand::Download(args),
        } => print_ok(
            "downloaded",
            "Animeko APK is present and verified.",
            download_animeko(&args.version)?,
        ),
        Commands::Owned {
            command: OwnedCommand::Status(args),
        } => {
            let report = owned_status(&args.instance, &args.image)?;
            let message = if report.running {
                "Owned headless Android backend is running."
            } else {
                "Owned headless Android backend is stopped."
            };
            let status = if report.running { "running" } else { "stopped" };
            print_ok(status, message, report)
        }
        Commands::Owned {
            command: OwnedCommand::Provision(args),
        } => print_ok(
            "provisioned",
            "Owned runtime artifacts are provisioned.",
            provision_owned_runtime(&args.instance, &args.image)?,
        ),
        Commands::Owned {
            command: OwnedCommand::Bootstrap(args),
        } => print_ok(
            "ready",
            "Owned Android runtime was provisioned, launched, and finished booting.",
            bootstrap_owned_runtime(&args.instance, &args.image)?,
        ),
        Commands::Owned {
            command: OwnedCommand::Ensure(args),
        } => {
            let report = ensure_owned_runtime(&args.instance, &args.image)?;
            if report.runtime.launch_ready {
                print_ok("ready", "Owned emulator runtime is ready.", report)
            } else {
                print_result(
                    false,
                    "blocked",
                    "Owned emulator runtime is not launch-ready.",
                    report,
                )
            }
        }
        Commands::Owned {
            command: OwnedCommand::ImageCheck(args),
        } => {
            let report = inspect_owned_runtime(&args.instance, &args.image)?;
            if report.image_ready {
                print_ok("ready", "Owned system image is present.", report)
            } else {
                print_result(false, "blocked", "Owned system image is missing.", report)
            }
        }
        Commands::Owned {
            command: OwnedCommand::LaunchPlan(args),
        } => {
            let report = owned_launch_plan(&args.instance, &args.image)?;
            if report.runtime.launch_ready {
                print_ok("ready", "Owned emulator launch plan is ready.", report)
            } else {
                print_result(
                    false,
                    "blocked",
                    "Owned emulator launch plan is blocked.",
                    report,
                )
            }
        }
        Commands::Owned {
            command: OwnedCommand::Launch(args),
        } => print_ok(
            "launched",
            "Owned headless Android backend process started.",
            owned_launch(&args.instance, &args.image)?,
        ),
        Commands::Apk {
            command: ApkCommand::Install(args),
        } => {
            let report = install_apk(args)?;
            if report
                .launch
                .as_ref()
                .is_some_and(|window| window.display_status == "unsupported")
            {
                print_result(
                    false,
                    "unsupported",
                    "APK installed, but per-app windows require an Android 11+ (API 30+) image; no whole-device window fallback is allowed.",
                    report,
                )
            } else if report
                .launch
                .as_ref()
                .is_some_and(|window| !window.display_ready())
            {
                print_result(
                    false,
                    "blocked",
                    "APK installed and scrcpy started, but the virtual display id is still pending.",
                    report,
                )
            } else {
                print_ok("installed", "APK install command finished.", report)
            }
        }
        Commands::Apk {
            command: ApkCommand::ShellIcon(args),
        } => {
            let report = resolve_shell_icon(args)?;
            if report.icon_path.is_some() {
                print_ok("ready", "APK shell icon is available.", report)
            } else {
                print_result(
                    false,
                    "missing",
                    "APK shell icon could not be extracted.",
                    report,
                )
            }
        }
        Commands::App {
            command: AppCommand::List(args),
        } => print_ok(
            "listed",
            "Launchable Android apps listed.",
            list_apps(args.start)?,
        ),
        Commands::App {
            command: AppCommand::Launch(args),
        } => {
            let report =
                launch_package(&args.package, args.title.as_deref(), args.icon.as_deref())?;
            if report.display_ready() {
                print_ok("launched", "Android app window opened.", report)
            } else if report.display_status == "unsupported" {
                print_result(
                    false,
                    "unsupported",
                    "Per-app windows require an Android 11+ (API 30+) image; no whole-device or QEMU-window fallback is allowed.",
                    report,
                )
            } else {
                print_result(
                    false,
                    "blocked",
                    "scrcpy is running, but its virtual display id is still pending.",
                    report,
                )
            }
        }
        Commands::App {
            command: AppCommand::Stop(args),
        } => print_ok(
            "stopped",
            "Android app session stopped.",
            stop_package(&args.package)?,
        ),
        Commands::App {
            command: AppCommand::Rename(args),
        } => print_ok(
            "renamed",
            "Application display name updated.",
            rename_installed_app(&args.package, &args.name)?,
        ),
        Commands::App {
            command: AppCommand::Uninstall(args),
        } => print_ok(
            "uninstalled",
            "Android application uninstalled.",
            uninstall_package(&args.package)?,
        ),

        Commands::Shortcut {
            command: ShortcutCommand::Create(args),
        } => print_ok(
            "created",
            "Desktop shortcut created.",
            create_app_shortcut(args)?,
        ),
        Commands::Integration {
            command: IntegrationCommand::RegisterApk(args),
        } => print_ok(
            "registered",
            "APK files now open with Android Simulator for the current user.",
            integration::register_apk_association(&args.launcher)?,
        ),
        Commands::Integration {
            command: IntegrationCommand::Status(args),
        } => {
            let report = integration::inspect_apk_association(&args.launcher)?;
            if report.registered {
                print_ok("ready", "APK file association is registered.", report)
            } else {
                print_result(
                    false,
                    "missing",
                    "APK file association is not registered.",
                    report,
                )
            }
        }
        Commands::Input {
            command: InputCommand::Tap(args),
        } => print_ok(
            "executed",
            "Tap input sent.",
            send_input(
                adb::InputAction::Tap {
                    x: args.x,
                    y: args.y,
                },
                args.display_id,
            )?,
        ),
        Commands::Input {
            command: InputCommand::Keyevent(args),
        } => print_ok(
            "executed",
            "Key event sent.",
            send_input(
                adb::InputAction::Keyevent { key: args.key },
                args.display_id,
            )?,
        ),
        Commands::Input {
            command: InputCommand::Swipe(args),
        } => print_ok(
            "executed",
            "Swipe input sent.",
            send_input(
                adb::InputAction::Swipe {
                    x1: args.x1,
                    y1: args.y1,
                    x2: args.x2,
                    y2: args.y2,
                    duration_ms: args.duration_ms,
                },
                args.display_id,
            )?,
        ),
        Commands::Settings {
            command: SettingsCommand::Show,
        } => print_ok(
            "ready",
            "Simulator settings loaded.",
            load_simulator_settings()?,
        ),
        Commands::Settings {
            command: SettingsCommand::Set(args),
        } => print_ok(
            "saved",
            "Simulator settings saved.",
            update_simulator_settings(args)?,
        ),
        Commands::Settings {
            command: SettingsCommand::Reset,
        } => print_ok(
            "reset",
            "Simulator settings reset to defaults.",
            reset_simulator_settings()?,
        ),
    }
}

fn print_ok<T: Serialize>(status: &'static str, message: &str, data: T) -> Result<()> {
    print_result(true, status, message, data)
}

fn print_result<T: Serialize>(
    ok: bool,
    status: &'static str,
    message: &str,
    data: T,
) -> Result<()> {
    let result = CommandResult {
        ok,
        status,
        message: message.to_string(),
        data,
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn simulator_settings_runtime_root() -> Result<PathBuf> {
    Ok(owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?.runtime_root)
}

fn load_simulator_settings() -> Result<runtime_settings::SimulatorSettings> {
    runtime_settings::load(&simulator_settings_runtime_root()?)
}

fn update_simulator_settings(args: SettingsSetArgs) -> Result<runtime_settings::SimulatorSettings> {
    let runtime_root = simulator_settings_runtime_root()?;
    let mut settings = runtime_settings::load(&runtime_root)?;
    if let Some(value) = args.performance_mode {
        settings.performance_mode = value;
    }
    if let Some(value) = args.cpu_cores {
        settings.cpu_cores = value;
    }
    if let Some(value) = args.memory_mb {
        settings.memory_mb = value;
    }
    if let Some(value) = args.renderer_mode {
        settings.renderer_mode = value;
    }
    if let Some(value) = args.graphics_strategy {
        settings.graphics_strategy = value;
    }
    if let Some(value) = args.resolution_mode {
        settings.resolution_mode = value;
    }
    if let Some(value) = args.custom_width {
        settings.custom_width = value;
    }
    if let Some(value) = args.custom_height {
        settings.custom_height = value;
    }
    if let Some(value) = args.custom_dpi {
        settings.custom_dpi = value;
    }
    if let Some(value) = args.max_frame_rate {
        settings.max_frame_rate = value;
    }
    if let Some(value) = args.dynamic_frame_rate {
        settings.dynamic_frame_rate = value;
    }
    if let Some(value) = args.dynamic_low_frame_rate {
        settings.dynamic_low_frame_rate = value;
    }
    if let Some(value) = args.vertical_sync {
        settings.vertical_sync = value;
    }
    if let Some(value) = args.super_resolution {
        settings.super_resolution = value;
    }
    if let Some(value) = args.super_resolution_scale_percent {
        settings.super_resolution_scale_percent = value;
    }
    if let Some(value) = args.frame_interpolation {
        settings.frame_interpolation = value;
    }
    if let Some(value) = args.system_audio {
        settings.system_audio = value;
    }
    if let Some(value) = args.keep_app_alive {
        settings.keep_app_alive = value;
    }
    if let Some(value) = args.remember_window_position {
        settings.remember_window_position = value;
    }
    if let Some(value) = args.fixed_window_size {
        settings.fixed_window_size = value;
    }
    if let Some(value) = args.auto_rotate {
        settings.auto_rotate = value;
    }
    if let Some(value) = args.quit_confirm {
        settings.quit_confirm = value;
    }
    runtime_settings::save(&runtime_root, &settings)?;
    persist_runtime_profile_and_scheduler(&settings)?;
    Ok(settings)
}

fn reset_simulator_settings() -> Result<runtime_settings::SimulatorSettings> {
    let runtime_root = simulator_settings_runtime_root()?;
    let settings = runtime_settings::SimulatorSettings::default();
    runtime_settings::save(&runtime_root, &settings)?;
    persist_runtime_profile_and_scheduler(&settings)?;
    Ok(settings)
}

fn persist_runtime_profile_and_scheduler(
    settings: &runtime_settings::SimulatorSettings,
) -> Result<()> {
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    if let Some(parent) = layout.profile_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let profile = default_runtime_profile(OWNED_INSTANCE, OWNED_IMAGE_ID, settings);
    fs::write(&layout.profile_path, serde_json::to_string_pretty(&profile)?)?;

    let adb_path = layout
        .sdk_root
        .join("android")
        .join("platform-tools")
        .join(exe("adb"));
    let live_windows = if adb_path.is_file() {
        app_window::refresh_live_app_window_priorities(
            &layout.runtime_root,
            &layout.sdk_root,
            &adb_path,
            settings.performance_mode,
        )?
    } else {
        0
    };

    if let Some(pid) = running_owned_pid(&layout.pid_path, &layout.qemu_path)? {
        if live_windows > 0 {
            process_priority::promote_latency_sensitive_process(pid, settings.performance_mode)?;
        } else {
            process_priority::demote_idle_process(pid)?;
        }
    }
    Ok(())
}

fn doctor() -> Result<DoctorReport> {
    let sdk_root = sdk::android_sdk_root();
    let adb = sdk::sdk_tool("platform-tools", &exe("adb"));
    let apkanalyzer = sdk::sdk_cmdline_tool("apkanalyzer");
    let windows_features = windows_feature_states();
    let hypervisor_ready = features_have_hypervisor(&windows_features)
        && hypervisor_boot_enabled()
        && hypervisor_present();
    let devices = command_lines(&adb.path, &["devices", "-l"]).unwrap_or_default();

    Ok(DoctorReport {
        sdk_root,
        data_root: data_root()?,
        adb,
        apkanalyzer,
        windows_features,
        hypervisor_ready,
        devices,
        owned_runtime: inspect_owned_runtime(OWNED_INSTANCE, OWNED_IMAGE_ID)?,
    })
}

fn default_runtime_profile(
    instance: &str,
    image: &str,
    settings: &runtime_settings::SimulatorSettings,
) -> RuntimeProfile {
    RuntimeProfile {
        backend: "owned-qemu-blissos".to_string(),
        instance_id: instance.to_string(),
        image_id: image.to_string(),
        image_format: "verified-blissos-iso-direct-kernel".to_string(),
        cpu_model: "Skylake-Client-v4".to_string(),
        cpu_cores: settings.effective_cpu_cores(),
        memory_mb: settings.effective_memory_mb(),
        data_partition_mb: 8192,
        gpu_backend: "virtio-vga-gl-egl-headless".to_string(),
        acceleration: "whpx-required".to_string(),
        adb_endpoint: adb::OWNED_ADB_SERIAL.to_string(),
        snapshot: false,
        root_expected: true,
        owned_system_image: true,
        os_name: "BlissOS Generic FOSS".to_string(),
        os_version: bliss::IMAGE_VERSION.to_string(),
        android_api: bliss::IMAGE_ANDROID_API,
        security_patch: bliss::IMAGE_SECURITY_PATCH.to_string(),
        control_plane: vec![
            "owned-qemu-binary".to_string(),
            "verified-third-party-blissos-image".to_string(),
            "direct-kernel-deterministic-boot".to_string(),
            "persistent-ext4-data".to_string(),
            "kernel-boot-properties".to_string(),
            "cpu-memory-disk".to_string(),
            "gpu-renderer".to_string(),
            "headless-android-backend".to_string(),
            "scrcpy-per-app-virtual-displays".to_string(),
            "persistent-raw-ext4-data".to_string(),
            "adb-root-remount-probe".to_string(),
            "no-official-avd-runtime".to_string(),
        ],
    }
}

fn download_animeko(version: &str) -> Result<DownloadReport> {
    let dir = data_root()?.join("downloads").join("animeko").join(version);
    fs::create_dir_all(&dir)?;
    let apk_path = dir.join(ANIMEKO_APK);
    let sha1_path = dir.join(ANIMEKO_SHA1);
    let base = format!("{GITHUB_RELEASE_BASE}/{version}");
    download_if_missing(&format!("{base}/{ANIMEKO_APK}"), &apk_path)?;
    download_if_missing(&format!("{base}/{ANIMEKO_SHA1}"), &sha1_path)?;

    let expected_sha1 = fs::read_to_string(&sha1_path)?
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("SHA1 file is empty"))?
        .to_ascii_lowercase();
    let actual_sha1 = sha1_file(&apk_path)?;
    if expected_sha1 != actual_sha1 {
        bail!("SHA1 mismatch for {}", apk_path.display());
    }

    Ok(DownloadReport {
        version: version.to_string(),
        bytes: fs::metadata(&apk_path)?.len(),
        apk_path,
        sha1_path,
        expected_sha1,
        actual_sha1,
    })
}

fn download_if_missing(url: &str, path: &Path) -> Result<()> {
    if path.exists() && fs::metadata(path)?.len() > 0 {
        return Ok(());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()?;
    let mut response = client
        .get(url)
        .header("User-Agent", "AndroidSimulator/0.1")
        .send()
        .with_context(|| format!("download failed: {url}"))?;
    if !response.status().is_success() {
        bail!("download failed with HTTP {}: {url}", response.status());
    }
    let tmp = path.with_extension("download");
    let mut file = fs::File::create(&tmp)?;
    response.copy_to(&mut file)?;
    file.flush()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn sha1_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn provision_owned_runtime(instance: &str, image: &str) -> Result<OwnedProvisionReport> {
    let _ = ensure_owned_runtime(instance, image)?;
    let layout = owned_layout(instance, image)?;
    let downloads = layout.runtime_root.join("downloads");
    fs::create_dir_all(&downloads)?;

    let scrcpy = app_window::provision_scrcpy(&layout.sdk_root)?;

    let pacman_command = owned_pacman_command(&layout);
    install_owned_qemu_package(&layout, &pacman_command)?;
    verify_owned_qemu_artifacts(&layout)?;
    let qemu_version = owned_qemu_version(&layout, &layout.qemu_path, "qemu-system-x86_64")?;
    let qemu_img_version = owned_qemu_version(&layout, &layout.qemu_img_path, "qemu-img")?;

    let image_download_path = downloads.join(bliss::IMAGE_FILE_NAME);
    download_if_missing(bliss::IMAGE_URL, &image_download_path)?;
    let actual_image_sha256 = bliss::sha256_file(&image_download_path)?;
    if actual_image_sha256 != bliss::IMAGE_SHA256 {
        bail!(
            "BlissOS ISO SHA256 mismatch for {}",
            image_download_path.display()
        );
    }
    install_verified_image_link(&image_download_path, &layout.iso_path)?;
    let boot_bundle = bliss::prepare_boot_bundle(&layout.iso_path, &layout.image_root)?;

    Ok(OwnedProvisionReport {
        schema_version: 3,
        toolchain_source: "msys2-ucrt64".to_string(),
        sdk_root: layout.sdk_root.clone(),
        pacman_path: layout.pacman_path.clone(),
        qemu_package: OWNED_QEMU_PACKAGE.to_string(),
        pacman_command: pacman_command.command_line(),
        qemu_path: layout.qemu_path.clone(),
        qemu_img_path: layout.qemu_img_path.clone(),
        qemu_data_root: layout.qemu_data_root.clone(),
        qemu_version,
        qemu_img_version,
        image_download_path,
        image_path: layout.iso_path,
        image_sha256: actual_image_sha256,
        boot_bundle,
        scrcpy,
        runtime: inspect_owned_runtime(instance, image)?,
    })
}

fn bootstrap_owned_runtime(instance: &str, image: &str) -> Result<OwnedBootstrapReport> {
    let started = Instant::now();
    let provision = provision_owned_runtime(instance, image)?;
    let launch = owned_launch(instance, image)?;
    let adb = require_owned_adb()?;
    adb::wait_for_owned_device(&adb, OWNED_BOOT_TIMEOUT)?;
    let layout = owned_layout(instance, image)?;
    fs::write(
        layout.instance_root.join("adb-ready.pid"),
        launch.pid.to_string(),
    )?;

    Ok(OwnedBootstrapReport {
        provision,
        launch,
        boot_completed: true,
        elapsed_ms: started.elapsed().as_millis(),
    })
}
fn install_verified_image_link(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        // Recreate even a valid legacy copy as a hard link so the verified
        // multi-gigabyte ISO consumes physical storage only once.
        fs::remove_file(destination)?;
    }

    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(hard_link_error) => fs::copy(source, destination)
            .map(|_| ())
            .with_context(|| {
                format!(
                    "failed to install verified BlissOS image as a hard link ({hard_link_error}) or copy: {}",
                    destination.display()
                )
            }),
    }
}

fn install_owned_qemu_package(layout: &OwnedLayout, command: &OwnedPacmanCommand) -> Result<()> {
    if !command.program.is_file() {
        bail!(
            "Missing central MSYS2 package manager: {}",
            command.program.display()
        );
    }
    let inherited_path = env::var_os("PATH");
    let environment = owned_pacman_environment_from_path(layout, inherited_path.as_deref())?;
    let output = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.current_dir)
        .env("PATH", environment.path)
        .output()
        .context("failed to install the owned QEMU package with central MSYS2 pacman")?;
    if !output.status.success() {
        bail!(
            "central MSYS2 pacman failed to install {}: {}{}",
            OWNED_QEMU_PACKAGE,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn verify_owned_qemu_artifacts(layout: &OwnedLayout) -> Result<()> {
    for (label, path) in [
        ("qemu-system-x86_64", &layout.qemu_path),
        ("qemu-img", &layout.qemu_img_path),
    ] {
        if !path.is_file() {
            bail!(
                "MSYS2 package {} did not provide {label} at {}",
                OWNED_QEMU_PACKAGE,
                path.display()
            );
        }
    }
    if !layout.qemu_data_root.is_dir() {
        bail!(
            "MSYS2 package {} did not provide the QEMU data directory at {}",
            OWNED_QEMU_PACKAGE,
            layout.qemu_data_root.display()
        );
    }
    Ok(())
}

fn owned_qemu_version(layout: &OwnedLayout, program: &Path, label: &str) -> Result<String> {
    let mut command = Command::new(program);
    command.arg("--version");
    configure_owned_qemu_command(&mut command, &layout.toolchain_root, &layout.qemu_data_root)?;
    let output = command
        .output()
        .with_context(|| format!("failed to inspect {label} version"))?;
    if !output.status.success() {
        bail!(
            "{label} --version failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let version = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| anyhow!("{label} --version returned no version text"))?
        .to_string();
    let normalized = version.to_ascii_lowercase();
    if !normalized.contains("qemu") || !normalized.contains("version") {
        bail!("{label} returned an unexpected version line: {version}");
    }
    Ok(version)
}

fn ensure_owned_runtime(instance: &str, image: &str) -> Result<OwnedEnsureReport> {
    let layout = owned_layout(instance, image)?;
    let mut created_paths = Vec::new();
    for path in [
        &layout.runtime_root,
        &layout.image_root,
        &layout.instance_root,
        &layout.instance_root.join("overlays"),
        &layout.instance_root.join("snapshots"),
        &layout.instance_root.join("logs"),
    ] {
        if !path.exists() {
            fs::create_dir_all(path)?;
            created_paths.push(path.to_path_buf());
        }
    }

    let settings = runtime_settings::load(&layout.runtime_root)?;
    let profile = default_runtime_profile(instance, image, &settings);
    fs::write(
        &layout.profile_path,
        serde_json::to_string_pretty(&profile)?,
    )?;

    Ok(OwnedEnsureReport {
        created_paths,
        runtime: inspect_owned_runtime(instance, image)?,
    })
}

fn inspect_owned_runtime(instance: &str, image: &str) -> Result<OwnedRuntimeReport> {
    let layout = owned_layout(instance, image)?;
    let settings = runtime_settings::load(&layout.runtime_root)?;
    let profile = default_runtime_profile(instance, image, &settings);
    let profile_exists = layout.profile_path.exists();
    let qemu_available = layout.qemu_path.exists();
    let qemu_img_available = layout.qemu_img_path.exists();
    let qemu_data_available = layout.qemu_data_root.is_dir();
    let mke2fs_available = sdk::sdk_tool("platform-tools", &exe("mke2fs")).available;
    let scrcpy = app_window::inspect_scrcpy(&layout.sdk_root)?;
    let image_ready = layout.iso_path.is_file()
        && layout.kernel_path.is_file()
        && layout.initrd_path.is_file()
        && layout.image_manifest_path.is_file();
    let image_mode = if image_ready {
        "blissos-direct-kernel"
    } else {
        "missing"
    }
    .to_string();
    let whpx_ready = hypervisor_ready();

    let mut blockers = Vec::new();
    if !qemu_available {
        blockers.push(format!(
            "Missing owned QEMU binary: {}. Do not use Android SDK emulator.exe as fallback.",
            layout.qemu_path.display()
        ));
    }
    if !qemu_img_available {
        blockers.push(format!(
            "Missing owned qemu-img binary: {}.",
            layout.qemu_img_path.display()
        ));
    }
    if !qemu_data_available {
        blockers.push(format!(
            "Missing owned QEMU data directory: {}. Provision {} from central MSYS2 pacman.",
            layout.qemu_data_root.display(),
            OWNED_QEMU_PACKAGE
        ));
    }
    if !mke2fs_available && !layout.userdata_path.is_file() {
        blockers.push(
            "Missing mke2fs.exe in the central Android SDK; the persistent BlissOS data disk cannot be created."
                .to_string(),
        );
    }
    if !scrcpy.available {
        blockers.push(scrcpy.blocker.clone().unwrap_or_else(|| {
            format!(
                "Official scrcpy {} is missing under {}. Run owned provision.",
                app_window::SCRCPY_VERSION,
                scrcpy.install_root.display()
            )
        }));
    }
    if !image_ready {
        blockers.push(format!(
            "Missing verified BlissOS runtime bundle under {}. Run owned provision to verify the ISO and extract its direct-boot kernel/initrd.",
            layout.image_root.display(),
        ));
    }
    if !profile_exists {
        blockers.push(format!(
            "Missing owned runtime profile: {}. Run owned ensure to create it.",
            layout.profile_path.display()
        ));
    }
    if !whpx_ready {
        blockers.push(
                        "Windows hypervisor is not active for this boot. Enable Windows Hypervisor Platform/Hyper-V, set `bcdedit /set hypervisorlaunchtype auto`, then restart Windows. Owned runtime refuses slow TCG fallback."
                .to_string(),
        );
    }

    Ok(OwnedRuntimeReport {
        runtime_root: layout.runtime_root,
        sdk_root: layout.sdk_root,
        toolchain_root: layout.toolchain_root,
        qemu_data_root: layout.qemu_data_root,
        image_root: layout.image_root,
        instance_root: layout.instance_root,
        profile_path: layout.profile_path,
        qemu_path: layout.qemu_path,
        qemu_img_path: layout.qemu_img_path,
        scrcpy,
        iso_path: layout.iso_path,
        kernel_path: layout.kernel_path,
        initrd_path: layout.initrd_path,
        image_manifest_path: layout.image_manifest_path,
        userdata_path: layout.userdata_path,
        profile_exists,
        qemu_available,
        qemu_img_available,
        qemu_data_available,
        mke2fs_available,
        whpx_ready,
        image_ready,
        image_mode,
        launch_ready: blockers.is_empty(),
        blockers,
        profile,
    })
}

fn owned_launch_plan(instance: &str, image: &str) -> Result<OwnedLaunchPlan> {
    let runtime = inspect_owned_runtime(instance, image)?;
    let command_line = owned_qemu_command_line(&runtime);

    Ok(OwnedLaunchPlan {
        runtime,
        command_line,
    })
}

fn owned_qemu_command_line(runtime: &OwnedRuntimeReport) -> Vec<String> {
    vec![
        runtime.qemu_path.to_string_lossy().to_string(),
        "-name".to_string(),
        format!(
            "guest={},process=AndroidSimulator-{}",
            runtime.profile.instance_id, runtime.profile.instance_id
        ),
        "-accel".to_string(),
        "whpx".to_string(),
        "-machine".to_string(),
        "q35,vmport=off".to_string(),
        "-L".to_string(),
        runtime.qemu_data_root.to_string_lossy().to_string(),
        "-cpu".to_string(),
        runtime.profile.cpu_model.clone(),
        "-smp".to_string(),
        format!("sockets=1,cores={},threads=1", runtime.profile.cpu_cores),
        "-m".to_string(),
        runtime.profile.memory_mb.to_string(),
        "-rtc".to_string(),
        "base=localtime,clock=host,driftfix=slew".to_string(),
        "-no-reboot".to_string(),
        "-device".to_string(),
        // The whole-device display is never shown. Keep one small accelerated
        // scanout for Android/SystemUI compatibility so the headless guest does
        // not composite an unnecessary 1280x800 desktop at ~75 Hz while all
        // user-visible applications render on independent scrcpy displays.
        "virtio-vga-gl,xres=640,yres=480".to_string(),
        "-display".to_string(),
        "egl-headless,gl=on".to_string(),
        "-monitor".to_string(),
        "none".to_string(),
        "-serial".to_string(),
        "none".to_string(),
        "-device".to_string(),
        "virtio-rng-pci".to_string(),
        "-netdev".to_string(),
        format!(
            "user,id=net0,ipv6=off,hostfwd=tcp:127.0.0.1:{OWNED_ADB_HOST_PORT}-:5555"
        ),
        "-device".to_string(),
        "virtio-net-pci,netdev=net0".to_string(),
        "-kernel".to_string(),
        runtime.kernel_path.to_string_lossy().to_string(),
        "-initrd".to_string(),
        runtime.initrd_path.to_string_lossy().to_string(),
        "-append".to_string(),
        "root=/dev/ram0 DATA=vda SETUPWIZARD=0 HWC=drm_minigbm GRALLOC=minigbm_arcvm FFMPEG_CODEC=1 FFMPEG_PREFER_C2=1 qemu=1 androidboot.qemu=1 androidboot.selinux=permissive".to_string(),
        "-cdrom".to_string(),
        runtime.iso_path.to_string_lossy().to_string(),
        "-drive".to_string(),
        format!(
            "file={},if=virtio,format=raw,cache=writeback,aio=threads,discard=unmap,detect-zeroes=unmap",
            runtime.userdata_path.display()
        ),
    ]
}

fn owned_launch(instance: &str, image: &str) -> Result<OwnedLaunchReport> {
    let runtime = inspect_owned_runtime(instance, image)?;
    if !runtime.launch_ready {
        bail!(
            "Owned emulator runtime is not launch-ready: {}",
            runtime.blockers.join("; ")
        );
    }

    let layout = owned_layout(instance, image)?;
    let settings = runtime_settings::load(&layout.runtime_root)?;
    let _lock =
        startup_lock::StartupLock::acquire(&layout.startup_lock_path, OWNED_START_LOCK_TIMEOUT)?;
    if let Some(pid) = running_owned_pid(&layout.pid_path, &runtime.qemu_path)? {
        process_priority::promote_latency_sensitive_process(pid, settings.performance_mode)?;
        let command_line = owned_qemu_command_line(&runtime);
        let log_path = latest_qemu_log(&runtime.instance_root)
            .unwrap_or_else(|| runtime.instance_root.join("logs").join("qemu-existing.log"));
        return Ok(OwnedLaunchReport {
            runtime,
            command_line,
            pid,
            log_path,
            userdata_created: false,
            reused: true,
        });
    }
    start_owned_qemu(runtime, &layout.pid_path)
}

fn owned_status(instance: &str, image: &str) -> Result<OwnedStatusReport> {
    let layout = owned_layout(instance, image)?;
    let pid = running_owned_pid(&layout.pid_path, &layout.qemu_path)?;
    Ok(OwnedStatusReport {
        backend: "owned-qemu-blissos",
        instance_id: instance.to_string(),
        running: pid.is_some(),
        pid,
        presentation: "headless",
    })
}

fn start_owned_qemu(runtime: OwnedRuntimeReport, pid_path: &Path) -> Result<OwnedLaunchReport> {
    let settings = runtime_settings::load(&runtime.runtime_root)?;
    let userdata_created = ensure_userdata_disk(&runtime)?;
    let command_line = owned_qemu_command_line(&runtime);
    let log_path = runtime
        .instance_root
        .join("logs")
        .join(format!("qemu-{}.log", unix_millis()?));
    let log_file = fs::File::create(&log_path)?;
    let mut command = Command::new(&command_line[0]);
    command.args(&command_line[1..]);
    configure_owned_qemu_command(
        &mut command,
        &runtime.toolchain_root,
        &runtime.qemu_data_root,
    )?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("failed to start owned QEMU")?;
    if let Err(error) = fs::write(pid_path, child.id().to_string()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context("failed to persist owned QEMU PID");
    }

    if let Err(error) = wait_for_owned_qemu_startup(&mut child, &log_path) {
        let _ = fs::remove_file(pid_path);
        return Err(error);
    }
    if let Err(error) =
        process_priority::promote_latency_sensitive_process(child.id(), settings.performance_mode)
    {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(pid_path);
        return Err(error).context("failed to pin owned QEMU latency priority");
    }

    Ok(OwnedLaunchReport {
        runtime,
        command_line,
        pid: child.id(),
        log_path,
        userdata_created,
        reused: false,
    })
}

fn wait_for_owned_qemu_startup(child: &mut std::process::Child, log_path: &Path) -> Result<()> {
    let started_at = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let log_text = fs::read_to_string(log_path).unwrap_or_default();
            bail!("owned QEMU exited during startup with {status}. Log: {log_text}");
        }

        let elapsed = started_at.elapsed();
        if elapsed >= OWNED_QEMU_STARTUP_GRACE {
            return Ok(());
        }
        thread::sleep(OWNED_QEMU_STARTUP_POLL_INTERVAL.min(OWNED_QEMU_STARTUP_GRACE - elapsed));
    }
}

#[derive(Debug, PartialEq, Eq)]
enum OwnedPidResolution {
    Reuse(u32),
    Recover(u32),
    Missing,
    ForeignPortOwner(u32),
}

fn select_owned_runtime_pid(
    marker: Option<(u32, bool)>,
    listener: Option<(u32, bool)>,
) -> OwnedPidResolution {
    if let Some((pid, true)) = marker {
        return OwnedPidResolution::Reuse(pid);
    }
    match listener {
        Some((pid, true)) => OwnedPidResolution::Recover(pid),
        Some((pid, false)) => OwnedPidResolution::ForeignPortOwner(pid),
        None => OwnedPidResolution::Missing,
    }
}

fn running_owned_pid(pid_path: &Path, expected_qemu_path: &Path) -> Result<Option<u32>> {
    let marker_pid = fs::read_to_string(pid_path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .map(|pid| {
            app_window::process_matches_exact_executable(pid, expected_qemu_path)
                .map(|matches| (pid, matches))
        })
        .transpose()?;
    if let Some((pid, true)) = marker_pid {
        return Ok(Some(pid));
    }
    let listener_pid = owned_adb_listener_pid()?
        .map(|pid| {
            app_window::process_matches_exact_executable(pid, expected_qemu_path)
                .map(|matches| (pid, matches))
        })
        .transpose()?;

    match select_owned_runtime_pid(marker_pid, listener_pid) {
        OwnedPidResolution::Reuse(pid) => Ok(Some(pid)),
        OwnedPidResolution::Recover(pid) => {
            fs::write(pid_path, pid.to_string())
                .context("failed to recover the owned QEMU PID marker")?;
            Ok(Some(pid))
        }
        OwnedPidResolution::Missing => {
            let _ = fs::remove_file(pid_path);
            Ok(None)
        }
        OwnedPidResolution::ForeignPortOwner(pid) => {
            let _ = fs::remove_file(pid_path);
            bail!(
                "owned Android ADB port 127.0.0.1:{OWNED_ADB_HOST_PORT} is occupied by process {pid}, which is not the central owned QEMU"
            )
        }
    }
}

#[cfg(windows)]
fn owned_adb_listener_pid() -> Result<Option<u32>> {
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
            TCP_TABLE_OWNER_PID_LISTENER,
        },
        Networking::WinSock::AF_INET,
    };

    let mut byte_count = 0u32;
    let first_status = unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut byte_count,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if first_status != ERROR_INSUFFICIENT_BUFFER && first_status != 0 {
        bail!("failed to size the Windows TCP listener table: error {first_status}");
    }
    if byte_count < mem::size_of::<u32>() as u32 {
        return Ok(None);
    }

    let word_count = (byte_count as usize).div_ceil(mem::size_of::<u32>());
    let mut buffer = vec![0u32; word_count];
    let status = unsafe {
        GetExtendedTcpTable(
            buffer.as_mut_ptr().cast(),
            &mut byte_count,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if status != 0 {
        bail!("failed to read the Windows TCP listener table: error {status}");
    }

    let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
    let entry_count = unsafe { (*table).dwNumEntries as usize };
    let required_bytes = mem::size_of::<u32>()
        .checked_add(
            entry_count
                .checked_mul(mem::size_of::<MIB_TCPROW_OWNER_PID>())
                .ok_or_else(|| anyhow!("Windows TCP listener table size overflow"))?,
        )
        .ok_or_else(|| anyhow!("Windows TCP listener table size overflow"))?;
    if required_bytes > byte_count as usize {
        bail!("Windows TCP listener table returned a truncated payload");
    }

    let first_row = unsafe { ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>() };
    let rows = unsafe { std::slice::from_raw_parts(first_row, entry_count) };
    Ok(rows
        .iter()
        .find(|row| u16::from_be(row.dwLocalPort as u16) == OWNED_ADB_HOST_PORT)
        .map(|row| row.dwOwningPid))
}

#[cfg(not(windows))]
fn owned_adb_listener_pid() -> Result<Option<u32>> {
    Ok(None)
}

fn latest_qemu_log(instance_root: &Path) -> Option<PathBuf> {
    let mut logs = fs::read_dir(instance_root.join("logs"))
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("qemu-") && name.ends_with(".log"))
        })
        .collect::<Vec<_>>();
    logs.sort_by_key(|path| fs::metadata(path).and_then(|item| item.modified()).ok());
    logs.pop()
}

fn ensure_userdata_disk(runtime: &OwnedRuntimeReport) -> Result<bool> {
    if runtime.userdata_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = runtime.userdata_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let size = format!("{}M", runtime.profile.data_partition_mb);
    let mut command = Command::new(&runtime.qemu_img_path);
    command
        .args(["create", "-f", "raw"])
        .arg(&runtime.userdata_path)
        .arg(size);
    configure_owned_qemu_command(
        &mut command,
        &runtime.toolchain_root,
        &runtime.qemu_data_root,
    )?;
    let output = command
        .output()
        .context("failed to create owned userdata disk")?;
    if !output.status.success() {
        bail!(
            "qemu-img create failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mke2fs = sdk::require_tool(
        sdk::sdk_tool("platform-tools", &exe("mke2fs")),
        "mke2fs in the D-drive Android SDK",
    )?;
    let mut format_command = Command::new(mke2fs);
    format_command
        .args(["-t", "ext4", "-L", "data", "-m", "0", "-F"])
        .arg(&runtime.userdata_path);
    configure_background_process(&mut format_command);
    let format_output = format_command
        .output()
        .context("failed to format the persistent BlissOS data image")?;
    if !format_output.status.success() {
        let _ = fs::remove_file(&runtime.userdata_path);
        bail!(
            "mke2fs failed: {}{}",
            String::from_utf8_lossy(&format_output.stdout),
            String::from_utf8_lossy(&format_output.stderr)
        );
    }
    Ok(true)
}

struct OwnedLayout {
    runtime_root: PathBuf,
    sdk_root: PathBuf,
    msys2_root: PathBuf,
    pacman_path: PathBuf,
    toolchain_root: PathBuf,
    qemu_data_root: PathBuf,
    image_root: PathBuf,
    instance_root: PathBuf,
    profile_path: PathBuf,
    qemu_path: PathBuf,
    qemu_img_path: PathBuf,
    iso_path: PathBuf,
    kernel_path: PathBuf,
    initrd_path: PathBuf,
    image_manifest_path: PathBuf,
    userdata_path: PathBuf,
    startup_lock_path: PathBuf,
    pid_path: PathBuf,
}

#[derive(Debug, Eq, PartialEq)]
struct OwnedPacmanCommand {
    program: PathBuf,
    args: Vec<String>,
    current_dir: PathBuf,
}

impl OwnedPacmanCommand {
    fn command_line(&self) -> Vec<String> {
        std::iter::once(self.program.to_string_lossy().to_string())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Eq, PartialEq)]
struct OwnedQemuEnvironment {
    current_dir: PathBuf,
    path: OsString,
    qemu_data_dir: PathBuf,
}

#[derive(Debug, Eq, PartialEq)]
struct OwnedPacmanEnvironment {
    path: OsString,
}

fn owned_layout(instance: &str, image: &str) -> Result<OwnedLayout> {
    let sdk_root = canonical_owned_sdk_root()?;
    let runtime_root = sdk_root.join("android-simulator-runtime");
    owned_layout_from_roots(&runtime_root, &sdk_root, instance, image)
}

fn owned_layout_from_roots(
    runtime_root: &Path,
    sdk_root: &Path,
    instance: &str,
    image: &str,
) -> Result<OwnedLayout> {
    validate_id(instance, "instance")?;
    validate_id(image, "image")?;
    let sdk_root = validate_owned_sdk_root(sdk_root)?;
    let msys2_root = sdk_root.join("msys64");
    let pacman_path = msys2_root.join("usr").join("bin").join(exe("pacman"));
    let ucrt64_root = msys2_root.join("ucrt64");
    let toolchain_root = ucrt64_root.join("bin");
    let qemu_data_root = ucrt64_root.join("share").join("qemu");
    let image_root = runtime_root.join("images").join(image);
    let instance_root = runtime_root.join("instances").join(instance);
    let profile_path = instance_root.join("profile.json");
    let qemu_path = toolchain_root.join(exe("qemu-system-x86_64"));
    let qemu_img_path = toolchain_root.join(exe("qemu-img"));
    let iso_path = image_root.join(bliss::IMAGE_FILE_NAME);
    let kernel_path = image_root.join(bliss::KERNEL_FILE_NAME);
    let initrd_path = image_root.join(bliss::PATCHED_INITRD_FILE_NAME);
    let image_manifest_path = image_root.join("runtime-image.json");
    let userdata_path = instance_root.join("overlays").join("data.img");
    let startup_lock_path = instance_root.join("startup.lock");
    let pid_path = instance_root.join("qemu.pid");
    Ok(OwnedLayout {
        runtime_root: runtime_root.to_path_buf(),
        sdk_root,
        msys2_root,
        pacman_path,
        toolchain_root,
        qemu_data_root,
        image_root,
        instance_root,
        profile_path,
        qemu_path,
        qemu_img_path,
        iso_path,
        kernel_path,
        initrd_path,
        image_manifest_path,
        userdata_path,
        startup_lock_path,
        pid_path,
    })
}

fn canonical_owned_sdk_root() -> Result<PathBuf> {
    let configured = Path::new(OWNED_SDK_ROOT);
    let canonical = fs::canonicalize(configured).with_context(|| {
        format!(
            "central SDK root is unavailable or cannot be normalized: {}",
            configured.display()
        )
    })?;
    validate_owned_sdk_root(&canonical)
}

fn validate_owned_sdk_root(candidate: &Path) -> Result<PathBuf> {
    let candidate_text = candidate.to_string_lossy().replace('/', "\\");
    if candidate_text
        .split('\\')
        .any(|component| matches!(component, "." | ".."))
    {
        bail!(
            "owned QEMU SDK root must already be normalized to {}",
            OWNED_SDK_ROOT
        );
    }
    let normalized = normalized_windows_path_key(&candidate_text);
    let allowed = normalized_windows_path_key(OWNED_SDK_ROOT);
    if !normalized.eq_ignore_ascii_case(&allowed) {
        bail!(
            "owned QEMU SDK root must resolve exactly to {}; got {}",
            OWNED_SDK_ROOT,
            candidate.display()
        );
    }
    Ok(PathBuf::from(OWNED_SDK_ROOT))
}

fn normalized_windows_path_key(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .unwrap_or(path)
        .trim_end_matches('\\')
        .to_string()
}

fn owned_pacman_command(layout: &OwnedLayout) -> OwnedPacmanCommand {
    OwnedPacmanCommand {
        program: layout.pacman_path.clone(),
        args: ["--needed", "--noconfirm", "-S", OWNED_QEMU_PACKAGE]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
        current_dir: layout.msys2_root.clone(),
    }
}

fn owned_pacman_environment_from_path(
    layout: &OwnedLayout,
    inherited_path: Option<&OsStr>,
) -> Result<OwnedPacmanEnvironment> {
    let usr_bin = layout
        .pacman_path
        .parent()
        .ok_or_else(|| anyhow!("central pacman path has no parent directory"))?;
    let path = joined_search_path([usr_bin, layout.toolchain_root.as_path()], inherited_path)
        .context("failed to build central MSYS2 pacman search path")?;
    Ok(OwnedPacmanEnvironment { path })
}

fn owned_qemu_environment_from_paths(
    toolchain_root: &Path,
    qemu_data_root: &Path,
    inherited_path: Option<&OsStr>,
) -> Result<OwnedQemuEnvironment> {
    let path = joined_search_path([toolchain_root], inherited_path)
        .context("failed to build owned QEMU DLL search path")?;
    Ok(OwnedQemuEnvironment {
        current_dir: toolchain_root.to_path_buf(),
        path,
        qemu_data_dir: qemu_data_root.to_path_buf(),
    })
}

fn joined_search_path<'a>(
    prefixes: impl IntoIterator<Item = &'a Path>,
    inherited_path: Option<&OsStr>,
) -> Result<OsString> {
    let mut path_entries = prefixes
        .into_iter()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    if let Some(path) = inherited_path {
        path_entries.extend(env::split_paths(path));
    }
    env::join_paths(path_entries).map_err(Into::into)
}

fn configure_owned_qemu_command(
    command: &mut Command,
    toolchain_root: &Path,
    qemu_data_root: &Path,
) -> Result<()> {
    let inherited_path = env::var_os("PATH");
    let environment = owned_qemu_environment_from_paths(
        toolchain_root,
        qemu_data_root,
        inherited_path.as_deref(),
    )?;
    command
        .current_dir(environment.current_dir)
        .env("PATH", environment.path)
        .env("QEMU_DATADIR", environment.qemu_data_dir);
    configure_background_process(command);
    Ok(())
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    // The guest performs rendering, input dispatch and host-network forwarding.
    // Force normal priority so efficiency/idle inheritance cannot turn touch
    // and network operations into multi-second stalls.
    command.creation_flags(WINDOWS_BACKGROUND_CREATION_FLAGS);
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

fn validate_id(value: &str, label: &str) -> Result<()> {
    let valid = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if value.is_empty() || !valid {
        bail!("{label} must contain only ASCII letters, numbers, dash, underscore, or dot");
    }
    Ok(())
}

fn resolve_shell_icon(args: ShellIconArgs) -> Result<ShellIconReport> {
    let apk_path = if args.apk.is_absolute() {
        args.apk
    } else {
        std::env::current_dir()?.join(args.apk)
    };
    if !apk_path.is_file() {
        bail!("APK not found: {}", apk_path.display());
    }
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let icon_path = sdk::cache_apk_shell_icon(&apk_path, &layout.runtime_root)?;
    Ok(ShellIconReport {
        schema_version: 1,
        apk: apk_path,
        cached: icon_path.is_some(),
        icon_path,
    })
}

fn install_apk(args: InstallArgs) -> Result<InstallReport> {
    let started = Instant::now();
    let apk_path = match args.apk {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()?.join(path),
        None => data_root()?
            .join("downloads")
            .join("animeko")
            .join(ANIMEKO_VERSION)
            .join(ANIMEKO_APK),
    };
    if !apk_path.is_file() {
        bail!("APK not found: {}", apk_path.display());
    }
    let apkanalyzer = sdk::require_tool(sdk::sdk_cmdline_tool("apkanalyzer"), "apkanalyzer")?;
    let package = match args.package {
        Some(package) => {
            adb::validate_package_id(&package)?;
            package
        }
        None => sdk::apk_package(&apkanalyzer, &apk_path)?,
    };
    let application_name = match args.title.as_deref() {
        Some(title) => app_window::resolve_window_title(&package, Some(title))?,
        None => match sdk::apk_label(&apkanalyzer, &apk_path)? {
            Some(label) => app_window::resolve_window_title(&package, Some(&label))?,
            None => app_window::resolve_window_title(&package, None)?,
        },
    };
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let icon_path = sdk::cache_apk_icon(&apkanalyzer, &apk_path, &package, &layout.runtime_root)
        .ok()
        .flatten();
    let _ = write_installed_app_identity(
        &layout.runtime_root,
        InstalledAppIdentity {
            package: package.clone(),
            name: application_name.clone(),
            icon_path: icon_path.clone(),
        },
    );
    let adb = require_owned_adb()?;
    let runtime_started = ensure_owned_runtime_for_adb(&adb)?;
    let output = Command::new(&adb)
        .args(["-s", adb::OWNED_ADB_SERIAL])
        .arg("install")
        .arg("-r")
        .arg(&apk_path)
        .output()
        .context("failed to start adb install")?;
    let adb_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() || adb_output.contains("Failure [") {
        bail!("adb install failed: {adb_output}");
    }
    let launch = if args.launch {
        Some(app_window::launch_app_window(
            &layout.runtime_root,
            &layout.sdk_root,
            &adb,
            &package,
            &application_name,
            icon_path.as_deref(),
            runtime_started,
        )?)
    } else {
        None
    };
    let launched = launch.is_some();
    Ok(InstallReport {
        apk_path,
        package,
        application_name,
        installed: true,
        launched,
        launch,
        elapsed_ms: started.elapsed().as_millis(),
        adb_output,
    })
}

fn list_apps(start_runtime: bool) -> Result<adb::AppListReport> {
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let settings = runtime_settings::load(&layout.runtime_root)?;
    let running_pid = running_owned_pid(&layout.pid_path, &layout.qemu_path)?;
    if !start_runtime && running_pid.is_none() {
        bail!("Owned Android runtime is offline. Start it before listing apps.");
    }
    if let Some(pid) = running_pid {
        process_priority::promote_latency_sensitive_process(pid, settings.performance_mode)
            .context("failed to wake the owned Android runtime before listing apps")?;
    }

    let adb = require_owned_adb()?;
    if start_runtime {
        let _ = ensure_owned_runtime_for_adb(&adb)?;
    } else if !adb::owned_device_booted(&adb)? {
        bail!("Owned Android runtime is offline. Start it before listing apps.");
    }

    let mut report = adb::list_apps(&adb)?;
    enrich_user_app_list(&layout.runtime_root, &mut report.apps);
    report
        .apps
        .retain(|app| is_desktop_usable_app(&app.package, &app.label));
    hydrate_missing_user_app_icons(&adb, &layout.runtime_root, &mut report.apps);
    report.count = report.apps.len();
    if !app_window::has_live_app_windows(&layout.runtime_root, &layout.sdk_root, &adb)?
        && let Some(pid) = running_owned_pid(&layout.pid_path, &layout.qemu_path)?
    {
        process_priority::demote_idle_process(pid)
            .context("failed to restore Android standby performance mode")?;
    }
    Ok(report)
}

fn is_desktop_usable_app(package: &str, label: &str) -> bool {
    let package = package.to_ascii_lowercase();
    let label = label.to_lowercase();
    let camera_package = package.contains("opencamera")
        || package.contains(".camera")
        || package.starts_with("camera.")
        || package.ends_with(".camera");
    let camera_label =
        label.contains("camera") || label.contains("相机") || label.contains("摄像机");
    !camera_package && !camera_label
}

fn enrich_user_app_list(runtime_root: &Path, apps: &mut [adb::AppInfo]) {
    for app in apps.iter_mut() {
        if let Some(identity) = read_installed_app_identity(runtime_root, &app.package) {
            if !identity.name.trim().is_empty() {
                app.label = identity.name;
            }
            if let Some(icon) = identity.icon_path.filter(|path| path.is_file()) {
                app.icon_path = Some(icon);
            }
        }

        if app.icon_path.is_none() {
            app.icon_path = sdk::cached_apk_icon(runtime_root, &app.package);
        }
    }
}

fn hydrate_missing_user_app_icons(adb_path: &Path, runtime_root: &Path, apps: &mut [adb::AppInfo]) {
    if apps.iter().all(|app| app.icon_path.is_some()) {
        return;
    }

    let temporary_root = runtime_root.join("cache").join("app-list-apks");
    if fs::create_dir_all(&temporary_root).is_err() {
        return;
    }
    let apkanalyzer = sdk::require_tool(
        sdk::sdk_cmdline_tool("apkanalyzer"),
        "Android SDK apkanalyzer",
    )
    .unwrap_or_default();

    for app in apps.iter_mut().filter(|app| app.icon_path.is_none()) {
        let Ok(remote_apk) = adb::installed_base_apk_path(adb_path, &app.package) else {
            continue;
        };
        let temporary_apk =
            temporary_root.join(format!(".{}-{}.apk", app.package, std::process::id()));
        let icon_path = if adb::pull_file(adb_path, &remote_apk, &temporary_apk).is_ok() {
            sdk::cache_apk_icon(&apkanalyzer, &temporary_apk, &app.package, runtime_root)
                .ok()
                .flatten()
        } else {
            None
        };
        let _ = fs::remove_file(&temporary_apk);

        if let Some(icon_path) = icon_path {
            apply_hydrated_user_app_icon(runtime_root, app, icon_path);
        }
    }
}

fn apply_hydrated_user_app_icon(
    runtime_root: &Path,
    app: &mut adb::AppInfo,
    icon_path: PathBuf,
) -> bool {
    if !icon_path.is_file() {
        return false;
    }
    app.icon_path = Some(icon_path.clone());
    let _ = write_installed_app_identity(
        runtime_root,
        InstalledAppIdentity {
            package: app.package.clone(),
            name: app.label.clone(),
            icon_path: Some(icon_path),
        },
    );
    true
}

fn launch_package(
    package: &str,
    title: Option<&str>,
    icon_override: Option<&Path>,
) -> Result<app_window::AppWindowReport> {
    adb::validate_package_id(package)?;
    let adb = require_owned_adb()?;
    let runtime_started = ensure_owned_runtime_for_adb(&adb)?;
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let identity = read_installed_app_identity(&layout.runtime_root, package);
    let icon_override = resolve_existing_icon(icon_override)?;
    let resolved_title = if icon_override.is_some() {
        title.or_else(|| identity.as_ref().map(|item| item.name.as_str()))
    } else {
        identity.as_ref().map(|item| item.name.as_str()).or(title)
    };
    let title = app_window::resolve_window_title(package, resolved_title)?;
    let raw_icon = icon_override
        .or_else(|| identity.and_then(|item| item.icon_path.filter(|path| path.is_file())))
        .or_else(|| sdk::cached_apk_icon(&layout.runtime_root, package));
    // Prefer an already-rendered compact desktop icon. Never re-hash/re-render
    // multi-megabyte APK dumps on the interactive launch path.
    let icon_path =
        raw_icon.map(|path| sdk::prefer_existing_desktop_rounded_icon(&path).unwrap_or(path));
    app_window::launch_app_window(
        &layout.runtime_root,
        &layout.sdk_root,
        &adb,
        package,
        &title,
        icon_path.as_deref(),
        runtime_started,
    )
}

fn stop_package(package: &str) -> Result<adb::AppStopReport> {
    adb::validate_package_id(package)?;
    let adb = require_owned_adb()?;
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let settings = runtime_settings::load(&layout.runtime_root)?;
    let running_pid = running_owned_pid(&layout.pid_path, &layout.qemu_path)?;
    if let Some(pid) = running_pid {
        process_priority::promote_latency_sensitive_process(pid, settings.performance_mode)
            .context("failed to wake the owned Android runtime before stopping an app")?;
    }
    let report =
        app_window::stop_app_session(&layout.runtime_root, &layout.sdk_root, &adb, package)?;
    if !app_window::has_live_app_windows(&layout.runtime_root, &layout.sdk_root, &adb)?
        && let Some(pid) = running_pid
    {
        process_priority::demote_idle_process(pid)
            .context("failed to enter Android standby performance mode")?;
    }
    Ok(report)
}

fn resolve_existing_icon(icon: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(icon) = icon else {
        return Ok(None);
    };
    let icon = if icon.is_absolute() {
        icon.to_path_buf()
    } else {
        std::env::current_dir()?.join(icon)
    };
    if !icon.is_file() {
        bail!("icon not found: {}", icon.display());
    }
    Ok(Some(icon))
}

fn create_app_shortcut(args: ShortcutArgs) -> Result<shortcut::ShortcutReport> {
    adb::validate_package_id(&args.package)?;
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let icon_override = resolve_existing_icon(args.icon.as_deref())?;
    let cached_identity = read_installed_app_identity(&layout.runtime_root, &args.package);
    let mut resolved_name = if icon_override.is_some() {
        args.name.trim().to_string()
    } else {
        cached_identity
            .as_ref()
            .map(|item| item.name.clone())
            .unwrap_or_else(|| args.name.trim().to_string())
    };
    let mut icon_path = icon_override.clone().or_else(|| {
        cached_identity
            .and_then(|item| item.icon_path.filter(|path| path.is_file()))
            .or_else(|| sdk::cached_apk_icon(&layout.runtime_root, &args.package))
    });

    // Desktop entries must use Android's resolved application label and icon,
    // not a package-derived guess or the generic launcher executable icon.
    // Inspection is best-effort so an offline runtime can still create a
    // functional shortcut using an already cached identity.
    if icon_override.is_none()
        && let Ok(adb_path) = require_owned_adb()
        && adb::owned_device_booted(&adb_path).unwrap_or(false)
        && let Ok(remote_apk) = adb::installed_base_apk_path(&adb_path, &args.package)
    {
        let cache_root = layout.runtime_root.join("cache").join("shortcut-apks");
        let temporary_apk =
            cache_root.join(format!(".{}-{}.apk", args.package, std::process::id()));
        if adb::pull_file(&adb_path, &remote_apk, &temporary_apk).is_ok()
            && let Ok(apkanalyzer) = sdk::require_tool(
                sdk::sdk_cmdline_tool("apkanalyzer"),
                "Android SDK apkanalyzer",
            )
        {
            if let Ok(Some(label)) = sdk::apk_label(&apkanalyzer, &temporary_apk) {
                resolved_name = label;
            }
            if let Ok(resolved_icon) = sdk::cache_apk_icon(
                &apkanalyzer,
                &temporary_apk,
                &args.package,
                &layout.runtime_root,
            ) {
                icon_path = resolved_icon.or(icon_path);
            }
        }
        let _ = fs::remove_file(&temporary_apk);
    }

    let _ = write_installed_app_identity(
        &layout.runtime_root,
        InstalledAppIdentity {
            package: args.package.clone(),
            name: resolved_name.clone(),
            icon_path: icon_path.clone(),
        },
    );

    // Keep the original Android artwork for the hosted window, while desktop
    // shortcuts receive an independent transparent rounded-corner derivative.
    let shortcut_icon_path = icon_path
        .as_deref()
        .map(|source| sdk::cache_desktop_rounded_icon(source, &layout.runtime_root))
        .transpose()
        .context("failed to prepare rounded desktop shortcut icon")?;

    shortcut::create_shortcut(
        &args.package,
        &resolved_name,
        &args.launcher,
        shortcut_icon_path.as_deref(),
    )
}

fn installed_app_identity_path(runtime_root: &Path, package: &str) -> PathBuf {
    runtime_root
        .join("app-identities")
        .join(format!("{package}.json"))
}

fn rename_installed_app(package: &str, name: &str) -> Result<InstalledAppIdentity> {
    adb::validate_package_id(package)?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        bail!("application name must contain between 1 and 80 characters");
    }
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let identity = InstalledAppIdentity {
        package: package.to_string(),
        name: name.to_string(),
        icon_path: read_installed_app_identity(&layout.runtime_root, package)
            .and_then(|identity| identity.icon_path),
    };
    write_installed_app_identity(&layout.runtime_root, identity.clone())?;
    Ok(identity)
}

fn uninstall_package(package: &str) -> Result<adb::AppUninstallReport> {
    adb::validate_package_id(package)?;
    let adb = require_owned_adb()?;
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let _ = app_window::stop_app_session(&layout.runtime_root, &layout.sdk_root, &adb, package);
    let report = adb::uninstall_package(&adb, package)?;
    let _ = fs::remove_file(installed_app_identity_path(&layout.runtime_root, package));
    Ok(report)
}

fn read_installed_app_identity(runtime_root: &Path, package: &str) -> Option<InstalledAppIdentity> {
    let bytes = fs::read(installed_app_identity_path(runtime_root, package)).ok()?;
    let identity = serde_json::from_slice::<InstalledAppIdentity>(&bytes).ok()?;
    (identity.package == package && !identity.name.trim().is_empty()).then_some(identity)
}

fn write_installed_app_identity(runtime_root: &Path, identity: InstalledAppIdentity) -> Result<()> {
    adb::validate_package_id(&identity.package)?;
    if identity.name.trim().is_empty() {
        bail!("installed application identity name cannot be empty");
    }
    let destination = installed_app_identity_path(runtime_root, &identity.package);
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("installed application identity path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = destination.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(&identity)?)?;
    fs::rename(&temporary, &destination).or_else(|error| {
        if destination.is_file() {
            fs::remove_file(&destination)?;
            fs::rename(&temporary, &destination)
        } else {
            Err(error)
        }
    })?;
    Ok(())
}

fn send_input(action: adb::InputAction, display_id: Option<u32>) -> Result<adb::InputReport> {
    let _ = adb::adb_input_args(&action, display_id)?;
    let adb = require_owned_adb()?;
    let _ = ensure_owned_runtime_for_adb(&adb)?;
    adb::execute_input(&adb, action, display_id)
}

fn require_owned_adb() -> Result<PathBuf> {
    sdk::require_tool(
        sdk::sdk_tool("platform-tools", &exe("adb")),
        "adb in the D-drive Android SDK",
    )
}

fn ensure_owned_runtime_for_adb(adb_path: &Path) -> Result<bool> {
    let layout = owned_layout(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    let settings = runtime_settings::load(&layout.runtime_root)?;
    let running_pid = running_owned_pid(&layout.pid_path, &layout.qemu_path)?;
    if let Some(pid) = running_pid {
        // Wake the guest before any ADB boot/connect probe. An inherited Idle
        // priority can otherwise make the localhost transport appear hung.
        process_priority::promote_latency_sensitive_process(pid, settings.performance_mode)
            .context("failed to wake the owned Android runtime before ADB access")?;
    }
    let ready_marker_path = layout.instance_root.join("adb-ready.pid");
    let ready_marker = fs::read_to_string(&ready_marker_path).ok();
    if owned_runtime_ready_marker_matches(ready_marker.as_deref(), running_pid) {
        // The QEMU PID marker outlives the ADB server and its TCP transports.
        // Re-probe the exact dedicated owned serial so `adb connect` can restore
        // it after a server restart or localhost transport drift.
        let owned_adb_booted = adb::owned_device_booted(adb_path)?;
        if owned_runtime_ready_marker_allows_fast_path(
            ready_marker.as_deref(),
            running_pid,
            owned_adb_booted,
        ) {
            return Ok(false);
        }
    } else if adb::owned_device_booted(adb_path)? {
        if let Some(pid) = running_pid {
            fs::write(&ready_marker_path, pid.to_string())?;
        }
        return Ok(false);
    }

    let _lock =
        startup_lock::StartupLock::acquire(&layout.startup_lock_path, OWNED_START_LOCK_TIMEOUT)?;
    let _ = ensure_owned_runtime(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
    if adb::owned_device_booted(adb_path)? {
        if let Some(pid) = running_owned_pid(&layout.pid_path, &layout.qemu_path)? {
            fs::write(&ready_marker_path, pid.to_string())?;
        }
        return Ok(false);
    }

    let mut runtime_started = false;
    if !adb::owned_device_online(adb_path)?
        && running_owned_pid(&layout.pid_path, &layout.qemu_path)?.is_none()
    {
        let runtime = inspect_owned_runtime(OWNED_INSTANCE, OWNED_IMAGE_ID)?;
        if !runtime.launch_ready {
            bail!(
                "Owned emulator runtime is not launch-ready: {}",
                runtime.blockers.join("; ")
            );
        }
        let _ = start_owned_qemu(runtime, &layout.pid_path)?;
        runtime_started = true;
    }

    adb::wait_for_owned_device(adb_path, OWNED_BOOT_TIMEOUT)?;
    if let Some(pid) = running_owned_pid(&layout.pid_path, &layout.qemu_path)? {
        fs::write(&ready_marker_path, pid.to_string())?;
    }
    Ok(runtime_started)
}

fn owned_runtime_ready_marker_matches(marker: Option<&str>, running_pid: Option<u32>) -> bool {
    marker
        .and_then(|value| value.trim().parse::<u32>().ok())
        .zip(running_pid)
        .is_some_and(|(marked_pid, current_pid)| marked_pid == current_pid)
}

fn owned_runtime_ready_marker_allows_fast_path(
    marker: Option<&str>,
    running_pid: Option<u32>,
    owned_adb_booted: bool,
) -> bool {
    owned_adb_booted && owned_runtime_ready_marker_matches(marker, running_pid)
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn windows_feature_states() -> Vec<FeatureStatus> {
    static STATES: std::sync::OnceLock<Vec<FeatureStatus>> = std::sync::OnceLock::new();
    STATES.get_or_init(query_windows_feature_states).clone()
}

fn query_windows_feature_states() -> Vec<FeatureStatus> {
    const FEATURES: [&str; 3] = [
        "HypervisorPlatform",
        "Microsoft-Hyper-V-Hypervisor",
        "VirtualMachinePlatform",
    ];
    FEATURES
        .iter()
        .map(|feature| FeatureStatus {
            name: feature.to_string(),
            state: query_feature_state_with_dism(feature).unwrap_or_else(|| "unknown".to_string()),
        })
        .collect()
}

fn hypervisor_ready() -> bool {
    features_have_hypervisor(&windows_feature_states())
        && hypervisor_boot_enabled()
        && hypervisor_present()
}

fn hypervisor_boot_enabled() -> bool {
    let output = Command::new("bcdedit.exe")
        .args(["/enum", "{current}"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            hypervisor_boot_enabled_from_bcd(&String::from_utf8_lossy(&output.stdout))
        }
        _ => false,
    }
}

fn hypervisor_present() -> bool {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[bool](Get-CimInstance Win32_ComputerSystem).HypervisorPresent",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => parse_windows_bool(&output.stdout),
        _ => false,
    }
}

fn parse_windows_bool(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes)
        .trim()
        .eq_ignore_ascii_case("true")
}

fn hypervisor_boot_enabled_from_bcd(text: &str) -> bool {
    let launch_type = text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        if key.eq_ignore_ascii_case("hypervisorlaunchtype") {
            parts.next()
        } else {
            None
        }
    });
    launch_type.is_some_and(|value| !value.eq_ignore_ascii_case("off"))
}

fn features_have_hypervisor(windows_features: &[FeatureStatus]) -> bool {
    windows_features.iter().any(|feature| {
        matches!(
            feature.name.as_str(),
            "HypervisorPlatform" | "Microsoft-Hyper-V-Hypervisor"
        ) && (feature.state.contains("已启用") || feature.state.contains("Enabled"))
    })
}

fn query_feature_state_with_dism(feature: &str) -> Option<String> {
    let argument = format!("/FeatureName:{feature}");
    let output = Command::new("dism.exe")
        .args(["/English", "/Online", "/Get-FeatureInfo", &argument])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("State :"))
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .map(ToString::to_string)
}

fn command_lines(tool: &Option<PathBuf>, args: &[&str]) -> Result<Vec<String>> {
    let path = tool.as_ref().ok_or_else(|| anyhow!("tool missing"))?;
    let output = Command::new(path).args(args).output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn unix_millis() -> Result<u128> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis())
}

fn data_root() -> Result<PathBuf> {
    let root = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("LOCALAPPDATA directory not found"))?
        .join("AndroidSimulator");
    fs::create_dir_all(&root)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_uses_simulatorctl_program_name() {
        assert_eq!(Cli::command().get_name(), "simulatorctl");
    }

    #[test]
    fn test_hypervisor_boot_parser_rejects_explicit_off_and_accepts_auto() {
        assert!(!hypervisor_boot_enabled_from_bcd(
            "identifier {current}\r\nhypervisorlaunchtype    Off\r\n"
        ));
        assert!(hypervisor_boot_enabled_from_bcd(
            "identifier {current}\r\nhypervisorlaunchtype    Auto\r\n"
        ));
        assert!(!hypervisor_boot_enabled_from_bcd(
            "identifier {current}\r\ndescription Windows 11\r\n"
        ));
    }

    #[test]
    fn test_windows_hypervisor_presence_parser_accepts_only_true() {
        assert!(parse_windows_bool(b"True\r\n"));
        assert!(parse_windows_bool(b" true \n"));
        assert!(!parse_windows_bool(b"False\r\n"));
        assert!(!parse_windows_bool(b""));
    }

    #[test]
    fn test_owned_layout_uses_only_central_msys2_ucrt64_toolchain() {
        let runtime_root = PathBuf::from(r"D:\runtime\AndroidSimulator");
        let layout = owned_layout_from_roots(
            &runtime_root,
            Path::new(r"D:\vibecoding\sdk"),
            "reader",
            "owned-image",
        )
        .unwrap();

        assert_eq!(
            layout.pacman_path,
            PathBuf::from(r"D:\vibecoding\sdk\msys64\usr\bin\pacman.exe")
        );
        assert_eq!(
            layout.toolchain_root,
            PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin")
        );
        assert_eq!(
            layout.qemu_path,
            PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin\qemu-system-x86_64.exe")
        );
        assert_eq!(
            layout.qemu_img_path,
            PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin\qemu-img.exe")
        );
        assert_eq!(
            layout.qemu_data_root,
            PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\share\qemu")
        );
    }

    #[test]
    fn test_owned_layout_rejects_any_other_or_non_normalized_sdk_root() {
        let runtime_root = PathBuf::from(r"D:\runtime\AndroidSimulator");

        assert!(
            owned_layout_from_roots(
                &runtime_root,
                Path::new(r"C:\vibecoding\sdk"),
                "reader",
                "owned-image"
            )
            .is_err()
        );
        assert!(
            owned_layout_from_roots(
                &runtime_root,
                Path::new(r"D:\vibecoding\sdk\..\sdk"),
                "reader",
                "owned-image"
            )
            .is_err()
        );
    }

    #[test]
    fn test_owned_pacman_command_has_one_current_qemu_source() {
        let runtime_root = PathBuf::from(r"D:\runtime\AndroidSimulator");
        let layout = owned_layout_from_roots(
            &runtime_root,
            Path::new(r"D:\vibecoding\sdk"),
            "reader",
            "owned-image",
        )
        .unwrap();

        let command = owned_pacman_command(&layout);

        assert_eq!(command.program, layout.pacman_path);
        assert_eq!(
            command.args,
            [
                "--needed",
                "--noconfirm",
                "-S",
                "mingw-w64-ucrt-x86_64-qemu"
            ]
        );
        assert_eq!(
            command.current_dir,
            PathBuf::from(r"D:\vibecoding\sdk\msys64")
        );

        let environment =
            owned_pacman_environment_from_path(&layout, Some(OsStr::new(r"C:\Windows\System32")))
                .unwrap();
        let path_entries = env::split_paths(&environment.path).collect::<Vec<_>>();
        assert_eq!(
            &path_entries[..2],
            [
                PathBuf::from(r"D:\vibecoding\sdk\msys64\usr\bin"),
                PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin")
            ]
        );
    }

    #[test]
    fn test_owned_qemu_environment_uses_ucrt64_bin_and_share() {
        let runtime_root = PathBuf::from(r"D:\runtime\AndroidSimulator");
        let layout = owned_layout_from_roots(
            &runtime_root,
            Path::new(r"D:\vibecoding\sdk"),
            "reader",
            "owned-image",
        )
        .unwrap();

        let environment = owned_qemu_environment_from_paths(
            &layout.toolchain_root,
            &layout.qemu_data_root,
            Some(OsStr::new(r"C:\Windows\System32")),
        )
        .unwrap();

        assert_eq!(environment.current_dir, layout.toolchain_root);
        assert_eq!(environment.qemu_data_dir, layout.qemu_data_root);
        assert_eq!(
            std::env::split_paths(&environment.path).next(),
            Some(PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin"))
        );
    }

    #[test]
    fn test_owned_qemu_command_line_is_named_accelerated_and_never_tcg() {
        let root = PathBuf::from(r"D:\runtime");
        let runtime = OwnedRuntimeReport {
            runtime_root: root.clone(),
            sdk_root: PathBuf::from(OWNED_SDK_ROOT),
            toolchain_root: root.join("qemu"),
            qemu_data_root: root.join("share").join("qemu"),
            image_root: root.join("image"),
            instance_root: root.join("instance"),
            profile_path: root.join("profile.json"),
            qemu_path: root.join("qemu-system-x86_64.exe"),
            qemu_img_path: root.join("qemu-img.exe"),
            scrcpy: app_window::ScrcpyStatus {
                install_root: PathBuf::from(r"D:\vibecoding\sdk\scrcpy"),
                scrcpy_path: PathBuf::from(r"D:\vibecoding\sdk\scrcpy\scrcpy.exe"),
                provenance_path: PathBuf::from(r"D:\vibecoding\sdk\scrcpy\provenance.json"),
                archive_path: PathBuf::from(
                    r"D:\vibecoding\sdk\.downloads\scrcpy\scrcpy-win64-v4.1.zip",
                ),
                central_adb_path: PathBuf::from(
                    r"D:\vibecoding\sdk\android\platform-tools\adb.exe",
                ),
                available: true,
                version: Some("scrcpy 4.1".to_string()),
                blocker: None,
            },
            iso_path: root.join(bliss::IMAGE_FILE_NAME),
            kernel_path: root.join(bliss::KERNEL_FILE_NAME),
            initrd_path: root.join(bliss::PATCHED_INITRD_FILE_NAME),
            image_manifest_path: root.join("runtime-image.json"),
            userdata_path: root.join("data.img"),
            profile_exists: true,
            qemu_available: true,
            qemu_img_available: true,
            qemu_data_available: true,
            mke2fs_available: true,
            whpx_ready: true,
            image_ready: true,
            image_mode: "blissos-14-direct-kernel".to_string(),
            launch_ready: true,
            blockers: Vec::new(),
            profile: default_runtime_profile(
                "reader",
                "owned-image",
                &runtime_settings::SimulatorSettings::default(),
            ),
        };

        let args = owned_qemu_command_line(&runtime);
        let joined = args.join(" ");

        assert!(joined.contains("-name guest=reader,process=AndroidSimulator-reader"));
        assert!(joined.contains("-accel whpx"));
        assert!(joined.contains("-cpu Skylake-Client-v4"));
        assert!(joined.contains("-m 6144"));
        assert!(joined.contains(r"-L D:\runtime\share\qemu"));
        assert!(joined.contains("hostfwd=tcp:127.0.0.1:15555-:5555"));
        assert!(joined.contains("ipv6=off"));
        assert!(joined.contains("-device virtio-vga-gl,xres=640,yres=480"));
        assert!(joined.contains("-display egl-headless,gl=on"));
        assert!(joined.contains("-kernel D:\\runtime\\kernel"));
        assert!(joined.contains("-initrd D:\\runtime\\initrd.android-simulator.img"));
        assert!(joined.contains("DATA=vda"));
        assert!(joined.contains("GRALLOC=minigbm_arcvm"));
        assert!(!joined.contains("HWACCEL=0"));
        assert!(joined.contains("androidboot.qemu=1"));
        assert!(!joined.contains("VIRT_WIFI=1"));
        assert!(joined.contains("format=raw"));
        assert!(joined.contains("discard=unmap"));
        assert!(!joined.to_ascii_lowercase().contains("tcg"));
        assert!(!joined.to_ascii_lowercase().contains("emulator.exe"));
        assert!(!joined.to_ascii_lowercase().contains("sdl"));
        assert!(!joined.to_ascii_lowercase().contains("foreground"));
    }

    #[test]
    fn test_owned_background_process_is_hidden_and_normal_priority() {
        assert_ne!(
            WINDOWS_BACKGROUND_CREATION_FLAGS & WINDOWS_CREATE_NO_WINDOW,
            0
        );
        assert_ne!(
            WINDOWS_BACKGROUND_CREATION_FLAGS & WINDOWS_NORMAL_PRIORITY_CLASS,
            0
        );
        assert_eq!(
            WINDOWS_BACKGROUND_CREATION_FLAGS & WINDOWS_BELOW_NORMAL_PRIORITY_CLASS,
            0
        );
    }

    #[test]
    fn test_owned_qemu_startup_grace_catches_delayed_early_exit() {
        assert!(OWNED_QEMU_STARTUP_GRACE >= Duration::from_secs(5));
        assert!(OWNED_QEMU_STARTUP_POLL_INTERVAL <= Duration::from_millis(250));
    }

    #[test]
    fn test_owned_pid_resolution_recovers_a_trusted_adb_listener() {
        assert_eq!(
            select_owned_runtime_pid(None, Some((35764, true))),
            OwnedPidResolution::Recover(35764)
        );
        assert_eq!(
            select_owned_runtime_pid(Some((1200, false)), Some((35764, true))),
            OwnedPidResolution::Recover(35764)
        );
        assert_eq!(
            select_owned_runtime_pid(None, Some((999, false))),
            OwnedPidResolution::ForeignPortOwner(999)
        );
    }

    #[test]
    fn test_owned_runtime_ready_marker_requires_the_current_qemu_pid() {
        assert!(owned_runtime_ready_marker_matches(
            Some("32908\n"),
            Some(32908)
        ));
        assert!(!owned_runtime_ready_marker_matches(
            Some("32908"),
            Some(41000)
        ));
        assert!(!owned_runtime_ready_marker_matches(
            Some("invalid"),
            Some(32908)
        ));
        assert!(!owned_runtime_ready_marker_matches(Some("32908"), None));
    }

    #[test]
    fn test_owned_runtime_ready_marker_never_bypasses_a_missing_target_adb_transport() {
        assert!(owned_runtime_ready_marker_allows_fast_path(
            Some("32908\n"),
            Some(32908),
            true,
        ));
        assert!(!owned_runtime_ready_marker_allows_fast_path(
            Some("32908\n"),
            Some(32908),
            false,
        ));
    }

    #[test]
    fn test_app_list_enrichment_reads_cache_without_an_adb_or_apk_analysis_path() {
        let runtime_root = std::env::temp_dir().join(format!(
            "android-simulator-app-list-cache-{}-{}",
            std::process::id(),
            unix_millis().unwrap()
        ));
        let icon_path = runtime_root.join("icons").join("reader.ico");
        fs::create_dir_all(icon_path.parent().unwrap()).unwrap();
        fs::write(&icon_path, b"cached-icon").unwrap();
        write_installed_app_identity(
            &runtime_root,
            InstalledAppIdentity {
                package: "com.example.reader".to_string(),
                name: "Reader Desk".to_string(),
                icon_path: Some(icon_path.clone()),
            },
        )
        .unwrap();
        let mut apps = vec![adb::AppInfo {
            package: "com.example.reader".to_string(),
            activity: "com.example.ReaderActivity".to_string(),
            label: "Reader".to_string(),
            icon_path: None,
            version: None,
        }];

        enrich_user_app_list(&runtime_root, &mut apps);

        assert_eq!(apps[0].label, "Reader Desk");
        assert_eq!(apps[0].icon_path.as_deref(), Some(icon_path.as_path()));
        let _ = fs::remove_dir_all(runtime_root);
    }

    #[test]
    fn test_hydrated_app_icon_updates_the_list_and_persistent_identity() {
        let runtime_root = std::env::temp_dir().join(format!(
            "android-simulator-hydrated-app-icon-{}-{}",
            std::process::id(),
            unix_millis().unwrap()
        ));
        let icon_path = runtime_root.join("app-icons").join("reader.ico");
        fs::create_dir_all(icon_path.parent().unwrap()).unwrap();
        fs::write(&icon_path, b"cached-icon").unwrap();
        let mut app = adb::AppInfo {
            package: "com.example.reader".to_string(),
            activity: "com.example.ReaderActivity".to_string(),
            label: "Reader".to_string(),
            icon_path: None,
            version: None,
        };

        assert!(apply_hydrated_user_app_icon(
            &runtime_root,
            &mut app,
            icon_path.clone()
        ));
        assert_eq!(app.icon_path.as_deref(), Some(icon_path.as_path()));
        let identity = read_installed_app_identity(&runtime_root, "com.example.reader").unwrap();
        assert_eq!(identity.name, "Reader");
        assert_eq!(identity.icon_path.as_deref(), Some(icon_path.as_path()));
        let _ = fs::remove_dir_all(runtime_root);
    }

    #[test]
    fn test_desktop_app_list_hides_camera_only_apps() {
        assert!(!is_desktop_usable_app(
            "net.sourceforge.opencamera",
            "Open Camera"
        ));
        assert!(!is_desktop_usable_app("com.android.camera2", "相机"));
        assert!(is_desktop_usable_app(
            "com.example.document",
            "Document Scanner"
        ));
    }

    #[test]
    fn test_cli_parses_stage_two_app_and_install_contracts() {
        let status =
            Cli::try_parse_from(["simulatorctl", "owned", "status", "--instance", "reader"])
                .unwrap();
        let Commands::Owned {
            command: OwnedCommand::Status(status_args),
        } = status.command
        else {
            panic!("expected owned status command");
        };
        assert_eq!(status_args.instance, "reader");

        let list = Cli::try_parse_from(["simulatorctl", "app", "list"]).unwrap();
        let Commands::App {
            command: AppCommand::List(list_args),
        } = list.command
        else {
            panic!("expected app list command");
        };
        assert!(!list_args.start);

        let started_list = Cli::try_parse_from(["simulatorctl", "app", "list", "--start"]).unwrap();
        let Commands::App {
            command: AppCommand::List(started_list_args),
        } = started_list.command
        else {
            panic!("expected app list command");
        };
        assert!(started_list_args.start);

        let install = Cli::try_parse_from([
            "simulatorctl",
            "apk",
            "install",
            "--apk",
            r"D:\apps\reader.apk",
            "--package",
            "com.example.reader",
            "--launch",
            "--title",
            "Reader",
        ])
        .unwrap();
        let Commands::Apk {
            command: ApkCommand::Install(args),
        } = install.command
        else {
            panic!("expected apk install command");
        };
        assert_eq!(args.package.as_deref(), Some("com.example.reader"));
        assert!(args.launch);
        assert_eq!(args.title.as_deref(), Some("Reader"));

        let launch = Cli::try_parse_from([
            "simulatorctl",
            "app",
            "launch",
            "--package",
            "com.example.reader",
            "--title",
            "Reader's Desk",
        ])
        .unwrap();
        let Commands::App {
            command: AppCommand::Launch(args),
        } = launch.command
        else {
            panic!("expected app launch command");
        };
        assert_eq!(args.title.as_deref(), Some("Reader's Desk"));

        let rename = Cli::try_parse_from([
            "simulatorctl",
            "app",
            "rename",
            "--package",
            "com.example.reader",
            "--name",
            "Reader Pro",
        ])
        .unwrap();
        let Commands::App {
            command: AppCommand::Rename(args),
        } = rename.command
        else {
            panic!("expected app rename command");
        };
        assert_eq!(args.package, "com.example.reader");
        assert_eq!(args.name, "Reader Pro");

        let uninstall = Cli::try_parse_from([
            "simulatorctl",
            "app",
            "uninstall",
            "--package",
            "com.example.reader",
        ])
        .unwrap();
        let Commands::App {
            command: AppCommand::Uninstall(args),
        } = uninstall.command
        else {
            panic!("expected app uninstall command");
        };
        assert_eq!(args.package, "com.example.reader");
    }

    #[test]
    fn test_cli_parses_shortcut_and_input_contracts() {
        let shortcut = Cli::try_parse_from([
            "simulatorctl",
            "shortcut",
            "create",
            "--package",
            "com.example.reader",
            "--name",
            "Reader",
            "--launcher",
            r"D:\Android Simulator\AndroidSimulator.exe",
        ])
        .unwrap();
        assert!(matches!(
            shortcut.command,
            Commands::Shortcut {
                command: ShortcutCommand::Create(_)
            }
        ));

        let swipe = Cli::try_parse_from([
            "simulatorctl",
            "input",
            "swipe",
            "--x1",
            "1",
            "--y1",
            "2",
            "--x2",
            "3",
            "--y2",
            "4",
            "--duration-ms",
            "250",
            "--display-id",
            "31",
        ])
        .unwrap();
        assert!(matches!(
            swipe.command,
            Commands::Input {
                command: InputCommand::Swipe(SwipeArgs {
                    duration_ms: 250,
                    display_id: Some(31),
                    ..
                })
            }
        ));
    }
}
