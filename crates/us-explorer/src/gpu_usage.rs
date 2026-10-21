//! GPU utilisation for the debug overlay, sampled every two seconds on a background
//! thread: the kernel's `gpu_busy_percent` for AMD (and recent Intel) on Linux, or
//! `nvidia-smi` for NVIDIA cards on Linux and Windows.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

const UNKNOWN: u32 = u32::MAX;
static BUSY: AtomicU32 = AtomicU32::new(UNKNOWN);

/// Latest GPU busy percentage, if any source is available.
pub fn percent() -> Option<u32> {
    static START: OnceLock<()> = OnceLock::new();
    START.get_or_init(|| {
        let _ = std::thread::Builder::new().name("gpu-usage".into()).spawn(sample_loop);
    });
    let v = BUSY.load(Ordering::Relaxed);
    (v != UNKNOWN).then_some(v)
}

fn sample_loop() {
    let mut nvidia_ok = true;
    loop {
        let v = sysfs().or_else(|| if nvidia_ok { nvidia() } else { None });
        if v.is_none() {
            // Don't keep spawning a missing tool.
            nvidia_ok = false;
        }
        BUSY.store(v.unwrap_or(UNKNOWN), Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(target_os = "linux")]
fn sysfs() -> Option<u32> {
    let mut best: Option<u32> = None;
    for e in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(e.path().join("device/gpu_busy_percent"))
            && let Ok(v) = s.trim().parse::<u32>()
        {
            best = Some(best.map_or(v, |b: u32| b.max(v)));
        }
    }
    best
}

#[cfg(not(target_os = "linux"))]
fn sysfs() -> Option<u32> {
    None
}

fn nvidia() -> Option<u32> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()).max()
}
