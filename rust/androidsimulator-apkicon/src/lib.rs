#![cfg(windows)]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicI32, AtomicUsize, Ordering},
};
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOINTERFACE,
    E_POINTER, HMODULE, S_FALSE, S_OK,
};
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::UI::WindowsAndMessaging::HICON;

type WinBool = i32;
type RawObject = *mut core::ffi::c_void;

const CLSID_APK_ICON_HANDLER: GUID = GUID {
    data1: 0xA7E8C1D2,
    data2: 0x4B3F,
    data3: 0x4E9A,
    data4: [0x9C, 0x2D, 0x81, 0xF0, 0xA6, 0xB7, 0xE5, 0xD1],
};

const IID_IUNKNOWN: GUID = GUID {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_ICLASSFACTORY: GUID = GUID {
    data1: 0x00000001,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IPERSIST: GUID = GUID {
    data1: 0x0000010C,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IPERSISTFILE: GUID = GUID {
    data1: 0x0000010B,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IEXTRACTICONW: GUID = GUID {
    data1: 0x000214EB,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const GIL_PERINSTANCE: u32 = 0x0002;

static LOCK_COUNT: AtomicI32 = AtomicI32::new(0);
static OBJECT_COUNT: AtomicI32 = AtomicI32::new(0);
static DLL_MODULE: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct Handler {
    persist_vtbl: *const PersistVtbl,
    extract_vtbl: *const ExtractVtbl,
    ref_count: AtomicI32,
    path: Option<PathBuf>,
    icon_cache: Option<PathBuf>,
}

#[repr(C)]
struct PersistVtbl {
    query_interface:
        unsafe extern "system" fn(*mut Handler, *const GUID, *mut RawObject) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut Handler) -> u32,
    release: unsafe extern "system" fn(*mut Handler) -> u32,
    get_class_id: unsafe extern "system" fn(*mut Handler, *mut GUID) -> HRESULT,
    is_dirty: unsafe extern "system" fn(*mut Handler) -> HRESULT,
    load: unsafe extern "system" fn(*mut Handler, *const u16, u32) -> HRESULT,
    save: unsafe extern "system" fn(*mut Handler, *const u16, WinBool) -> HRESULT,
    save_completed: unsafe extern "system" fn(*mut Handler, *const u16) -> HRESULT,
    get_cur_file: unsafe extern "system" fn(*mut Handler, *mut *mut u16) -> HRESULT,
}

#[repr(C)]
struct ExtractVtbl {
    query_interface:
        unsafe extern "system" fn(*mut ExtractVtbl, *const GUID, *mut RawObject) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut ExtractVtbl) -> u32,
    release: unsafe extern "system" fn(*mut ExtractVtbl) -> u32,
    get_icon_location: unsafe extern "system" fn(
        *mut ExtractVtbl,
        u32,
        *mut u16,
        u32,
        *mut i32,
        *mut u32,
    ) -> HRESULT,
    extract: unsafe extern "system" fn(
        *mut ExtractVtbl,
        *const u16,
        u32,
        *mut HICON,
        *mut HICON,
        u32,
    ) -> HRESULT,
}

#[repr(C)]
struct ClassFactory {
    vtbl: *const FactoryVtbl,
    ref_count: AtomicI32,
}

#[repr(C)]
struct FactoryVtbl {
    query_interface:
        unsafe extern "system" fn(*mut ClassFactory, *const GUID, *mut RawObject) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut ClassFactory) -> u32,
    release: unsafe extern "system" fn(*mut ClassFactory) -> u32,
    create_instance: unsafe extern "system" fn(
        *mut ClassFactory,
        RawObject,
        *const GUID,
        *mut RawObject,
    ) -> HRESULT,
    lock_server: unsafe extern "system" fn(*mut ClassFactory, WinBool) -> HRESULT,
}

static PERSIST_VTBL: PersistVtbl = PersistVtbl {
    query_interface: persist_query_interface,
    add_ref: persist_add_ref,
    release: persist_release,
    get_class_id: persist_get_class_id,
    is_dirty: persist_is_dirty,
    load: persist_load,
    save: persist_save,
    save_completed: persist_save_completed,
    get_cur_file: persist_get_cur_file,
};

static EXTRACT_VTBL: ExtractVtbl = ExtractVtbl {
    query_interface: extract_query_interface,
    add_ref: extract_add_ref,
    release: extract_release,
    get_icon_location: extract_get_icon_location,
    extract: extract_extract,
};

static FACTORY_VTBL: FactoryVtbl = FactoryVtbl {
    query_interface: factory_query_interface,
    add_ref: factory_add_ref,
    release: factory_release,
    create_instance: factory_create_instance,
    lock_server: factory_lock_server,
};

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    module: HMODULE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> WinBool {
    unsafe {
        if reason == DLL_PROCESS_ATTACH {
            DLL_MODULE.store(module as usize, Ordering::SeqCst);
            let _ = DisableThreadLibraryCalls(module);
        }
    }
    1
}

#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECT_COUNT.load(Ordering::SeqCst) == 0 && LOCK_COUNT.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut RawObject,
) -> HRESULT {
    unsafe {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = core::ptr::null_mut();
        if !guid_eq(&*rclsid, &CLSID_APK_ICON_HANDLER) {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory = Box::into_raw(Box::new(ClassFactory {
            vtbl: &FACTORY_VTBL,
            ref_count: AtomicI32::new(1),
        }));
        OBJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        let hr = factory_query_interface(factory, riid, ppv);
        factory_release(factory);
        hr
    }
}

unsafe fn handler_from_extract(this: *mut ExtractVtbl) -> *mut Handler {
    unsafe { (this as *mut u8).sub(std::mem::size_of::<*const PersistVtbl>()) as *mut Handler }
}

unsafe extern "system" fn factory_query_interface(
    this: *mut ClassFactory,
    riid: *const GUID,
    ppv: *mut RawObject,
) -> HRESULT {
    unsafe {
        if riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = core::ptr::null_mut();
        if guid_eq(&*riid, &IID_IUNKNOWN) || guid_eq(&*riid, &IID_ICLASSFACTORY) {
            *ppv = this as RawObject;
            factory_add_ref(this);
            S_OK
        } else {
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn factory_add_ref(this: *mut ClassFactory) -> u32 {
    unsafe { (*this).ref_count.fetch_add(1, Ordering::SeqCst) as u32 + 1 }
}

unsafe extern "system" fn factory_release(this: *mut ClassFactory) -> u32 {
    unsafe {
        let remaining = (*this).ref_count.fetch_sub(1, Ordering::SeqCst) - 1;
        if remaining == 0 {
            drop(Box::from_raw(this));
            OBJECT_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
        remaining as u32
    }
}

unsafe extern "system" fn factory_create_instance(
    _this: *mut ClassFactory,
    punk_outer: RawObject,
    riid: *const GUID,
    ppv: *mut RawObject,
) -> HRESULT {
    unsafe {
        if !punk_outer.is_null() {
            return CLASS_E_NOAGGREGATION;
        }
        if riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = core::ptr::null_mut();
        let handler = Box::into_raw(Box::new(Handler {
            persist_vtbl: &PERSIST_VTBL,
            extract_vtbl: &EXTRACT_VTBL,
            ref_count: AtomicI32::new(1),
            path: None,
            icon_cache: None,
        }));
        OBJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        let hr = persist_query_interface(handler, riid, ppv);
        persist_release(handler);
        hr
    }
}

unsafe extern "system" fn factory_lock_server(_this: *mut ClassFactory, flock: WinBool) -> HRESULT {
    if flock != 0 {
        LOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    } else {
        LOCK_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
    S_OK
}

unsafe extern "system" fn persist_query_interface(
    this: *mut Handler,
    riid: *const GUID,
    ppv: *mut RawObject,
) -> HRESULT {
    unsafe {
        if riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        *ppv = core::ptr::null_mut();
        if guid_eq(&*riid, &IID_IUNKNOWN)
            || guid_eq(&*riid, &IID_IPERSIST)
            || guid_eq(&*riid, &IID_IPERSISTFILE)
        {
            *ppv = this as RawObject;
            persist_add_ref(this);
            S_OK
        } else if guid_eq(&*riid, &IID_IEXTRACTICONW) {
            *ppv = std::ptr::addr_of_mut!((*this).extract_vtbl) as RawObject;
            persist_add_ref(this);
            S_OK
        } else {
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn persist_add_ref(this: *mut Handler) -> u32 {
    unsafe { (*this).ref_count.fetch_add(1, Ordering::SeqCst) as u32 + 1 }
}

unsafe extern "system" fn persist_release(this: *mut Handler) -> u32 {
    unsafe {
        let remaining = (*this).ref_count.fetch_sub(1, Ordering::SeqCst) - 1;
        if remaining == 0 {
            drop(Box::from_raw(this));
            OBJECT_COUNT.fetch_sub(1, Ordering::SeqCst);
        }
        remaining as u32
    }
}

unsafe extern "system" fn persist_get_class_id(
    _this: *mut Handler,
    pclass_id: *mut GUID,
) -> HRESULT {
    unsafe {
        if pclass_id.is_null() {
            return E_POINTER;
        }
        *pclass_id = CLSID_APK_ICON_HANDLER;
        S_OK
    }
}

unsafe extern "system" fn persist_is_dirty(_this: *mut Handler) -> HRESULT {
    S_FALSE
}

unsafe extern "system" fn persist_load(
    this: *mut Handler,
    psz_file_name: *const u16,
    _mode: u32,
) -> HRESULT {
    unsafe {
        if psz_file_name.is_null() {
            return E_INVALIDARG;
        }
        let Some(path) = wide_to_path(psz_file_name) else {
            return E_INVALIDARG;
        };
        (*this).path = Some(path);
        (*this).icon_cache = None;
        S_OK
    }
}

unsafe extern "system" fn persist_save(
    _this: *mut Handler,
    _psz_file_name: *const u16,
    _fremember: WinBool,
) -> HRESULT {
    E_FAIL
}

unsafe extern "system" fn persist_save_completed(
    _this: *mut Handler,
    _psz_file_name: *const u16,
) -> HRESULT {
    E_FAIL
}

unsafe extern "system" fn persist_get_cur_file(
    _this: *mut Handler,
    ppsz_file_name: *mut *mut u16,
) -> HRESULT {
    unsafe {
        if ppsz_file_name.is_null() {
            return E_POINTER;
        }
        *ppsz_file_name = core::ptr::null_mut();
        E_FAIL
    }
}

unsafe extern "system" fn extract_query_interface(
    this: *mut ExtractVtbl,
    riid: *const GUID,
    ppv: *mut RawObject,
) -> HRESULT {
    unsafe {
        let handler = handler_from_extract(this);
        persist_query_interface(handler, riid, ppv)
    }
}

unsafe extern "system" fn extract_add_ref(this: *mut ExtractVtbl) -> u32 {
    unsafe { persist_add_ref(handler_from_extract(this)) }
}

unsafe extern "system" fn extract_release(this: *mut ExtractVtbl) -> u32 {
    unsafe { persist_release(handler_from_extract(this)) }
}

unsafe extern "system" fn extract_get_icon_location(
    this: *mut ExtractVtbl,
    _u_flags: u32,
    sz_icon_file: *mut u16,
    cch_max: u32,
    pi_index: *mut i32,
    pw_flags: *mut u32,
) -> HRESULT {
    unsafe {
        if sz_icon_file.is_null() || pi_index.is_null() || pw_flags.is_null() {
            return E_POINTER;
        }
        let handler = handler_from_extract(this);
        let Some(path) = (*handler).path.clone() else {
            return E_FAIL;
        };
        let Some(icon) = ensure_icon(&path) else {
            return E_FAIL;
        };
        (*handler).icon_cache = Some(icon.clone());
        if !write_wide_path(&icon, sz_icon_file, cch_max) {
            return E_FAIL;
        }
        *pi_index = 0;
        *pw_flags = GIL_PERINSTANCE;
        S_OK
    }
}

unsafe extern "system" fn extract_extract(
    _this: *mut ExtractVtbl,
    _psz_file: *const u16,
    _n_icon_index: u32,
    _phicon_large: *mut HICON,
    _phicon_small: *mut HICON,
    _n_icon_size: u32,
) -> HRESULT {
    S_FALSE
}

fn ensure_icon(apk_path: &Path) -> Option<PathBuf> {
    let runtime_root = runtime_root()?;
    let icon_root = runtime_root.join("app-icons").join("shell");
    let _ = fs::create_dir_all(&icon_root);
    let hash = sha256_file(apk_path).ok()?;
    let destination = icon_root.join(format!("{}.ico", &hash[..16]));
    if destination.is_file() {
        return Some(destination);
    }

    if let Some(icon) = extract_via_simulatorctl(apk_path) {
        if icon.is_file() {
            return Some(icon);
        }
    }

    let badging = aapt_badging(apk_path)?;
    let icon_entry = parse_application_icon(&badging)?;
    let extension = Path::new(&icon_entry)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let bytes = extract_zip_entry(apk_path, &icon_entry)?;
    let png = match extension.as_str() {
        "png" => bytes,
        "jpg" | "jpeg" | "webp" => raster_to_png(&bytes, &extension)?,
        _ => return extract_via_simulatorctl(apk_path),
    };
    let ico = png_to_ico_bytes(&png)?;
    atomic_write(&destination, &ico).ok()?;
    destination.is_file().then_some(destination)
}

fn extract_via_simulatorctl(apk_path: &Path) -> Option<PathBuf> {
    let simulatorctl = simulatorctl_path()?;
    let output = Command::new(simulatorctl)
        .args(["apk", "shell-icon", "--apk", &apk_path.to_string_lossy()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_json_path(&text, "icon_path")
}

fn simulatorctl_path() -> Option<PathBuf> {
    let module = DLL_MODULE.load(Ordering::SeqCst);
    if module != 0 {
        if let Some(dir) = module_directory(module as HMODULE) {
            let candidate = dir.join("simulatorctl.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("simulatorctl.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn module_directory(module: HMODULE) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
    if len == 0 || len as usize >= buffer.len() {
        return None;
    }
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    Path::new(&path).parent().map(Path::to_path_buf)
}

fn runtime_root() -> Option<PathBuf> {
    let sdk = env::var_os("VIBECODING_SDK_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| {
            let default = PathBuf::from(r"D:\vibecoding\sdk");
            if default.is_dir() {
                Some(default)
            } else {
                None
            }
        })?;
    let root = sdk.join("android-simulator-runtime");
    fs::create_dir_all(&root).ok()?;
    Some(root)
}

fn aapt_badging(apk_path: &Path) -> Option<String> {
    let aapt2 = latest_build_tool("aapt2.exe")?;
    let output = Command::new(aapt2)
        .args(["dump", "badging"])
        .arg(apk_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn latest_build_tool(name: &str) -> Option<PathBuf> {
    let root = PathBuf::from(r"D:\vibecoding\sdk\android\build-tools");
    let mut candidates = fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join(name))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

fn parse_application_icon(output: &str) -> Option<String> {
    let mut candidates = Vec::new();
    let mut fallback = None;
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("application-icon-") {
            if let Some((density, value)) = rest.split_once(':') {
                if let Ok(density) = density.parse::<u32>() {
                    if let Some(path) = parse_single_quoted(value) {
                        candidates.push((density, path.to_string()));
                    }
                }
            }
        } else if line.starts_with("application:") {
            if let Some((_, value)) = line.split_once(" icon='") {
                if let Some(path) = value.split_once('\'').map(|item| item.0) {
                    fallback = Some(path.to_string());
                }
            }
        }
    }
    candidates.sort_by_key(|item| item.0);
    candidates.pop().map(|item| item.1).or(fallback)
}

fn parse_single_quoted(value: &str) -> Option<&str> {
    let value = value.trim();
    value
        .strip_prefix('\'')?
        .split_once('\'')
        .map(|item| item.0)
        .filter(|item| !item.is_empty())
}

fn extract_zip_entry(apk_path: &Path, entry: &str) -> Option<Vec<u8>> {
    if !entry.starts_with("res/") || entry.contains("..") {
        return None;
    }
    let output = Command::new("tar.exe")
        .arg("-xOf")
        .arg(apk_path)
        .arg(entry)
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    Some(output.stdout)
}

fn raster_to_png(bytes: &[u8], extension: &str) -> Option<Vec<u8>> {
    let temp_root = env::temp_dir().join("android-simulator-apk-icon");
    fs::create_dir_all(&temp_root).ok()?;
    let pid = std::process::id();
    let source = temp_root.join(format!("src-{pid}.{extension}"));
    let destination = temp_root.join(format!("out-{pid}.png"));
    fs::write(&source, bytes).ok()?;
    if extension.eq_ignore_ascii_case("webp") {
        for decoder in [
            PathBuf::from(r"D:\vibecoding\sdk\msys64\ucrt64\bin\dwebp.exe"),
            PathBuf::from(r"D:\vibecoding\sdk\msys64\mingw64\bin\dwebp.exe"),
        ] {
            if decoder.is_file() {
                let _ = Command::new(&decoder)
                    .arg(&source)
                    .arg("-o")
                    .arg(&destination)
                    .output();
                if destination.is_file() {
                    let png = fs::read(&destination).ok();
                    let _ = fs::remove_file(&source);
                    let _ = fs::remove_file(&destination);
                    return png;
                }
            }
        }
        let ffmpeg = PathBuf::from(r"D:\vibecoding\sdk\ffmpeg\bin\ffmpeg.exe");
        if ffmpeg.is_file() {
            let _ = Command::new(&ffmpeg)
                .args(["-y", "-i"])
                .arg(&source)
                .arg(&destination)
                .output();
            if destination.is_file() {
                let png = fs::read(&destination).ok();
                let _ = fs::remove_file(&source);
                let _ = fs::remove_file(&destination);
                return png;
            }
        }
    }
    let script = "Add-Type -AssemblyName System.Drawing.Common; \
        $image=[System.Drawing.Image]::FromFile($args[0]); \
        try { $image.Save($args[1],[System.Drawing.Imaging.ImageFormat]::Png) } \
        finally { $image.Dispose() }";
    let _ = Command::new("pwsh.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-CommandWithArgs",
            script,
        ])
        .arg(&source)
        .arg(&destination)
        .output();
    let png = fs::read(&destination).ok();
    let _ = fs::remove_file(&source);
    let _ = fs::remove_file(&destination);
    png
}

fn png_to_ico_bytes(png: &[u8]) -> Option<Vec<u8>> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 29 || &png[..8] != PNG_SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return None;
    }
    let png_length = u32::try_from(png.len()).ok()?;
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
    Some(ico)
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path)?;
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

fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
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

fn parse_json_path(text: &str, key: &str) -> Option<PathBuf> {
    let needle = format!("\"{key}\"");
    let index = text.find(&needle)?;
    let after = text[index + needle.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    if after.starts_with("null") {
        return None;
    }
    let after = after.strip_prefix('"')?;
    let mut value = String::new();
    let mut chars = after.chars();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            break;
        }
        if ch == '\\' {
            if let Some(escaped) = chars.next() {
                value.push(match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
        } else {
            value.push(ch);
        }
    }
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value.replace('/', "\\")))
    }
}

fn wide_to_path(ptr: *const u16) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 32_768 {
                return None;
            }
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);
        Some(PathBuf::from(text))
    }
}

fn write_wide_path(path: &Path, buffer: *mut u16, cch_max: u32) -> bool {
    if buffer.is_null() || cch_max == 0 {
        return false;
    }
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    if wide.len() > cch_max as usize {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), buffer, wide.len());
    }
    true
}

fn guid_eq(left: &GUID, right: &GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}
