use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

const APK_PROG_ID: &str = "AndroidSimulator.Apk";
const APK_EXTENSION_KEY: &str = r"HKCU\Software\Classes\.apk";
const APK_PROG_ID_KEY: &str = r"HKCU\Software\Classes\AndroidSimulator.Apk";
const APK_EXPLORER_CHOICE_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.apk";
const APK_OPEN_WITH_PROGIDS_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.apk\OpenWithProgids";
// {BB2E617C-0920-11D1-9A0B-00C04FC2D6C1} IExtractImage is not used; IconHandler CLSID is local.
const APK_ICON_HANDLER_CLSID: &str = "{A7E8C1D2-4B3F-4E9A-9C2D-81F0A6B7E5D1}";
const APK_ICON_HANDLER_CLSID_KEY: &str =
    r"HKCU\Software\Classes\CLSID\{A7E8C1D2-4B3F-4E9A-9C2D-81F0A6B7E5D1}";
const APK_ICON_HANDLER_INPROC_KEY: &str =
    r"HKCU\Software\Classes\CLSID\{A7E8C1D2-4B3F-4E9A-9C2D-81F0A6B7E5D1}\InProcServer32";
const APK_ICON_HANDLER_SHELLEX_KEY: &str =
    r"HKCU\Software\Classes\AndroidSimulator.Apk\shellex\IconHandler";
// Standard IconHandler shell extension key under the ProgID.
const APK_ICON_HANDLER_SHELL_GUID: &str = "{0003000C-0000-0000-C000-000000000046}";

#[derive(Debug, Serialize)]
pub struct ApkAssociationReport {
    pub schema_version: u8,
    pub extension: &'static str,
    pub prog_id: &'static str,
    pub launcher: PathBuf,
    pub open_command: String,
    pub registered: bool,
    pub scope: &'static str,
    pub explorer_choice_reset: bool,
    pub icon_handler: Option<PathBuf>,
    pub icon_handler_registered: bool,
}

pub fn apk_open_command(launcher: &Path) -> String {
    format!(
        "\"{}\" --apk \"%1\" --background",
        launcher.to_string_lossy()
    )
}

pub fn register_apk_association(launcher: &Path) -> Result<ApkAssociationReport> {
    let launcher = normalize_launcher(launcher)?;
    let open_command = apk_open_command(&launcher);
    let icon_handler = resolve_icon_handler_dll(&launcher)?;
    reset_explorer_apk_choice()?;
    add_default_value(APK_EXTENSION_KEY, APK_PROG_ID)?;
    add_named_value(
        APK_EXTENSION_KEY,
        "Content Type",
        "application/vnd.android.package-archive",
    )?;
    add_named_value(APK_EXTENSION_KEY, "PerceivedType", "application")?;
    add_default_value(APK_PROG_ID_KEY, "Android application package")?;
    add_default_value(
        &format!(r"{APK_PROG_ID_KEY}\DefaultIcon"),
        &format!("\"{}\",0", launcher.display()),
    )?;
    add_named_value(
        &format!(r"{APK_PROG_ID_KEY}\shell\open"),
        "FriendlyAppName",
        "Android Simulator",
    )?;
    add_default_value(
        &format!(r"{APK_PROG_ID_KEY}\shell\open\command"),
        &open_command,
    )?;
    register_icon_handler(&icon_handler)?;

    let report = inspect_apk_association(&launcher)?;
    if !report.registered {
        bail!("Windows registry did not retain the Android Simulator APK association");
    }
    notify_shell_association_changed();
    Ok(report)
}

pub fn inspect_apk_association(launcher: &Path) -> Result<ApkAssociationReport> {
    let launcher = normalize_launcher(launcher)?;
    let open_command = apk_open_command(&launcher);
    let extension = query_default_value(APK_EXTENSION_KEY).unwrap_or_default();
    let command =
        query_default_value(&format!(r"{APK_PROG_ID_KEY}\shell\open\command")).unwrap_or_default();
    let explorer_choice_reset = explorer_choice_is_exclusive();
    let icon_handler = resolve_icon_handler_dll(&launcher).ok();
    let icon_handler_registered = icon_handler_is_registered(icon_handler.as_deref());
    let association_ready =
        extension == APK_PROG_ID && command == open_command && explorer_choice_reset;
    Ok(ApkAssociationReport {
        schema_version: 3,
        extension: ".apk",
        prog_id: APK_PROG_ID,
        launcher,
        registered: association_ready && icon_handler_registered,
        open_command,
        scope: "current-user",
        explorer_choice_reset,
        icon_handler,
        icon_handler_registered,
    })
}

fn register_icon_handler(icon_handler: &Path) -> Result<()> {
    if !icon_handler.is_file() {
        bail!("APK IconHandler DLL not found: {}", icon_handler.display());
    }
    add_default_value(
        APK_ICON_HANDLER_CLSID_KEY,
        "Android Simulator APK Icon Handler",
    )?;
    add_default_value(APK_ICON_HANDLER_INPROC_KEY, &icon_handler.to_string_lossy())?;
    add_named_value(APK_ICON_HANDLER_INPROC_KEY, "ThreadingModel", "Apartment")?;
    add_default_value(APK_ICON_HANDLER_SHELLEX_KEY, APK_ICON_HANDLER_CLSID)?;
    // Also register under the standard IconHandler shell GUID path used by Explorer.
    add_default_value(
        &format!(r"{APK_PROG_ID_KEY}\shellex\{APK_ICON_HANDLER_SHELL_GUID}"),
        APK_ICON_HANDLER_CLSID,
    )?;
    Ok(())
}

fn icon_handler_is_registered(expected_dll: Option<&Path>) -> bool {
    let Some(expected_dll) = expected_dll else {
        return false;
    };
    let clsid = query_default_value(APK_ICON_HANDLER_SHELLEX_KEY).unwrap_or_default();
    if !clsid.eq_ignore_ascii_case(APK_ICON_HANDLER_CLSID) {
        return false;
    }
    let inproc = query_default_value(APK_ICON_HANDLER_INPROC_KEY).unwrap_or_default();
    if let (Ok(left), Ok(right)) = (
        Path::new(&inproc).canonicalize(),
        expected_dll.canonicalize(),
    ) {
        return left == right;
    }
    inproc.eq_ignore_ascii_case(&expected_dll.to_string_lossy())
}

pub fn resolve_icon_handler_dll(launcher: &Path) -> Result<PathBuf> {
    let launcher_dir = launcher
        .parent()
        .ok_or_else(|| anyhow::anyhow!("launcher has no parent directory"))?;
    let beside_launcher = launcher_dir.join("AndroidSimulator.ApkIcon.dll");
    if beside_launcher.is_file() {
        return Ok(beside_launcher);
    }
    // Debug builds may still keep the DLL under the Rust target output.
    let from_env = std::env::current_exe().ok().and_then(|path| {
        path.parent()
            .map(|dir| dir.join("AndroidSimulator.ApkIcon.dll"))
    });
    if let Some(path) = from_env
        && path.is_file()
    {
        return Ok(path);
    }
    bail!(
        "AndroidSimulator.ApkIcon.dll was not found beside {}",
        launcher.display()
    )
}

fn reset_explorer_apk_choice() -> Result<()> {
    // Explorer may keep a legacy OpenWithList MRU (for example from another
    // emulator) even when HKCU\Software\Classes points at Android Simulator.
    // Reset only the .apk choice and recreate an exclusive ProgID entry so a
    // real ShellExecute/double-click follows the command reported by status.
    let _ = run_reg(["DELETE", APK_EXPLORER_CHOICE_KEY, "/f"]);
    run_reg([
        "ADD",
        APK_OPEN_WITH_PROGIDS_KEY,
        "/v",
        APK_PROG_ID,
        "/t",
        "REG_NONE",
        "/d",
        "",
        "/f",
    ])
    .context("failed to reset the current-user Explorer APK choice")?;
    Ok(())
}

fn explorer_choice_is_exclusive() -> bool {
    let android_simulator_present =
        run_reg(["QUERY", APK_OPEN_WITH_PROGIDS_KEY, "/v", APK_PROG_ID]).is_ok();
    let legacy_mru_absent =
        run_reg(["QUERY", &format!(r"{APK_EXPLORER_CHOICE_KEY}\OpenWithList")]).is_err();
    let protected_choice_absent =
        run_reg(["QUERY", &format!(r"{APK_EXPLORER_CHOICE_KEY}\UserChoice")]).is_err();
    android_simulator_present && legacy_mru_absent && protected_choice_absent
}

fn normalize_launcher(launcher: &Path) -> Result<PathBuf> {
    let launcher = if launcher.is_absolute() {
        launcher.to_path_buf()
    } else {
        std::env::current_dir()?.join(launcher)
    };
    if !launcher.is_file() {
        bail!("launcher not found: {}", launcher.display());
    }
    if !launcher
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        bail!("APK association launcher must be an .exe file");
    }
    Ok(launcher)
}

fn add_default_value(key: &str, data: &str) -> Result<()> {
    run_reg(["ADD", key, "/ve", "/t", "REG_SZ", "/d", data, "/f"])
        .with_context(|| format!("failed to set default registry value for {key}"))?;
    Ok(())
}

fn add_named_value(key: &str, name: &str, data: &str) -> Result<()> {
    run_reg(["ADD", key, "/v", name, "/t", "REG_SZ", "/d", data, "/f"])
        .with_context(|| format!("failed to set registry value {key}\\{name}"))?;
    Ok(())
}

fn query_default_value(key: &str) -> Result<String> {
    let output = run_reg(["QUERY", key, "/ve"])?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find_map(|line| {
            let (_, value) = line.split_once("REG_SZ")?;
            Some(value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("registry key has no default REG_SZ value: {key}"))
}

fn run_reg<const N: usize>(args: [&str; N]) -> Result<Output> {
    let mut command = Command::new("reg.exe");
    command.args(args);
    configure_background_process(&mut command);
    let output = command.output().context("failed to start reg.exe")?;
    if !output.status.success() {
        bail!(
            "reg.exe failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn notify_shell_association_changed() {
    let mut command = Command::new("ie4uinit.exe");
    command.arg("-show");
    configure_background_process(&mut command);
    let _ = command.status();
    // Also broadcast association change via PowerShell for Explorer icon refresh.
    let mut refresh = Command::new("pwsh.exe");
    refresh.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Add-Type -Namespace Shell -Name Notify -MemberDefinition '[DllImport(\"shell32.dll\")] public static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);'; [Shell.Notify]::SHChangeNotify(0x08000000, 0x1000, [IntPtr]::Zero, [IntPtr]::Zero)",
    ]);
    configure_background_process(&mut refresh);
    let _ = refresh.status();
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apk_open_command_quotes_launcher_and_file_placeholder() {
        let command = apk_open_command(Path::new(
            r"D:\Apps\Android Simulator\AndroidSimulator.App.exe",
        ));

        assert_eq!(
            command,
            r#""D:\Apps\Android Simulator\AndroidSimulator.App.exe" --apk "%1" --background"#
        );
    }

    #[test]
    fn test_normalize_launcher_rejects_non_executable_or_missing_file() {
        assert!(normalize_launcher(Path::new(r"D:\missing\launcher.exe")).is_err());
        assert!(normalize_launcher(Path::new(r"D:\missing\launcher.cmd")).is_err());
    }

    #[test]
    fn test_apk_icon_handler_clsid_is_stable() {
        assert_eq!(
            APK_ICON_HANDLER_CLSID,
            "{A7E8C1D2-4B3F-4E9A-9C2D-81F0A6B7E5D1}"
        );
    }
}
