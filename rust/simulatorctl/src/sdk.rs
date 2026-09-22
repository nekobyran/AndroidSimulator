use crate::adb::validate_package_id;
use anyhow::{Context, Result, anyhow, bail};
use roxmltree::{Document, Node};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const DEFAULT_VIBECODING_SDK_ROOT: &str = r"D:\vibecoding\sdk";
pub const DEFAULT_ANDROID_SDK_ROOT: &str = r"D:\vibecoding\sdk\android";
const DEFAULT_JAVA_HOME: &str = r"D:\vibecoding\sdk\jdk";
const DESKTOP_ROUNDED_ICON_SIZE: u32 = 256;
const DESKTOP_ROUNDED_ICON_RADIUS_PERCENT: u32 = 22;

#[derive(Debug, Serialize)]
pub struct ToolStatus {
    pub path: Option<PathBuf>,
    pub available: bool,
    pub detail: String,
}

pub fn sdk_candidates(
    android_home: Option<PathBuf>,
    android_sdk_root: Option<PathBuf>,
    vibecoding_sdk_root: Option<PathBuf>,
    system_drive: &str,
) -> Vec<PathBuf> {
    let mut approved = Vec::new();
    if let Some(root) = vibecoding_sdk_root
        && normalized_key(&root) == normalized_key(Path::new(DEFAULT_VIBECODING_SDK_ROOT))
    {
        push_unique_approved(&mut approved, root.join("android"), system_drive);
    }
    push_unique_approved(
        &mut approved,
        PathBuf::from(DEFAULT_ANDROID_SDK_ROOT),
        system_drive,
    );

    let mut candidates = Vec::new();
    for path in [android_home, android_sdk_root].into_iter().flatten() {
        if is_on_drive(&path, system_drive)
            || !approved
                .iter()
                .any(|root| normalized_key(root) == normalized_key(&path))
        {
            continue;
        }
        push_unique(&mut candidates, path);
    }
    for path in approved {
        push_unique(&mut candidates, path);
    }
    candidates
}

fn push_unique_approved(candidates: &mut Vec<PathBuf>, path: PathBuf, system_drive: &str) {
    if path.is_absolute() && !is_on_drive(&path, system_drive) {
        push_unique(candidates, path);
    }
}

fn push_unique(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    let key = normalized_key(&path);
    if !candidates
        .iter()
        .any(|existing| normalized_key(existing) == key)
    {
        candidates.push(path);
    }
}

fn normalized_key(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase()
}

pub fn parse_apkanalyzer_package(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| validate_package_id(line).is_ok())
        .map(ToString::to_string)
}

pub fn android_sdk_root() -> Option<PathBuf> {
    let system_drive = env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    sdk_candidates(
        env::var_os("ANDROID_HOME").map(PathBuf::from),
        env::var_os("ANDROID_SDK_ROOT").map(PathBuf::from),
        env::var_os("VIBECODING_SDK_ROOT").map(PathBuf::from),
        &system_drive,
    )
    .into_iter()
    .find(|path| {
        path.is_dir()
            && fs::canonicalize(path)
                .ok()
                .is_some_and(|resolved| !is_on_drive(&resolved, &system_drive))
    })
}

pub fn sdk_tool(folder: &str, name: &str) -> ToolStatus {
    tool_status(android_sdk_root().map(|root| root.join(folder).join(name)))
}

pub fn sdk_cmdline_tool(name: &str) -> ToolStatus {
    let path = android_sdk_root().and_then(|root| find_cmdline_tool(&root, name));
    tool_status(path)
}

pub fn require_tool(status: ToolStatus, label: &str) -> Result<PathBuf> {
    status
        .path
        .filter(|path| path.is_file())
        .ok_or_else(|| anyhow!("{label} not found in the approved vibecoding SDK root"))
}

pub fn apk_package(apkanalyzer: &Path, apk_path: &Path) -> Result<String> {
    let output = run_script_or_executable(
        apkanalyzer,
        &["manifest", "application-id", &apk_path.to_string_lossy()],
    )
    .with_context(|| format!("failed to inspect APK: {}", apk_path.display()))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("apkanalyzer failed: {combined}");
    }
    parse_apkanalyzer_package(&combined)
        .ok_or_else(|| anyhow!("apkanalyzer did not return a valid application id"))
}

pub fn apk_label(_apkanalyzer: &Path, apk_path: &Path) -> Result<Option<String>> {
    let aapt2 = require_latest_build_tool("aapt2.exe")?;
    let output = background_command(&aapt2)
        .args(["dump", "badging"])
        .arg(apk_path)
        .output()
        .with_context(|| format!("failed to inspect APK label: {}", apk_path.display()))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("aapt2 dump badging failed: {combined}");
    }
    Ok(parse_aapt_application_label(&combined))
}

pub fn cache_apk_icon(
    apkanalyzer: &Path,
    apk_path: &Path,
    package: &str,
    runtime_root: &Path,
) -> Result<Option<PathBuf>> {
    validate_package_id(package)?;
    if !apk_path.is_file() {
        bail!("APK icon source does not exist: {}", apk_path.display());
    }

    let icon_root = runtime_root.join("app-icons");
    fs::create_dir_all(&icon_root)?;
    let apk_hash = sha256_file(apk_path)?;
    let destination = icon_root.join(format!("{package}-{}.ico", &apk_hash[..16]));
    if destination.is_file() {
        return Ok(Some(destination));
    }

    let badging = aapt_badging(apk_path)?;
    let Some(icon_entry) = parse_aapt_application_icon(&badging) else {
        return Ok(None);
    };
    match materialize_apk_icon_file(
        apkanalyzer,
        apk_path,
        &icon_entry,
        &destination,
        &icon_root,
        package,
        &apk_hash,
    )? {
        true => Ok(Some(destination)),
        false => Ok(None),
    }
}

/// Cache a per-APK Explorer icon keyed only by file content hash.
///
/// Used by the shell IconHandler so each `.apk` shows its own application icon
/// without requiring package metadata first.
pub fn cache_apk_shell_icon(apk_path: &Path, runtime_root: &Path) -> Result<Option<PathBuf>> {
    if !apk_path.is_file() {
        bail!("APK icon source does not exist: {}", apk_path.display());
    }
    let icon_root = runtime_root.join("app-icons").join("shell");
    fs::create_dir_all(&icon_root)?;
    let apk_hash = sha256_file(apk_path)?;
    let destination = icon_root.join(format!("{}.ico", &apk_hash[..16]));
    if destination.is_file() {
        return Ok(Some(destination));
    }

    let badging = aapt_badging(apk_path)?;
    let package = parse_aapt_package_name(&badging).unwrap_or_else(|| "unknown".to_string());
    let Some(icon_entry) = parse_aapt_application_icon(&badging) else {
        return Ok(None);
    };
    let apkanalyzer = require_tool(sdk_cmdline_tool("apkanalyzer"), "apkanalyzer").ok();
    let apkanalyzer_path = apkanalyzer.as_deref().unwrap_or_else(|| Path::new(""));
    match materialize_apk_icon_file(
        apkanalyzer_path,
        apk_path,
        &icon_entry,
        &destination,
        &icon_root,
        &package,
        &apk_hash,
    )? {
        true => Ok(Some(destination)),
        false => Ok(None),
    }
}

fn aapt_badging(apk_path: &Path) -> Result<String> {
    let aapt2 = require_latest_build_tool("aapt2.exe")?;
    let output = background_command(&aapt2)
        .args(["dump", "badging"])
        .arg(apk_path)
        .output()
        .context("failed to inspect APK icon metadata with aapt2")?;
    let badging = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("aapt2 dump badging failed: {badging}");
    }
    Ok(badging)
}

fn materialize_apk_icon_file(
    apkanalyzer: &Path,
    apk_path: &Path,
    icon_entry: &str,
    destination: &Path,
    icon_root: &Path,
    package: &str,
    apk_hash: &str,
) -> Result<bool> {
    validate_apk_resource_path(icon_entry)?;
    let temporary_png = icon_root.join(format!(
        ".{package}-{}-{}.png",
        std::process::id(),
        &apk_hash[..8]
    ));
    let result = (|| -> Result<bool> {
        let extension = Path::new(icon_entry)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp"
        ) {
            let raster = extract_apk_entry(apk_path, icon_entry)?;
            match detect_raster_format(&raster) {
                Some("png") => fs::write(&temporary_png, raster)?,
                Some(actual_extension @ ("jpg" | "webp")) => {
                    // Some APKs store JPEG/WebP bytes behind a `.png` resource
                    // name. Trust the file signature so icon extraction remains
                    // automatic instead of falling back to the launcher icon.
                    let temporary_raster = temporary_png.with_extension(actual_extension);
                    fs::write(&temporary_raster, raster)?;
                    let conversion = render_raster_to_png(&temporary_raster, &temporary_png);
                    let _ = fs::remove_file(&temporary_raster);
                    conversion?;
                }
                _ => bail!("APK application icon is not a supported raster image"),
            }
        } else if extension.eq_ignore_ascii_case("xml") {
            if !apkanalyzer.is_file() {
                return Ok(false);
            }
            let vector_xml = apk_resource_xml(apkanalyzer, apk_path, icon_entry)?;
            let svg = match android_vector_to_svg(&vector_xml) {
                Ok(svg) => svg,
                Err(_) => return Ok(false),
            };
            render_svg_to_png(&svg, &temporary_png)?;
        } else {
            return Ok(false);
        }

        let png = fs::read(&temporary_png)?;
        let ico = png_to_ico_bytes(&png)?;
        atomic_write(destination, &ico)?;
        Ok(true)
    })();
    let _ = fs::remove_file(&temporary_png);
    result
}

pub fn cached_apk_icon(runtime_root: &Path, package: &str) -> Option<PathBuf> {
    validate_package_id(package).ok()?;
    let prefix = format!("{package}-");
    let mut icons = fs::read_dir(runtime_root.join("app-icons"))
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&prefix)
                        && name.ends_with(".ico")
                        && !name.contains("-desktop-rounded-")
                })
        })
        .collect::<Vec<_>>();
    icons.sort_by_key(|path| fs::metadata(path).and_then(|item| item.modified()).ok());
    icons.pop()
}

/// Prefer a previously cached compact desktop icon when the host needs a title-bar asset.
/// This path never hashes or re-renders multi-megabyte APK dumps on the launch hot path.
pub fn prefer_existing_desktop_rounded_icon(source_icon: &Path) -> Option<PathBuf> {
    if !source_icon.is_file() {
        return None;
    }
    let file_name = source_icon.file_name()?.to_string_lossy();
    if file_name.contains("-desktop-rounded-") {
        return Some(source_icon.to_path_buf());
    }
    let directory = source_icon.parent()?;
    let stem = source_icon.file_stem()?.to_string_lossy();
    let mut matches = fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{stem}-desktop-rounded-")) && name.ends_with(".ico")
                })
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|path| {
        fs::metadata(path)
            .map(|item| item.len())
            .unwrap_or(u64::MAX)
    });
    matches.into_iter().next()
}

/// Creates a desktop-only rounded icon without modifying the source artwork.
///
/// The cached ICO contains one 256x256 PNG-compressed frame. Its cache key is
/// derived from the source bytes so replacing an icon never reuses stale output.
pub fn cache_desktop_rounded_icon(source_icon: &Path, runtime_root: &Path) -> Result<PathBuf> {
    if !source_icon.is_file() {
        bail!(
            "desktop icon source does not exist: {}",
            source_icon.display()
        );
    }

    let icon_root = runtime_root.join("app-icons");
    fs::create_dir_all(&icon_root)?;
    let source_hash = sha256_file(source_icon)?;
    let destination = desktop_rounded_icon_destination(&icon_root, source_icon, &source_hash);
    if destination.is_file() {
        return Ok(destination);
    }

    let temporary_png = icon_root.join(format!(
        ".desktop-rounded-{}-{}.png",
        std::process::id(),
        &source_hash[..8]
    ));
    let temporary_source_png = icon_root.join(format!(
        ".desktop-rounded-source-{}-{}.png",
        std::process::id(),
        &source_hash[..8]
    ));
    let source_bytes = fs::read(source_icon)?;
    let extracted_png = largest_ico_png_payload(&source_bytes)?;
    if let Some(png) = extracted_png {
        fs::write(&temporary_source_png, png)?;
    }
    let render_source = if extracted_png.is_some() {
        temporary_source_png.as_path()
    } else {
        source_icon
    };
    let render_result = (|| -> Result<PathBuf> {
        render_desktop_rounded_icon(render_source, &temporary_png)?;
        let png = fs::read(&temporary_png)?;
        let ico = png_to_ico_bytes(&png)?;
        atomic_write(&destination, &ico)?;
        Ok(destination.clone())
    })();
    let _ = fs::remove_file(&temporary_png);
    let _ = fs::remove_file(&temporary_source_png);
    render_result
}

fn largest_ico_png_payload(bytes: &[u8]) -> Result<Option<&[u8]>> {
    const ICO_HEADER: &[u8; 4] = &[0, 0, 1, 0];
    const DIRECTORY_HEADER_SIZE: usize = 6;
    const DIRECTORY_ENTRY_SIZE: usize = 16;

    if bytes.len() < ICO_HEADER.len() || &bytes[..ICO_HEADER.len()] != ICO_HEADER {
        return Ok(None);
    }
    if bytes.len() < DIRECTORY_HEADER_SIZE {
        bail!("ICO directory header is truncated");
    }
    let count = usize::from(u16::from_le_bytes(bytes[4..6].try_into()?));
    if count == 0 {
        bail!("ICO directory contains no images");
    }
    let directory_end = DIRECTORY_HEADER_SIZE
        .checked_add(
            count
                .checked_mul(DIRECTORY_ENTRY_SIZE)
                .ok_or_else(|| anyhow!("ICO directory size overflow"))?,
        )
        .ok_or_else(|| anyhow!("ICO directory size overflow"))?;
    if directory_end > bytes.len() {
        bail!("ICO directory entries are truncated");
    }

    let mut largest: Option<(&[u8], u64, usize)> = None;
    for index in 0..count {
        let entry_start = DIRECTORY_HEADER_SIZE + index * DIRECTORY_ENTRY_SIZE;
        let entry = &bytes[entry_start..entry_start + DIRECTORY_ENTRY_SIZE];
        let payload_length = usize::try_from(u32::from_le_bytes(entry[8..12].try_into()?))?;
        let payload_offset = usize::try_from(u32::from_le_bytes(entry[12..16].try_into()?))?;
        let payload_end = payload_offset
            .checked_add(payload_length)
            .ok_or_else(|| anyhow!("ICO image payload bounds overflow at entry {index}"))?;
        if payload_length == 0 || payload_offset < directory_end || payload_end > bytes.len() {
            bail!("ICO image payload is out of bounds at entry {index}");
        }

        let payload = &bytes[payload_offset..payload_end];
        let Ok((width, height)) = png_dimensions(payload) else {
            continue;
        };
        let area = u64::from(width) * u64::from(height);
        if largest
            .as_ref()
            .is_none_or(|(_, largest_area, largest_length)| {
                (area, payload_length) > (*largest_area, *largest_length)
            })
        {
            largest = Some((payload, area, payload_length));
        }
    }
    Ok(largest.map(|(payload, _, _)| payload))
}

fn desktop_rounded_icon_destination(
    icon_root: &Path,
    source_icon: &Path,
    source_hash: &str,
) -> PathBuf {
    let source_stem = source_icon
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("icon");
    icon_root.join(format!(
        "{source_stem}-desktop-rounded-{}.ico",
        &source_hash[..16]
    ))
}

fn desktop_rounded_corner_radius(size: u32) -> u32 {
    size * DESKTOP_ROUNDED_ICON_RADIUS_PERCENT / 100
}

fn desktop_rounded_icon_script(size: u32, radius: u32) -> String {
    format!(
        "$ErrorActionPreference='Stop'; \
        Add-Type -AssemblyName System.Drawing.Common; \
        $size={size}; $radius={radius}; $diameter=$radius*2; \
        $image=[System.Drawing.Image]::FromFile($args[0]); \
        $bitmap=$null; $graphics=$null; $path=$null; \
        try {{ \
            $bitmap=[System.Drawing.Bitmap]::new($size,$size,[System.Drawing.Imaging.PixelFormat]::Format32bppArgb); \
            $bitmap.SetResolution(96,96); \
            $graphics=[System.Drawing.Graphics]::FromImage($bitmap); \
            $graphics.Clear([System.Drawing.Color]::Transparent); \
            $graphics.SmoothingMode=[System.Drawing.Drawing2D.SmoothingMode]::AntiAlias; \
            $graphics.InterpolationMode=[System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic; \
            $graphics.PixelOffsetMode=[System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality; \
            $graphics.CompositingQuality=[System.Drawing.Drawing2D.CompositingQuality]::HighQuality; \
            $path=[System.Drawing.Drawing2D.GraphicsPath]::new(); \
            $path.AddArc(0,0,$diameter,$diameter,180,90); \
            $path.AddArc($size-$diameter,0,$diameter,$diameter,270,90); \
            $path.AddArc($size-$diameter,$size-$diameter,$diameter,$diameter,0,90); \
            $path.AddArc(0,$size-$diameter,$diameter,$diameter,90,90); \
            $path.CloseFigure(); \
            $graphics.SetClip($path); \
            $target=[System.Drawing.RectangleF]::new(0,0,$size,$size); \
            $graphics.DrawImage($image,$target); \
            $bitmap.Save($args[1],[System.Drawing.Imaging.ImageFormat]::Png); \
        }} finally {{ \
            if ($path) {{ $path.Dispose() }}; \
            if ($graphics) {{ $graphics.Dispose() }}; \
            if ($bitmap) {{ $bitmap.Dispose() }}; \
            $image.Dispose(); \
        }}"
    )
}

fn render_desktop_rounded_icon(source: &Path, destination: &Path) -> Result<()> {
    let script = desktop_rounded_icon_script(
        DESKTOP_ROUNDED_ICON_SIZE,
        desktop_rounded_corner_radius(DESKTOP_ROUNDED_ICON_SIZE),
    );
    let output = background_command(Path::new("pwsh.exe"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-CommandWithArgs",
            &script,
        ])
        .arg(source)
        .arg(destination)
        .output()
        .context("failed to render rounded desktop icon")?;
    if !output.status.success() || !destination.is_file() {
        bail!(
            "rounded desktop icon rendering failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn parse_aapt_application_icon(output: &str) -> Option<String> {
    let mut candidates = Vec::new();
    let mut fallback = None;
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("application-icon-")
            && let Some((density, value)) = rest.split_once(':')
            && let Ok(density) = density.parse::<u32>()
            && let Some(path) = parse_single_quoted(value)
            && path.starts_with("res/")
        {
            candidates.push((density, path.to_string()));
        } else if line.starts_with("application:")
            && let Some((_, value)) = line.split_once(" icon='")
            && let Some(path) = value.split_once('\'').map(|item| item.0)
            && path.starts_with("res/")
        {
            fallback = Some(path.to_string());
        }
    }
    candidates.sort_by_key(|item| item.0);
    candidates.pop().map(|item| item.1).or(fallback)
}

fn parse_aapt_package_name(output: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|line| {
        let rest = line.strip_prefix("package:")?;
        let name = rest
            .split_whitespace()
            .find_map(|part| part.strip_prefix("name="))?;
        let package = parse_single_quoted(name)?;
        validate_package_id(package)
            .ok()
            .map(|_| package.to_string())
    })
}

fn parse_aapt_application_label(output: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|line| {
        let value = line.strip_prefix("application-label:")?;
        let label = parse_single_quoted(value)?.trim();
        (!label.is_empty() && !label.chars().any(char::is_control)).then(|| label.to_string())
    })
}

fn parse_single_quoted(value: &str) -> Option<&str> {
    let value = value.trim();
    value
        .strip_prefix('\'')?
        .split_once('\'')
        .map(|item| item.0)
        .filter(|item| !item.is_empty())
}

fn validate_apk_resource_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || !value.starts_with("res/")
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("unsafe APK icon resource path: {value}");
    }
    Ok(())
}

fn require_latest_build_tool(name: &str) -> Result<PathBuf> {
    let root = android_sdk_root()
        .ok_or_else(|| anyhow!("approved Android SDK root is unavailable"))?
        .join("build-tools");
    let mut candidates = fs::read_dir(&root)
        .with_context(|| {
            format!(
                "Android build-tools directory is missing: {}",
                root.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join(name))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| build_tool_version_key(path));
    candidates
        .pop()
        .ok_or_else(|| anyhow!("{name} was not found in the approved Android SDK"))
}

fn build_tool_version_key(path: &Path) -> Vec<u32> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or_default())
        .collect()
}

fn extract_apk_entry(apk_path: &Path, entry: &str) -> Result<Vec<u8>> {
    validate_apk_resource_path(entry)?;
    let output = background_command(Path::new("tar.exe"))
        .arg("-xOf")
        .arg(apk_path)
        .arg(entry)
        .output()
        .context("failed to extract APK icon resource")?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!(
            "failed to extract APK icon resource {entry}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

fn apk_resource_xml(apkanalyzer: &Path, apk_path: &Path, entry: &str) -> Result<String> {
    validate_apk_resource_path(entry)?;
    let output = run_script_or_executable(
        apkanalyzer,
        &[
            "resources",
            "xml",
            "--file",
            entry,
            &apk_path.to_string_lossy(),
        ],
    )?;
    if !output.status.success() {
        bail!(
            "apkanalyzer resources xml failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("APK icon XML is not UTF-8")
}

fn android_vector_to_svg(xml: &str) -> Result<String> {
    const ANDROID_NS: &str = "http://schemas.android.com/apk/res/android";
    let document = Document::parse(xml).context("decoded Android icon XML is invalid")?;
    let root = document.root_element();
    if root.tag_name().name() != "vector" {
        bail!("APK icon XML is not an Android vector drawable");
    }
    let viewport_width = android_attribute(root, ANDROID_NS, "viewportWidth")
        .ok_or_else(|| anyhow!("vector icon has no viewportWidth"))?;
    let viewport_height = android_attribute(root, ANDROID_NS, "viewportHeight")
        .ok_or_else(|| anyhow!("vector icon has no viewportHeight"))?;
    viewport_width.parse::<f32>()?;
    viewport_height.parse::<f32>()?;

    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\">",
        escape_xml_attribute(viewport_width),
        escape_xml_attribute(viewport_height)
    );
    append_vector_children(root, ANDROID_NS, &mut svg)?;
    svg.push_str("</svg>");
    Ok(svg)
}

fn append_vector_children(node: Node<'_, '_>, android_ns: &str, svg: &mut String) -> Result<()> {
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "path" => append_vector_path(child, android_ns, svg)?,
            "group" => {
                svg.push_str("<g");
                if let Some(transform) = vector_group_transform(child, android_ns) {
                    svg.push_str(" transform=\"");
                    svg.push_str(&escape_xml_attribute(&transform));
                    svg.push('\"');
                }
                svg.push('>');
                append_vector_children(child, android_ns, svg)?;
                svg.push_str("</g>");
            }
            _ => {}
        }
    }
    Ok(())
}

fn append_vector_path(node: Node<'_, '_>, android_ns: &str, svg: &mut String) -> Result<()> {
    let path_data = android_attribute(node, android_ns, "pathData")
        .ok_or_else(|| anyhow!("vector path has no pathData"))?;
    svg.push_str("<path d=\"");
    svg.push_str(&escape_xml_attribute(path_data));
    svg.push('\"');

    let fill = android_attribute(node, android_ns, "fillColor").unwrap_or("#000000");
    append_svg_color(svg, "fill", "fill-opacity", fill)?;
    if let Some(fill_alpha) = android_attribute(node, android_ns, "fillAlpha") {
        svg.push_str(" fill-opacity=\"");
        svg.push_str(&escape_xml_attribute(fill_alpha));
        svg.push('\"');
    }
    if let Some(stroke) = android_attribute(node, android_ns, "strokeColor") {
        append_svg_color(svg, "stroke", "stroke-opacity", stroke)?;
    }
    for (android_name, svg_name) in [
        ("strokeWidth", "stroke-width"),
        ("strokeAlpha", "stroke-opacity"),
        ("strokeLineCap", "stroke-linecap"),
        ("strokeLineJoin", "stroke-linejoin"),
        ("strokeMiterLimit", "stroke-miterlimit"),
    ] {
        if let Some(value) = android_attribute(node, android_ns, android_name) {
            svg.push(' ');
            svg.push_str(svg_name);
            svg.push_str("=\"");
            svg.push_str(&escape_xml_attribute(value));
            svg.push('\"');
        }
    }
    if android_attribute(node, android_ns, "fillType") == Some("evenOdd") {
        svg.push_str(" fill-rule=\"evenodd\"");
    }
    svg.push_str(" />");
    Ok(())
}

fn append_svg_color(
    svg: &mut String,
    color_attribute: &str,
    opacity_attribute: &str,
    value: &str,
) -> Result<()> {
    let (color, opacity) = android_color_to_svg(value)?;
    svg.push(' ');
    svg.push_str(color_attribute);
    svg.push_str("=\"");
    svg.push_str(&color);
    svg.push('\"');
    if let Some(opacity) = opacity {
        svg.push(' ');
        svg.push_str(opacity_attribute);
        svg.push_str("=\"");
        svg.push_str(&format!("{opacity:.4}"));
        svg.push('\"');
    }
    Ok(())
}

fn android_color_to_svg(value: &str) -> Result<(String, Option<f32>)> {
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            6 => Ok((format!("#{hex}"), None)),
            8 => {
                let alpha = u8::from_str_radix(&hex[..2], 16)? as f32 / 255.0;
                Ok((format!("#{}", &hex[2..]), Some(alpha)))
            }
            _ => bail!("unsupported Android vector color: {value}"),
        };
    }
    if value == "transparent" {
        return Ok(("#000000".to_string(), Some(0.0)));
    }
    bail!("unresolved Android vector color: {value}")
}

fn vector_group_transform(node: Node<'_, '_>, android_ns: &str) -> Option<String> {
    let pivot_x = android_attribute(node, android_ns, "pivotX").unwrap_or("0");
    let pivot_y = android_attribute(node, android_ns, "pivotY").unwrap_or("0");
    let rotation = android_attribute(node, android_ns, "rotation").unwrap_or("0");
    let scale_x = android_attribute(node, android_ns, "scaleX").unwrap_or("1");
    let scale_y = android_attribute(node, android_ns, "scaleY").unwrap_or("1");
    let translate_x = android_attribute(node, android_ns, "translateX").unwrap_or("0");
    let translate_y = android_attribute(node, android_ns, "translateY").unwrap_or("0");
    if [rotation, translate_x, translate_y]
        .iter()
        .all(|value| *value == "0")
        && [scale_x, scale_y].iter().all(|value| *value == "1")
    {
        return None;
    }
    Some(format!(
        "translate({translate_x} {translate_y}) translate({pivot_x} {pivot_y}) rotate({rotation}) scale({scale_x} {scale_y}) translate(-{pivot_x} -{pivot_y})"
    ))
}

fn android_attribute<'a, 'input>(
    node: Node<'a, 'input>,
    namespace: &str,
    name: &str,
) -> Option<&'a str> {
    node.attribute((namespace, name))
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('\"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_svg_to_png(svg: &str, destination: &Path) -> Result<()> {
    let sdk_root = android_sdk_root()
        .and_then(|root| root.parent().map(Path::to_path_buf))
        .ok_or_else(|| anyhow!("approved vibecoding SDK root is unavailable"))?;
    let renderer = sdk_root
        .join("msys64")
        .join("ucrt64")
        .join("bin")
        .join("rsvg-convert.exe");
    if !renderer.is_file() {
        bail!(
            "SVG icon renderer is missing from the central SDK: {}",
            renderer.display()
        );
    }
    let svg_path = destination.with_extension(format!("{}.svg", std::process::id()));
    fs::write(&svg_path, svg)?;
    let output = background_command(&renderer)
        .args(["-w", "256", "-h", "256", "-o"])
        .arg(destination)
        .arg(&svg_path)
        .output()
        .context("failed to render Android vector icon")?;
    let _ = fs::remove_file(&svg_path);
    if !output.status.success() || !destination.is_file() {
        bail!(
            "rsvg-convert failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn render_raster_to_png(source: &Path, destination: &Path) -> Result<()> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "webp" {
        if let Some(dwebp) = webp_decoder() {
            let output = background_command(&dwebp)
                .arg(source)
                .arg("-o")
                .arg(destination)
                .output()
                .context("failed to convert WebP icon with dwebp")?;
            if output.status.success() && destination.is_file() {
                return Ok(());
            }
        }
        if let Some(ffmpeg) = ffmpeg_binary() {
            let output = background_command(&ffmpeg)
                .args(["-y", "-i"])
                .arg(source)
                .arg(destination)
                .output()
                .context("failed to convert WebP icon with ffmpeg")?;
            if output.status.success() && destination.is_file() {
                return Ok(());
            }
        }
    }

    const SCRIPT: &str = "Add-Type -AssemblyName System.Drawing.Common; \
        $image=[System.Drawing.Image]::FromFile($args[0]); \
        try { $image.Save($args[1],[System.Drawing.Imaging.ImageFormat]::Png) } \
        finally { $image.Dispose() }";
    let output = background_command(Path::new("pwsh.exe"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-CommandWithArgs",
            SCRIPT,
        ])
        .arg(source)
        .arg(destination)
        .output()
        .context("failed to convert Android raster icon to PNG")?;
    if !output.status.success() || !destination.is_file() {
        bail!(
            "Android raster icon conversion failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn detect_raster_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn webp_decoder() -> Option<PathBuf> {
    [
        PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin\dwebp.exe"),
        PathBuf::from(r"D:\vibecoding\sdk\msys64\mingw64\bin\dwebp.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn ffmpeg_binary() -> Option<PathBuf> {
    let path = PathBuf::from(r"D:\vibecoding\sdk\ffmpeg\bin\ffmpeg.exe");
    path.is_file().then_some(path)
}

fn png_to_ico_bytes(png: &[u8]) -> Result<Vec<u8>> {
    let (width, height) = png_dimensions(png)?;
    let png_length = u32::try_from(png.len())?;
    let mut ico = Vec::with_capacity(22 + png.len());
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    ico.push(if width >= 256 { 0 } else { width as u8 });
    ico.push(if height >= 256 { 0 } else { height as u8 });
    ico.extend_from_slice(&[0, 0]);
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&32u16.to_le_bytes());
    ico.extend_from_slice(&png_length.to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    Ok(ico)
}

fn png_dimensions(png: &[u8]) -> Result<(u32, u32)> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 29 || &png[..8] != PNG_SIGNATURE || &png[12..16] != b"IHDR" {
        bail!("application icon renderer did not produce a valid PNG");
    }
    let width = u32::from_be_bytes(png[16..20].try_into()?);
    let height = u32::from_be_bytes(png[20..24].try_into()?);
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        bail!("application icon PNG dimensions are invalid: {width}x{height}");
    }
    Ok((width, height))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_) if path.is_file() => {
            let _ = fs::remove_file(&temporary);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error.into())
        }
    }
}

fn is_on_drive(path: &Path, drive: &str) -> bool {
    let requested_drive = drive
        .trim_end_matches(['\\', '/'])
        .as_bytes()
        .first()
        .copied();
    let path_drive = match path.components().next() {
        Some(std::path::Component::Prefix(prefix)) => match prefix.kind() {
            std::path::Prefix::Disk(letter) | std::path::Prefix::VerbatimDisk(letter) => {
                Some(letter)
            }
            _ => None,
        },
        _ => None,
    };
    matches!((path_drive, requested_drive), (Some(left), Some(right)) if left.eq_ignore_ascii_case(&right))
}

fn find_cmdline_tool(root: &Path, name: &str) -> Option<PathBuf> {
    let filename = if cfg!(windows) {
        format!("{name}.bat")
    } else {
        name.to_string()
    };
    let tools_root = root.join("cmdline-tools");
    let preferred = tools_root.join("latest").join("bin").join(&filename);
    if preferred.is_file() {
        return Some(preferred);
    }
    let mut versions = fs::read_dir(tools_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    versions
        .into_iter()
        .map(|version| version.join("bin").join(&filename))
        .find(|path| path.is_file())
}

fn tool_status(path: Option<PathBuf>) -> ToolStatus {
    let available = path.as_ref().is_some_and(|path| path.is_file());
    ToolStatus {
        path,
        available,
        detail: if available { "found" } else { "missing" }.to_string(),
    }
}

fn run_script_or_executable(path: &Path, args: &[&str]) -> Result<Output> {
    let is_cmd_script = path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
    });
    if is_cmd_script && cfg!(windows) {
        let mut command = background_command(Path::new("cmd.exe"));
        command.args(["/D", "/S", "/C"]).arg(path).args(args);
        let java_home = approved_java_home();
        if java_home.is_dir() {
            command.env("JAVA_HOME", &java_home);
            let mut paths = vec![java_home.join("bin")];
            if let Some(existing) = env::var_os("PATH") {
                paths.extend(env::split_paths(&existing));
            }
            command.env("PATH", env::join_paths(paths)?);
        }
        return Ok(command.output()?);
    }
    Ok(background_command(path).args(args).output()?)
}

fn background_command(path: &Path) -> Command {
    let mut command = Command::new(path);
    configure_background_command(&mut command);
    command
}

#[cfg(windows)]
fn configure_background_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000 | 0x0000_4000);
}

#[cfg(not(windows))]
fn configure_background_command(_command: &mut Command) {}

fn approved_java_home() -> PathBuf {
    env::var_os("VIBECODING_SDK_ROOT")
        .map(PathBuf::from)
        .filter(|root| {
            normalized_key(root) == normalized_key(Path::new(DEFAULT_VIBECODING_SDK_ROOT))
        })
        .map(|root| root.join("jdk"))
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_JAVA_HOME))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_PIXEL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xB5,
        0x1C, 0x0C, 0x02, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xFC,
        0xFF, 0x1F, 0x00, 0x02, 0xEB, 0x01, 0xF5, 0x8F, 0x59, 0xC5, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        png
    }

    fn png_compressed_ico(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut ico = vec![0, 0, 1, 0];
        ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
        let mut payload_offset = 6 + frames.len() * 16;
        for frame in frames {
            let (width, height) = png_dimensions(frame).unwrap();
            ico.push(if width >= 256 { 0 } else { width as u8 });
            ico.push(if height >= 256 { 0 } else { height as u8 });
            ico.extend_from_slice(&[0, 0]);
            ico.extend_from_slice(&1u16.to_le_bytes());
            ico.extend_from_slice(&32u16.to_le_bytes());
            ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
            ico.extend_from_slice(&(payload_offset as u32).to_le_bytes());
            payload_offset += frame.len();
        }
        for frame in frames {
            ico.extend_from_slice(frame);
        }
        ico
    }

    #[test]
    fn test_sdk_candidates_rejects_system_drive_and_uses_d_default() {
        let candidates = sdk_candidates(
            Some(PathBuf::from(r"C:\Users\owner\Android\Sdk")),
            Some(PathBuf::from(r"E:\Android\Sdk")),
            None,
            "C:",
        );

        assert_eq!(candidates, vec![PathBuf::from(DEFAULT_ANDROID_SDK_ROOT)]);
    }

    #[test]
    fn test_sdk_candidates_deduplicates_default_case_insensitively() {
        let candidates = sdk_candidates(
            Some(PathBuf::from(r"d:\VIBECODING\SDK\ANDROID")),
            None,
            None,
            "C:",
        );

        assert_eq!(
            candidates,
            vec![PathBuf::from(r"d:\VIBECODING\SDK\ANDROID")]
        );
    }

    #[test]
    fn test_sdk_candidates_rejects_noncentral_declared_root() {
        let candidates = sdk_candidates(
            Some(PathBuf::from(r"E:\portable-sdk\android")),
            Some(PathBuf::from(r"E:\other\android")),
            Some(PathBuf::from(r"E:\portable-sdk")),
            "C:",
        );

        assert_eq!(candidates, vec![PathBuf::from(DEFAULT_ANDROID_SDK_ROOT)]);
    }

    #[test]
    fn test_sdk_candidates_rejects_lexical_escape_from_approved_root() {
        let candidates = sdk_candidates(
            Some(PathBuf::from(r"D:\vibecoding\sdk\android\..\other-android")),
            None,
            None,
            "C:",
        );

        assert_eq!(candidates, vec![PathBuf::from(DEFAULT_ANDROID_SDK_ROOT)]);
    }

    #[test]
    fn test_sdk_candidates_rejects_verbatim_system_drive_root() {
        let candidates = sdk_candidates(
            Some(PathBuf::from(r"\\?\C:\vibecoding\sdk\android")),
            None,
            Some(PathBuf::from(r"\\?\C:\vibecoding\sdk")),
            "C:",
        );

        assert_eq!(candidates, vec![PathBuf::from(DEFAULT_ANDROID_SDK_ROOT)]);
    }

    #[test]
    fn test_parse_apkanalyzer_package_uses_first_valid_nonempty_line() {
        assert_eq!(
            parse_apkanalyzer_package("\r\ncom.example.reader\r\n"),
            Some("com.example.reader".to_string())
        );
        assert_eq!(parse_apkanalyzer_package("warning only\n"), None);
    }

    #[test]
    fn test_parse_aapt_application_icon_chooses_highest_density_resource() {
        let badging = "application-icon-160:'res/drawable/icon.xml'\napplication-icon-320:'res/mipmap-xhdpi/icon.png'\napplication: label='Reader' icon='res/drawable/icon.xml'\n";
        assert_eq!(
            parse_aapt_application_icon(badging),
            Some("res/mipmap-xhdpi/icon.png".to_string())
        );
    }

    #[test]
    fn test_parse_aapt_package_name_reads_quoted_application_id() {
        let badging = "package: name='com.example.reader' versionCode='1' versionName='1.0'\n";
        assert_eq!(
            parse_aapt_package_name(badging),
            Some("com.example.reader".to_string())
        );
        assert_eq!(parse_aapt_package_name("package: name='bad id'\n"), None);
    }

    #[test]
    fn test_parse_aapt_application_label_returns_resolved_display_name() {
        let badging = "package: name='cc.nkbr.lanzoumax'\napplication-label:'LanzouMax'\napplication: label='LanzouMax' icon='res/xE.jpg'\n";
        assert_eq!(
            parse_aapt_application_label(badging),
            Some("LanzouMax".to_string())
        );
    }

    #[test]
    fn test_android_vector_to_svg_preserves_real_paths_and_colors() {
        let vector = r##"<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
 android:width="108dp" android:height="108dp"
 android:viewportWidth="108" android:viewportHeight="108">
 <path android:fillColor="#0D9488" android:pathData="M18,8h72v72z" />
</vector>"##;
        let svg = android_vector_to_svg(vector).unwrap();
        assert!(svg.contains("viewBox=\"0 0 108 108\""));
        assert!(svg.contains("fill=\"#0D9488\""));
        assert!(svg.contains("d=\"M18,8h72v72z\""));
    }

    #[test]
    fn test_png_to_ico_bytes_embeds_png_at_standard_offset() {
        let png = png_header(256, 256);

        let ico = png_to_ico_bytes(&png).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 22);
        assert_eq!(&ico[22..], png.as_slice());
    }

    #[test]
    fn test_detect_raster_format_uses_file_signature_when_extension_lies() {
        assert_eq!(detect_raster_format(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(
            detect_raster_format(b"\xff\xd8\xff\xe0JFIFrest"),
            Some("jpg")
        );
        assert_eq!(
            detect_raster_format(b"RIFF\x10\x00\x00\x00WEBPrest"),
            Some("webp")
        );
        assert_eq!(detect_raster_format(b"not-an-image"), None);
    }

    #[test]
    fn test_largest_ico_png_payload_prefers_largest_valid_png_frame() {
        let small = png_header(32, 32);
        let large = png_header(256, 256);
        let ico = png_compressed_ico(&[small, large.clone()]);

        assert_eq!(
            largest_ico_png_payload(&ico).unwrap(),
            Some(large.as_slice())
        );
    }

    #[test]
    fn test_largest_ico_png_payload_rejects_out_of_bounds_entry() {
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        ico.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&100u32.to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());

        let error = largest_ico_png_payload(&ico).unwrap_err();
        assert!(error.to_string().contains("out of bounds at entry 0"));
    }

    #[test]
    fn test_desktop_rounded_corner_radius_is_twenty_two_percent() {
        assert_eq!(desktop_rounded_corner_radius(256), 56);
        assert_eq!(desktop_rounded_corner_radius(100), 22);
    }

    #[test]
    fn test_desktop_rounded_icon_destination_is_stable_and_desktop_specific() {
        let destination = desktop_rounded_icon_destination(
            Path::new(r"D:\runtime\app-icons"),
            Path::new(r"D:\source\Reader Icon.ico"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );

        assert_eq!(
            destination,
            PathBuf::from(r"D:\runtime\app-icons\Reader Icon-desktop-rounded-0123456789abcdef.ico")
        );
    }

    #[test]
    fn test_prefer_existing_desktop_rounded_icon_is_hashless_and_stable() {
        let test_root = env::temp_dir().join(format!(
            "android-simulator-prefer-rounded-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let icon_root = test_root.join("app-icons");
        fs::create_dir_all(&icon_root).unwrap();
        let raw = icon_root.join("com.example.reader-abcdef.ico");
        let rounded =
            icon_root.join("com.example.reader-abcdef-desktop-rounded-0123456789abcdef.ico");
        fs::write(&raw, b"raw-bytes").unwrap();
        fs::write(&rounded, b"rounded").unwrap();
        assert_eq!(
            prefer_existing_desktop_rounded_icon(&raw).as_deref(),
            Some(rounded.as_path())
        );
        assert_eq!(
            prefer_existing_desktop_rounded_icon(&rounded).as_deref(),
            Some(rounded.as_path())
        );
        let _ = fs::remove_dir_all(test_root);
    }

    #[test]
    fn test_cached_apk_icon_ignores_desktop_rounded_variant() {
        let test_root = env::temp_dir().join(format!(
            "simulatorctl-cached-apk-icon-test-{}",
            std::process::id()
        ));
        let icon_root = test_root.join("app-icons");
        let _ = fs::remove_dir_all(&test_root);
        fs::create_dir_all(&icon_root).unwrap();
        let original = icon_root.join("com.example.reader-0123456789abcdef.ico");
        let desktop_rounded =
            icon_root.join("com.example.reader-desktop-rounded-fedcba9876543210.ico");
        fs::write(&original, b"original").unwrap();
        fs::write(&desktop_rounded, b"rounded").unwrap();

        assert_eq!(
            cached_apk_icon(&test_root, "com.example.reader"),
            Some(original)
        );

        fs::remove_dir_all(&test_root).unwrap();
    }

    #[test]
    fn test_desktop_rounded_icon_script_keeps_transparent_png_canvas_and_round_clip() {
        let script = desktop_rounded_icon_script(256, 56);

        assert!(script.contains("$size=256; $radius=56;"));
        assert!(script.contains("Format32bppArgb"));
        assert!(script.contains("Clear([System.Drawing.Color]::Transparent)"));
        assert_eq!(script.matches("$path.AddArc(").count(), 4);
        assert!(script.contains("$graphics.SetClip($path)"));
        assert!(script.contains("[System.Drawing.Imaging.ImageFormat]::Png"));
    }

    #[cfg(windows)]
    #[test]
    fn test_cache_desktop_rounded_icon_writes_independent_png_compressed_256_ico() {
        let test_root = env::temp_dir().join(format!(
            "simulatorctl-rounded-icon-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&test_root);
        fs::create_dir_all(&test_root).unwrap();
        let source = test_root.join("source.png");
        fs::write(&source, ONE_PIXEL_PNG).unwrap();

        let destination = cache_desktop_rounded_icon(&source, &test_root).unwrap();
        let ico = fs::read(&destination).unwrap();

        assert_ne!(destination, source);
        assert_eq!(fs::read(&source).unwrap(), ONE_PIXEL_PNG);
        assert!(
            destination
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("-desktop-rounded-")
        );
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(&ico[22..30], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(ico[38..42].try_into().unwrap()), 256);
        assert_eq!(u32::from_be_bytes(ico[42..46].try_into().unwrap()), 256);

        fs::remove_dir_all(&test_root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn test_cache_desktop_rounded_icon_extracts_png_compressed_ico_before_rendering() {
        let test_root = env::temp_dir().join(format!(
            "simulatorctl-rounded-ico-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&test_root);
        fs::create_dir_all(&test_root).unwrap();
        let source = test_root.join("source.ico");
        let source_ico = png_to_ico_bytes(ONE_PIXEL_PNG).unwrap();
        fs::write(&source, &source_ico).unwrap();

        let destination = cache_desktop_rounded_icon(&source, &test_root).unwrap();
        let output_ico = fs::read(&destination).unwrap();

        assert_eq!(fs::read(&source).unwrap(), source_ico);
        assert_eq!(&output_ico[22..30], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            u32::from_be_bytes(output_ico[38..42].try_into().unwrap()),
            256
        );
        assert_eq!(
            u32::from_be_bytes(output_ico[42..46].try_into().unwrap()),
            256
        );
        assert_eq!(
            fs::read_dir(test_root.join("app-icons"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".desktop-rounded-source-"))
                .count(),
            0
        );

        fs::remove_dir_all(&test_root).unwrap();
    }
}
