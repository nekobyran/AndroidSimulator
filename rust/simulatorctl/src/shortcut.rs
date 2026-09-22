use crate::adb::validate_package_id;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Debug, Serialize)]
pub struct ShortcutReport {
    pub shortcut_path: PathBuf,
    pub target: PathBuf,
    pub arguments: String,
    pub package: String,
    pub name: String,
    pub icon: PathBuf,
    pub created: bool,
}

pub fn windows_quote_argument(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            quoted.push('"');
        } else {
            quoted.extend(std::iter::repeat_n('\\', backslashes));
            quoted.push(ch);
        }
        backslashes = 0;
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

pub fn shortcut_arguments(package: &str, name: &str) -> String {
    format!(
        "--package {} --app-name {} --background",
        windows_quote_argument(package),
        windows_quote_argument(name)
    )
}

pub fn safe_shortcut_stem(name: &str) -> String {
    let mut stem = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .take(120)
        .collect::<String>();
    while stem.ends_with([' ', '.']) {
        stem.pop();
    }
    if stem.is_empty() {
        return "Android App".to_string();
    }
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.iter().any(|item| stem.eq_ignore_ascii_case(item)) {
        stem.insert(0, '_');
    }
    stem
}

pub fn create_shortcut(
    package: &str,
    name: &str,
    launcher: &Path,
    icon: Option<&Path>,
) -> Result<ShortcutReport> {
    validate_package_id(package)?;
    if name.trim().is_empty() {
        bail!("shortcut name cannot be empty");
    }
    let target = if launcher.is_absolute() {
        launcher.to_path_buf()
    } else {
        env::current_dir()?.join(launcher)
    };
    if !target.is_file() {
        bail!("launcher not found: {}", target.display());
    }
    let desktop = dirs::desktop_dir().ok_or_else(|| anyhow!("Desktop directory not found"))?;
    let shortcut_path = desktop.join(format!("{}.lnk", safe_shortcut_stem(name)));
    let arguments = shortcut_arguments(package, name);
    let working_directory = target.parent().unwrap_or_else(|| Path::new("."));
    let icon = icon
        .filter(|path| path.is_file())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| target.clone());
    let script = shortcut_script(
        &shortcut_path,
        &target,
        &arguments,
        working_directory,
        &icon,
        name,
    );
    let output = run_powershell(&script).context("failed to create desktop shortcut")?;
    if !output.status.success() {
        bail!(
            "shortcut creation failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !shortcut_path.is_file() {
        bail!("PowerShell returned success but shortcut was not created");
    }
    Ok(ShortcutReport {
        shortcut_path,
        target,
        arguments,
        package: package.to_string(),
        name: name.to_string(),
        icon,
        created: true,
    })
}

fn shortcut_script(
    shortcut_path: &Path,
    target: &Path,
    arguments: &str,
    working_directory: &Path,
    icon: &Path,
    name: &str,
) -> String {
    format!(
        "$ErrorActionPreference='Stop'; \
         $shell=New-Object -ComObject WScript.Shell; \
         $shortcut=$shell.CreateShortcut('{}'); \
         $shortcut.TargetPath='{}'; \
         $shortcut.Arguments='{}'; \
         $shortcut.WorkingDirectory='{}'; \
         $shortcut.IconLocation='{},0'; \
         $shortcut.Description='Open {} in Android Simulator'; \
         $shortcut.Save();",
        ps_escape_path(shortcut_path),
        ps_escape_path(target),
        ps_escape_text(arguments),
        ps_escape_path(working_directory),
        ps_escape_path(icon),
        ps_escape_text(name),
    )
}

fn run_powershell(script: &str) -> Result<Output> {
    Command::new("pwsh.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .context("pwsh.exe (PowerShell 7) is required for shortcut creation")
}

fn ps_escape_path(path: &Path) -> String {
    ps_escape_text(&path.to_string_lossy())
}

fn ps_escape_text(text: &str) -> String {
    text.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_quote_argument_escapes_quotes_and_trailing_backslashes() {
        assert_eq!(windows_quote_argument("simple"), "\"simple\"");
        assert_eq!(
            windows_quote_argument("A \\\"quoted\\\" path\\"),
            "\"A \\\\\\\"quoted\\\\\\\" path\\\\\""
        );
    }

    #[test]
    fn test_shortcut_arguments_matches_launcher_contract() {
        assert_eq!(
            shortcut_arguments("com.example.reader", "Reader's Desk"),
            "--package \"com.example.reader\" --app-name \"Reader's Desk\" --background"
        );
    }

    #[test]
    fn test_safe_shortcut_stem_removes_windows_metacharacters_and_reserved_names() {
        assert_eq!(safe_shortcut_stem(" Reader:/Desk? "), "Reader__Desk_");
        assert_eq!(safe_shortcut_stem("CON"), "_CON");
        assert_eq!(safe_shortcut_stem("..."), "Android App");
    }

    #[test]
    fn test_shortcut_script_keeps_launcher_target_and_app_icon_distinct() {
        let launcher = Path::new(r"D:\release\AndroidSimulator.App.exe");
        let app_icon = Path::new(r"D:\runtime\app-icons\com.example.reader.ico");

        let script = shortcut_script(
            Path::new(r"C:\Users\Tester\Desktop\Reader.lnk"),
            launcher,
            r#"--package "com.example.reader" --background"#,
            Path::new(r"D:\release"),
            app_icon,
            "Reader",
        );

        assert!(script.contains(r"$shortcut.TargetPath='D:\release\AndroidSimulator.App.exe';"));
        assert!(
            script.contains(
                r"$shortcut.IconLocation='D:\runtime\app-icons\com.example.reader.ico,0';"
            )
        );
        assert!(!script.contains(r"$shortcut.TargetPath='D:\runtime\app-icons"));
        assert!(!script.contains(r"$shortcut.IconLocation='D:\release\AndroidSimulator.App.exe"));
    }
}
