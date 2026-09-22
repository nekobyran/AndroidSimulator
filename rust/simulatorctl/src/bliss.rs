use anyhow::{Context, Result, anyhow, bail};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub const IMAGE_ID: &str = "blissos-16.9.7-x86_64-foss";
pub const IMAGE_VERSION: &str = "16.9.7";
pub const IMAGE_ANDROID_API: u32 = 33;
pub const IMAGE_FILE_NAME: &str = "Bliss-v16.9.7-x86_64-OFFICIAL-foss-20241011.iso";
pub const IMAGE_URL: &str = "https://downloads.sourceforge.net/project/blissos-x86/Official/BlissOS16/FOSS/Generic/Bliss-v16.9.7-x86_64-OFFICIAL-foss-20241011.iso";
pub const IMAGE_SHA256: &str = "735cb962ec6bd92b62eb82a812831a38d79a0dfdf12b7973d2d0f7ab001ba68e";
pub const IMAGE_SECURITY_PATCH: &str = "2024-05-05";
pub const PATCHED_INITRD_FILE_NAME: &str = "initrd.android-simulator.img";
pub const KERNEL_FILE_NAME: &str = "kernel";
pub const ADB_PATCH_MARKER: &str = "Android Simulator loopback ADB transport";

const INITRD_FILE_NAME: &str = "initrd.img";
const INIT_SWITCH_ROOT: &str = "exec ${SWITCH:-switch_root} /android /init";

#[derive(Debug, Serialize)]
pub struct BootBundleReport {
    pub schema_version: u8,
    pub os: &'static str,
    pub version: &'static str,
    pub android_api: u32,
    pub security_patch: &'static str,
    pub source_url: &'static str,
    pub source_sha256: &'static str,
    pub iso_path: PathBuf,
    pub kernel_path: PathBuf,
    pub patched_initrd_path: PathBuf,
    pub manifest_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct BootBundleManifest<'a> {
    schema_version: u8,
    os: &'a str,
    version: &'a str,
    android_api: u32,
    security_patch: &'a str,
    source_url: &'a str,
    source_sha256: &'a str,
    source_iso: &'a Path,
    kernel: &'a Path,
    initrd: &'a Path,
    modifications: [&'a str; 4],
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open image for SHA256: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn prepare_boot_bundle(iso_path: &Path, image_root: &Path) -> Result<BootBundleReport> {
    if !iso_path.is_file() {
        bail!("BlissOS source ISO is missing: {}", iso_path.display());
    }
    let actual_sha256 = sha256_file(iso_path)?;
    if actual_sha256 != IMAGE_SHA256 {
        bail!("BlissOS source ISO SHA256 mismatch: expected {IMAGE_SHA256}, got {actual_sha256}");
    }

    fs::create_dir_all(image_root)?;
    let kernel_path = image_root.join(KERNEL_FILE_NAME);
    let original_initrd_path = image_root.join(INITRD_FILE_NAME);
    let patched_initrd_path = image_root.join(PATCHED_INITRD_FILE_NAME);
    extract_iso_boot_files(iso_path, image_root)?;
    require_nonempty_file(&kernel_path, "BlissOS kernel")?;
    require_nonempty_file(&original_initrd_path, "BlissOS initrd")?;
    patch_initrd(&original_initrd_path, &patched_initrd_path)?;
    require_nonempty_file(&patched_initrd_path, "Android Simulator patched initrd")?;

    let manifest_path = image_root.join("runtime-image.json");
    let manifest = BootBundleManifest {
        schema_version: 1,
        os: "BlissOS Generic FOSS",
        version: IMAGE_VERSION,
        android_api: IMAGE_ANDROID_API,
        security_patch: IMAGE_SECURITY_PATCH,
        source_url: IMAGE_URL,
        source_sha256: IMAGE_SHA256,
        source_iso: iso_path,
        kernel: &kernel_path,
        initrd: &patched_initrd_path,
        modifications: [
            "direct kernel boot for deterministic headless startup",
            "persistent ext4 data disk mounted as /data",
            "ADB transport restricted to QEMU host forwarding on 127.0.0.1",
            "x86_64 Codec2 mapper namespace repaired for per-app display encoding",
        ],
    };
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    Ok(BootBundleReport {
        schema_version: 1,
        os: "BlissOS Generic FOSS",
        version: IMAGE_VERSION,
        android_api: IMAGE_ANDROID_API,
        security_patch: IMAGE_SECURITY_PATCH,
        source_url: IMAGE_URL,
        source_sha256: IMAGE_SHA256,
        iso_path: iso_path.to_path_buf(),
        kernel_path,
        patched_initrd_path,
        manifest_path,
    })
}

fn extract_iso_boot_files(iso_path: &Path, image_root: &Path) -> Result<()> {
    let output = Command::new("tar.exe")
        .args(["-xf"])
        .arg(iso_path)
        .args(["-C"])
        .arg(image_root)
        .args([KERNEL_FILE_NAME, INITRD_FILE_NAME])
        .output()
        .context("Windows tar.exe is required to extract the verified BlissOS boot files")?;
    if !output.status.success() {
        bail!(
            "failed to extract BlissOS boot files: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn require_nonempty_file(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() || fs::metadata(path)?.len() == 0 {
        bail!("{label} is missing or empty: {}", path.display());
    }
    Ok(())
}

fn patch_initrd(source: &Path, destination: &Path) -> Result<()> {
    let mut decoder = GzDecoder::new(fs::File::open(source)?);
    let mut archive = Vec::new();
    decoder
        .read_to_end(&mut archive)
        .context("failed to decompress BlissOS initrd")?;
    let mut entries = parse_newc(&archive)?;
    let init_entry = entries
        .iter_mut()
        .find(|entry| entry.name == b"init")
        .ok_or_else(|| anyhow!("BlissOS initrd does not contain /init"))?;
    let original =
        std::str::from_utf8(&init_entry.data).context("BlissOS initrd /init is not valid UTF-8")?;
    init_entry.data = patch_init_script(original)?.into_bytes();

    let rebuilt = write_newc(&entries)?;
    let temporary = destination.with_extension("download");
    let mut encoder = GzEncoder::new(fs::File::create(&temporary)?, Compression::best());
    encoder.write_all(&rebuilt)?;
    encoder.finish()?.sync_all()?;
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(&temporary, destination)?;
    Ok(())
}

fn patch_init_script(script: &str) -> Result<String> {
    if script.contains(ADB_PATCH_MARKER) {
        bail!("BlissOS initrd already contains the Android Simulator ADB patch");
    }
    if !script.contains(INIT_SWITCH_ROOT) {
        bail!("BlissOS initrd /init does not contain the expected switch_root anchor");
    }
    let patch = format!(
        r#"# {ADB_PATCH_MARKER}
if [ -f system/etc/init/hw/init.usb.configfs.rc ]; then
    mkdir -p data/.android-simulator
    if [ -f system/etc/prop.default ]; then
        sed -e 's/^ro.adb.secure=.*/ro.adb.secure=0/' \
            -e 's/^ro.debuggable=.*/ro.debuggable=1/' \
            system/etc/prop.default > data/.android-simulator/prop.default
        grep -q '^service.adb.tcp.port=' data/.android-simulator/prop.default || \
            echo 'service.adb.tcp.port=5555' >> data/.android-simulator/prop.default
        chmod 0644 data/.android-simulator/prop.default
        mount --bind data/.android-simulator/prop.default system/etc/prop.default
    fi
    cat system/etc/init/hw/init.usb.configfs.rc > data/.android-simulator/adb.rc
    cat >> data/.android-simulator/adb.rc <<'ANDROID_SIMULATOR_ADB_RC'

# Android Simulator loopback-only TCP transport. QEMU exposes this guest port
# exclusively through hostfwd=tcp:127.0.0.1:15555-:5555.
on early-init
    setprop debug.ui.default_gralloc 4
    start vendor.graphics.allocator-4-0-arcvm
    setprop service.adb.tcp.port 5555

on post-fs-data
    setprop service.adb.tcp.port 5555
    setprop persist.adb.tcp.port 5555
    start adbd

on property:apexd.status=ready
    exec -- /system/bin/sh -c "sed -i '\|^namespace[.]sphal[.]search[.]paths += /system/.*/arm|d' /linkerconfig/com.android.media.swcodec/ld.config.txt"
    setprop service.adb.tcp.port 5555
    start adbd

on property:sys.boot_completed=1
    exec -- /system/bin/ip address replace 10.0.2.15/24 dev wifi_eth
    exec -- /system/bin/ip route replace default via 10.0.2.2 dev wifi_eth onlink
    exec -- /system/bin/ip rule add pref 18000 from all lookup main
    setprop service.adb.tcp.port 5555
    setprop persist.adb.tcp.port 5555
    start adbd
ANDROID_SIMULATOR_ADB_RC
    chmod 0644 data/.android-simulator/adb.rc
    if mount --bind data/.android-simulator/adb.rc system/etc/init/hw/init.usb.configfs.rc; then
        echo bound > data/.android-simulator/adb-bind-status
    else
        echo failed > data/.android-simulator/adb-bind-status
    fi
fi

"#
    );
    Ok(script.replacen(INIT_SWITCH_ROOT, &(patch + INIT_SWITCH_ROOT), 1))
}

#[derive(Debug)]
struct NewcEntry {
    magic: [u8; 6],
    fields: [u32; 13],
    name: Vec<u8>,
    data: Vec<u8>,
}

fn parse_newc(bytes: &[u8]) -> Result<Vec<NewcEntry>> {
    let mut offset = 0_usize;
    let mut entries = Vec::new();
    loop {
        let header = bytes
            .get(offset..offset + 110)
            .ok_or_else(|| anyhow!("truncated newc header at offset {offset}"))?;
        let magic: [u8; 6] = header[..6].try_into().unwrap();
        if &magic != b"070701" && &magic != b"070702" {
            bail!("unsupported initrd cpio magic at offset {offset}");
        }
        let mut fields = [0_u32; 13];
        for (index, field) in fields.iter_mut().enumerate() {
            let start = 6 + index * 8;
            let text = std::str::from_utf8(&header[start..start + 8])?;
            *field = u32::from_str_radix(text, 16)?;
        }
        offset += 110;
        let name_size = usize::try_from(fields[11])?;
        if name_size == 0 {
            bail!("newc entry has an empty name at offset {offset}");
        }
        let name_with_nul = bytes
            .get(offset..offset + name_size)
            .ok_or_else(|| anyhow!("truncated newc name at offset {offset}"))?;
        if name_with_nul.last() != Some(&0) {
            bail!("newc entry name is not NUL terminated at offset {offset}");
        }
        let name = name_with_nul[..name_size - 1].to_vec();
        offset = align4(offset + name_size);
        let data_size = usize::try_from(fields[6])?;
        let data = bytes
            .get(offset..offset + data_size)
            .ok_or_else(|| anyhow!("truncated newc data at offset {offset}"))?
            .to_vec();
        offset = align4(offset + data_size);
        let trailer = name == b"TRAILER!!!";
        entries.push(NewcEntry {
            magic,
            fields,
            name,
            data,
        });
        if trailer {
            break;
        }
    }
    Ok(entries)
}

fn write_newc(entries: &[NewcEntry]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for entry in entries {
        let mut fields = entry.fields;
        fields[6] = u32::try_from(entry.data.len())?;
        fields[11] = u32::try_from(entry.name.len() + 1)?;
        fields[12] = if &entry.magic == b"070702" {
            entry.data.iter().map(|byte| u32::from(*byte)).sum()
        } else {
            0
        };
        output.extend_from_slice(&entry.magic);
        for field in fields {
            write!(&mut output, "{field:08x}")?;
        }
        output.extend_from_slice(&entry.name);
        output.push(0);
        pad4(&mut output);
        output.extend_from_slice(&entry.data);
        pad4(&mut output);
    }
    Ok(output)
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn pad4(bytes: &mut Vec<u8>) {
    bytes.resize(align4(bytes.len()), 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_init_script_injects_loopback_adb_without_duplicate_patch() {
        let original = "before\nexec ${SWITCH:-switch_root} /android /init\nafter\n";

        let patched = patch_init_script(original).unwrap();

        assert!(patched.contains("service.adb.tcp.port 5555"));
        assert!(patched.contains("on property:apexd.status=ready"));
        assert!(patched.contains("on property:sys.boot_completed=1"));
        assert!(patched.contains("ip address replace 10.0.2.15/24 dev wifi_eth"));
        assert!(patched.contains("ip rule add pref 18000 from all lookup main"));
        assert!(patched.contains("com.android.media.swcodec/ld.config.txt"));
        assert!(patched.contains("namespace[.]sphal[.]search[.]paths"));
        assert!(patched.contains("cat system/etc/init/hw/init.usb.configfs.rc"));
        assert!(patched.contains("adb-bind-status"));
        assert!(patched.contains("ro.adb.secure=0"));
        assert_eq!(patched.matches(ADB_PATCH_MARKER).count(), 1);
        assert!(patch_init_script(&patched).is_err());
    }

    #[test]
    fn test_bliss_runtime_contract_is_android_13_and_sha256_pinned() {
        assert_eq!(IMAGE_ANDROID_API, 33);
        assert_eq!(IMAGE_VERSION, "16.9.7");
        assert!(IMAGE_FILE_NAME.starts_with("Bliss-v16.9.7-x86_64-OFFICIAL-foss-"));
        assert_eq!(IMAGE_SHA256.len(), 64);
        assert!(IMAGE_URL.starts_with("https://downloads.sourceforge.net/project/blissos-x86/"));
    }

    #[test]
    fn test_newc_roundtrip_preserves_entry_and_updates_payload_size() {
        let entries = vec![
            NewcEntry {
                magic: *b"070701",
                fields: [1, 0o100755, 0, 0, 1, 0, 3, 0, 0, 0, 0, 5, 0],
                name: b"init".to_vec(),
                data: b"new payload".to_vec(),
            },
            NewcEntry {
                magic: *b"070701",
                fields: [0; 13],
                name: b"TRAILER!!!".to_vec(),
                data: Vec::new(),
            },
        ];

        let encoded = write_newc(&entries).unwrap();
        let parsed = parse_newc(&encoded).unwrap();

        assert_eq!(parsed[0].name, b"init");
        assert_eq!(parsed[0].data, b"new payload");
        assert_eq!(parsed[0].fields[6], 11);
        assert_eq!(parsed[1].name, b"TRAILER!!!");
    }
}
