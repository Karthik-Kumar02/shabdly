//! Machine probes (spec section 2): RAM, macOS version, free disk, GPU, and the model they choose.
use shabdly_core::models::{DEFAULT_SPEED, ModelSpec, Os, best_offered, pick};
use shabdly_core::settings::{Settings, Speed};
use std::path::Path;
use std::process::Command;

const GB: u64 = 1 << 30;

/// Bytes to whole GB, rounded to the nearest (Windows reports a little under the installed amount).
pub fn parse_gb(bytes: u64) -> u64 {
    (bytes + GB / 2) / GB
}

/// "13.6.1" to (13, 6); anything unparsable is 0.
pub fn parse_macos_version(v: &str) -> (u32, u32) {
    let mut parts = v.trim().split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

pub(crate) fn run(program: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// Installed RAM in GB. `SHABDLY_RAM_GB` overrides the probe; 0 when the probe fails.
pub fn ram_gb() -> u64 {
    if let Some(gb) = std::env::var("SHABDLY_RAM_GB").ok().and_then(|v| v.trim().parse().ok()) {
        return gb;
    }
    parse_gb(total_ram_bytes())
}

#[cfg(target_os = "macos")]
fn total_ram_bytes() -> u64 {
    run(Path::new("/usr/sbin/sysctl"), &["-n", "hw.memsize"])
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(windows)]
fn total_ram_bytes() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain data; dwLength is set as the API requires.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 { m.ullTotalPhys } else { 0 }
    }
}

/// macOS (major, minor); (0, 0) elsewhere or when the probe fails.
pub fn macos_version() -> (u32, u32) {
    if cfg!(target_os = "macos") {
        run(Path::new("/usr/sbin/sysctl"), &["-n", "kern.osproductversion"])
            .map_or((0, 0), |v| parse_macos_version(&v))
    } else {
        (0, 0)
    }
}

pub fn os() -> Os {
    if cfg!(target_os = "macos") { Os::Mac } else { Os::Windows }
}

/// Free space on the volume holding `dir`; 0 when unknown, so a download is refused rather than
/// started on a full disk.
pub fn free_bytes(dir: &Path) -> u64 {
    fs4::available_space(dir).unwrap_or(0)
}

/// Mac: always (Metal). Windows: the bundled llama-server lists a Vulkan device. Probed once:
/// starting llama-server takes a moment and the answer does not change while Shabdly runs.
pub fn has_gpu(engines_dir: &Path) -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    static GPU: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *GPU.get_or_init(|| {
        run(&engines_dir.join("win/llama/llama-server.exe"), &["--list-devices"]).is_some_and(|out| lists_vulkan_device(&out))
    })
}

/// A "Vulkan0: <name>" device line, not just the word: the backend's own log says "Vulkan" too.
fn lists_vulkan_device(out: &str) -> bool {
    out.lines().any(|l| l.trim_start().starts_with("Vulkan0:"))
}

/// The saved speed (or the default), dropped to Fast when this machine no longer qualifies for
/// Best, then the catalog pick. `None`: no bundled engine on this macOS version.
pub fn chosen_spec(s: &Settings, engines_dir: &Path) -> Option<&'static ModelSpec> {
    let mut speed = s.speed.unwrap_or(DEFAULT_SPEED);
    if speed == Speed::Best && !best_offered(ram_gb(), has_gpu(engines_dir)) {
        speed = Speed::Fast;
    }
    pick(os(), macos_version(), speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_listed_vulkan_device_counts() {
        assert!(lists_vulkan_device("Available devices:\n  Vulkan0: AMD Radeon(TM) Graphics (8192 MiB, 7900 MiB free)\n"));
        assert!(!lists_vulkan_device("load_backend: loaded Vulkan backend from ggml-vulkan.dll\nggml_vulkan: No devices found.\nAvailable devices:\n"));
    }

    #[test]
    fn gb_rounds_to_nearest() {
        assert_eq!(parse_gb(8_589_934_592), 8);
        assert_eq!(parse_gb(17_179_869_184), 16);
        // Windows reports a little under the installed amount.
        assert_eq!(parse_gb(16_850_000_000), 16);
    }

    #[test]
    fn macos_version_parses() {
        assert_eq!(parse_macos_version("13.6.1"), (13, 6));
        assert_eq!(parse_macos_version("27.0\n"), (27, 0));
        assert_eq!(parse_macos_version("14"), (14, 0));
        assert_eq!(parse_macos_version(""), (0, 0));
    }

    #[test]
    fn ram_override_wins() {
        // SAFETY: no other test in this crate reads or writes SHABDLY_RAM_GB.
        unsafe { std::env::set_var("SHABDLY_RAM_GB", "8") };
        assert_eq!(ram_gb(), 8);
    }
}
