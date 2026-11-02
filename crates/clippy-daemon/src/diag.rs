//! Diagnostics mode: everything needed to understand a test session without
//! asking the tester to describe it.
//!
//! Off unless `diagnostics = true` in the config (Settings → Application).
//! When on, it writes under `%APPDATA%\clippy\logs`:
//!
//! - `daemon-YYYYMMDD.log`: the daemon's own log at debug level, whatever the
//!   console filter is, plus a line for any panic before the process aborts.
//! - `metrics-YYYYMMDD.jsonl`: one line every [`INTERVAL`] with CPU, memory,
//!   handles and GPU use for the daemon, the window (clippy-ui and its
//!   WebView2 processes) and whatever is in the foreground (the game), the
//!   whole PC's CPU, memory, GPU and disk, the capture engine's counters and
//!   the daemon's status.
//! - `system-YYYYMMDD_HHMMSS.json`: Windows build, CPU, RAM, GPUs and their
//!   drivers, displays and the config, written each time diagnostics starts.
//! - `ui-YYYYMMDD.jsonl`: written by the window itself (see clippy-ui's
//!   `diag_append`), not here.
//!
//! Files older than [`KEEP_DAYS`] are deleted at startup. Nothing leaves the
//! PC.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
/// Timestamps of the diagnostics log in local time, like the file names and
/// the clips: a UTC log had 22:18 clips next to 21:18 lines.
pub struct LocalTimer;

impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        // SAFETY: no arguments; returns the local wall-clock time by value.
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        write!(
            w,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
}

use windows::Win32::System::SystemInformation::{
    GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
};
use windows::Win32::System::Threading::{
    GetProcessHandleCount, GetProcessTimes, GetSystemTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::state::Daemon;

pub const INTERVAL: Duration = Duration::from_secs(2);
pub const KEEP_DAYS: u64 = 14;
/// A file stops growing past this; a runaway log never fills the disk.
pub const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Called once the config is loaded, so the startup lines reach the file.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `%APPDATA%\clippy\logs`, beside the config.
pub fn logs_dir() -> Option<PathBuf> {
    Some(clippy_core::config::Config::path()?.parent()?.join("logs"))
}

fn today() -> String {
    clippy_core::library::now_id_stamp()[..8].to_string()
}

/// Appends to `<prefix>-YYYYMMDD.<ext>`, reopening when the day changes.
struct Open {
    day: String,
    file: Option<File>,
    written: u64,
    /// When opening last failed, so a read-only folder is retried every few
    /// seconds rather than on every line.
    failed_at: Option<Instant>,
}

struct DailyFile {
    prefix: &'static str,
    ext: &'static str,
    open: Mutex<Option<Open>>,
}

impl DailyFile {
    const fn new(prefix: &'static str, ext: &'static str) -> Self {
        Self { prefix, ext, open: Mutex::new(None) }
    }

    fn write(&self, bytes: &[u8]) {
        let Ok(mut slot) = self.open.lock() else { return };
        self.write_locked(&mut slot, bytes);
    }

    /// For the panic hook: never waits more than ~100 ms for the lock (the
    /// panicking thread may be the one holding it), and writes through a
    /// poisoned one.
    fn write_best_effort(&self, bytes: &[u8]) {
        for _ in 0..100 {
            match self.open.try_lock() {
                Ok(mut slot) => return self.write_locked(&mut slot, bytes),
                Err(std::sync::TryLockError::Poisoned(p)) => {
                    return self.write_locked(&mut p.into_inner(), bytes);
                }
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    fn write_locked(&self, slot: &mut Option<Open>, bytes: &[u8]) {
        let day = today();
        let stale = slot.as_ref().is_none_or(|open| {
            open.day != day
                || (open.file.is_none()
                    && open.failed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(10)))
        });
        if stale {
            if slot.as_ref().is_some_and(|open| open.day != day) {
                prune();
            }
            let path = logs_dir().map(|dir| dir.join(format!("{}-{day}.{}", self.prefix, self.ext)));
            let file = path.as_ref().and_then(|path| {
                std::fs::create_dir_all(path.parent()?).ok()?;
                OpenOptions::new().create(true).append(true).open(path).ok()
            });
            let written = file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
            let failed_at = file.is_none().then(Instant::now);
            *slot = Some(Open { day, file, written, failed_at });
        }
        if let Some(open) = slot.as_mut()
            && open.written < MAX_FILE_BYTES
            && let Some(file) = open.file.as_mut()
        {
            // Diagnostics must never become the failure.
            if file.write_all(bytes).is_ok() {
                open.written += bytes.len() as u64;
            }
        }
    }
}

static DAEMON_LOG: DailyFile = DailyFile::new("daemon", "log");
static METRICS: DailyFile = DailyFile::new("metrics", "jsonl");

/// `tracing` writer for the daemon log: a no-op while diagnostics is off.
pub struct LogWriter;

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if enabled() {
            DAEMON_LOG.write(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Records a panic in the daemon log before `panic = "abort"` ends the
/// process, which is otherwise silent when no console is attached.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let line = format!(
            "{} PANIC thread={thread} {info}\n{}\n",
            clippy_core::library::now_id_stamp(),
            std::backtrace::Backtrace::force_capture()
        );
        // Written even when diagnostics is off: a crash is always worth a file.
        DAEMON_LOG.write_best_effort(line.as_bytes());
        previous(info);
    }));
}

/// Deletes log files older than [`KEEP_DAYS`].
pub fn prune() {
    let Some(dir) = logs_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let limit = Duration::from_secs(KEEP_DAYS * 24 * 3600);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > limit);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Starts the sampler. It follows the config: diagnostics starts and stops
/// within one [`INTERVAL`] of the setting changing.
pub fn spawn(daemon: Weak<Daemon>) {
    prune();
    let spawned = std::thread::Builder::new().name("clippy-diagnostics".into()).spawn(move || {
        let mut sampler = Sampler::default();
        let mut was = false;
        loop {
            let Some(daemon) = daemon.upgrade() else { return };
            let on = daemon.lock_config().diagnostics;
            ENABLED.store(on, Ordering::Relaxed);
            if on && !was {
                write_system_info(&daemon);
                tracing::info!(dir = ?logs_dir(), "diagnostics on");
            } else if !on && was {
                tracing::info!("diagnostics off");
            }
            was = on;
            if on {
                let mut line = sampler.sample(&daemon).to_string();
                line.push('\n');
                METRICS.write(line.as_bytes());
            }
            drop(daemon);
            std::thread::sleep(INTERVAL);
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "diagnostics thread not started");
    }
}

// ------------------------------------------------------------------ metrics

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

struct Process {
    pid: u32,
    parent: u32,
    exe: String,
}

fn processes() -> Vec<Process> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            out.push(Process {
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
                exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
            });
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    out
}

/// CPU time (100 ns), working set, private bytes and handles of one process.
fn process_counters(pid: u32) -> Option<(u64, u64, u64, u32)> {
    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user).is_ok();
        let mut memory = PROCESS_MEMORY_COUNTERS_EX {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let mem_ok = K32GetProcessMemoryInfo(handle, (&raw mut memory).cast(), memory.cb).as_bool();
        let mut handles = 0u32;
        let _ = GetProcessHandleCount(handle, &mut handles);
        let _ = CloseHandle(handle);
        if !times {
            return None;
        }
        let (ws, private) =
            if mem_ok { (memory.WorkingSetSize as u64, memory.PrivateUsage as u64) } else { (0, 0) };
        Some((ticks(kernel) + ticks(user), ws, private, handles))
    }
}

#[derive(Default)]
struct Sampler {
    at: Option<Instant>,
    system: Option<(u64, u64, u64)>,
    cpu: HashMap<u32, u64>,
    gpu: Option<gpu::Gpu>,
    gpu_tried: bool,
}

const MB: f64 = 1024.0 * 1024.0;

impl Sampler {
    fn sample(&mut self, daemon: &Daemon) -> Value {
        let now = Instant::now();
        let wall = self.at.map(|at| now.duration_since(at).as_secs_f64()).unwrap_or(0.0);
        self.at = Some(now);
        let cores = unsafe {
            let mut info = SYSTEM_INFO::default();
            GetSystemInfo(&mut info);
            info.dwNumberOfProcessors.max(1) as f64
        };

        let all = processes();
        let me = std::process::id();
        let foreground = unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
            pid
        };
        // The window: clippy-ui and everything under it (WebView2 browser,
        // renderer and GPU processes are children and grandchildren).
        let mut ui: Vec<u32> = all.iter().filter(|p| p.exe.eq_ignore_ascii_case("clippy-ui.exe")).map(|p| p.pid).collect();
        for _ in 0..3 {
            let more: Vec<u32> =
                all.iter().filter(|p| ui.contains(&p.parent) && !ui.contains(&p.pid)).map(|p| p.pid).collect();
            ui.extend(more);
        }

        let mut seen = HashMap::new();
        let mut group = |pids: &[u32]| -> Value {
            let (mut cpu, mut ws, mut private, mut handles) = (0.0, 0u64, 0u64, 0u32);
            for &pid in pids {
                if let Some((time, w, p, h)) = process_counters(pid) {
                    if let Some(before) = self.cpu.get(&pid).filter(|_| wall > 0.0) {
                        cpu += time.saturating_sub(*before) as f64 / 1e7 / wall / cores * 100.0;
                    }
                    seen.insert(pid, time);
                    ws += w;
                    private += p;
                    handles += h;
                }
            }
            json!({
                "cpu_pct": round1(cpu),
                "working_set_mb": round1(ws as f64 / MB),
                "private_mb": round1(private as f64 / MB),
                "handles": handles,
                "processes": pids.len(),
            })
        };
        let daemon_proc = group(&[me]);
        let ui_proc = group(&ui);
        let fg_name = all.iter().find(|p| p.pid == foreground).map(|p| p.exe.clone()).unwrap_or_default();
        let mut fg_proc = group(&[foreground]);
        self.cpu = seen;
        if let Some(fields) = fg_proc.as_object_mut() {
            fields.insert("exe".into(), Value::from(fg_name));
            fields.insert("pid".into(), Value::from(foreground));
        }

        // Whole PC.
        let system_cpu = unsafe {
            let (mut idle, mut kernel, mut user) = Default::default();
            GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok().and_then(|_| {
                let now = (ticks(idle), ticks(kernel), ticks(user));
                let before = self.system.replace(now)?;
                let busy = (now.1 - before.1) + (now.2 - before.2);
                let idle = now.0 - before.0;
                (busy > 0).then(|| round1((busy - idle.min(busy)) as f64 / busy as f64 * 100.0))
            })
        };
        let memory = unsafe {
            let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            GlobalMemoryStatusEx(&mut status).ok().map(|_| {
                json!({
                    "load_pct": status.dwMemoryLoad,
                    "total_mb": (status.ullTotalPhys as f64 / MB).round(),
                    "available_mb": (status.ullAvailPhys as f64 / MB).round(),
                    "commit_mb": ((status.ullTotalPageFile - status.ullAvailPageFile) as f64 / MB).round(),
                })
            })
        };

        if !self.gpu_tried {
            self.gpu_tried = true;
            self.gpu = gpu::Gpu::open();
        }
        let gpu = self.gpu.as_mut().map(|g| g.sample(me, foreground));

        let config = daemon.lock_config().clone();
        let disk_free_gb = free_gb(&config.clip_dir);
        drop(config);

        json!({
            "t": clippy_core::library::now_id_stamp(),
            "unix_ms": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            "daemon": daemon_proc,
            "ui": ui_proc,
            "foreground": fg_proc,
            "system": { "cpu_pct": system_cpu, "memory": memory, "clip_disk_free_gb": disk_free_gb },
            "gpu": gpu,
            "engine": daemon.stats_json(),
            "status": daemon.status().to_json(),
        })
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn free_gb(dir: &str) -> Option<f64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path = if dir.is_empty() {
        std::env::var_os("USERPROFILE").map(PathBuf::from)?
    } else {
        PathBuf::from(dir)
    };
    // The folder may not exist yet; its drive does.
    let root: PathBuf = path.ancestors().find(|p| p.exists())?.to_path_buf();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&windows::core::HSTRING::from(root.as_os_str()), Some(&mut free), None, None) }
        .ok()?;
    Some(round1(free as f64 / 1024.0 / MB))
}

/// GPU counters through PDH, the same source as Task Manager's GPU columns.
mod gpu {
    use serde_json::{Value, json};
    use windows::Win32::System::Performance::{
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
        PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };
    use windows::core::{PCWSTR, w};

    pub struct Gpu {
        query: PDH_HQUERY,
        engines: PDH_HCOUNTER,
        adapter_memory: PDH_HCOUNTER,
        process_memory: PDH_HCOUNTER,
    }

    // PDH handles are plain process-local handles, used from one thread.
    unsafe impl Send for Gpu {}

    impl Drop for Gpu {
        fn drop(&mut self) {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    fn counter(query: PDH_HQUERY, path: PCWSTR) -> Option<PDH_HCOUNTER> {
        let mut counter = PDH_HCOUNTER::default();
        (unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) } == 0).then_some(counter)
    }

    impl Gpu {
        pub fn open() -> Option<Self> {
            let mut query = PDH_HQUERY::default();
            if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
                return None;
            }
            let counters = (|| {
                Some((
                    counter(query, w!("\\GPU Engine(*)\\Utilization Percentage"))?,
                    counter(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"))?,
                    counter(query, w!("\\GPU Process Memory(*)\\Dedicated Usage"))?,
                ))
            })();
            let Some((engines, adapter_memory, process_memory)) = counters else {
                unsafe { PdhCloseQuery(query) };
                return None;
            };
            let gpu = Self { query, engines, adapter_memory, process_memory };
            unsafe { PdhCollectQueryData(query) };
            Some(gpu)
        }

        fn items(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
            let mut size = 0u32;
            let mut count = 0u32;
            unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None) };
            if size == 0 {
                return Vec::new();
            }
            // u64 storage keeps the item array aligned.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) } != 0 {
                return Vec::new();
            }
            (0..count as usize)
                .map(|i| unsafe {
                    let item = &*items.add(i);
                    (item.szName.to_string().unwrap_or_default(), item.FmtValue.Anonymous.doubleValue)
                })
                .collect()
        }

        pub(super) fn pid_of(instance: &str) -> Option<u32> {
            instance.strip_prefix("pid_")?.split('_').next()?.parse().ok()
        }

        pub fn sample(&mut self, me: u32, foreground: u32) -> Value {
            if unsafe { PdhCollectQueryData(self.query) } != 0 {
                return Value::Null;
            }
            // Engine utilisation, summed per engine type as Task Manager does
            // (it shows the busiest engine of each type; the sum per type is
            // what it reports per process).
            let mut by_type: std::collections::BTreeMap<String, f64> = Default::default();
            let (mut mine, mut fg) = (std::collections::BTreeMap::<String, f64>::new(), std::collections::BTreeMap::<String, f64>::new());
            for (name, value) in Self::items(self.engines) {
                let kind = name.rsplit("engtype_").next().unwrap_or("?").to_string();
                *by_type.entry(kind.clone()).or_default() += value;
                match Self::pid_of(&name) {
                    Some(pid) if pid == me => *mine.entry(kind).or_default() += value,
                    Some(pid) if pid == foreground => *fg.entry(kind).or_default() += value,
                    _ => {}
                }
            }
            let round = |m: std::collections::BTreeMap<String, f64>| -> Value {
                m.into_iter()
                    .filter(|(_, v)| *v >= 0.05)
                    .map(|(k, v)| (k, Value::from(super::round1(v.min(100.0)))))
                    .collect::<serde_json::Map<_, _>>()
                    .into()
            };
            let mb = |v: f64| super::round1(v / super::MB);
            let dedicated: f64 = Self::items(self.adapter_memory).iter().map(|(_, v)| v).sum();
            let process = Self::items(self.process_memory);
            let memory_of = |pid: u32| -> f64 {
                mb(process.iter().filter(|(n, _)| Self::pid_of(n) == Some(pid)).map(|(_, v)| v).sum())
            };
            json!({
                "engines_pct": round(by_type),
                "daemon_engines_pct": round(mine),
                "foreground_engines_pct": round(fg),
                "dedicated_used_mb": mb(dedicated),
                "daemon_dedicated_mb": memory_of(me),
                "foreground_dedicated_mb": memory_of(foreground),
            })
        }
    }
}

// --------------------------------------------------------------- system info

fn registry_string(key: PCWSTRish, value: PCWSTRish) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut buffer = [0u16; 512];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size))
    }
    .ok()
    .ok()?;
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]).trim().to_string())
}

fn registry_dword(key: PCWSTRish, value: PCWSTRish) -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_DWORD, None, Some((&raw mut data).cast()), Some(&mut size))
    }
    .ok()
    .ok()?;
    Some(data)
}

type PCWSTRish = windows::core::PCWSTR;

fn gpus() -> Vec<Value> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1};
    use windows::core::Interface as _;
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return Vec::new() };
    let mut out = Vec::new();
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else { break };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok().map(|v| {
            let v = v as u64;
            format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xFFFF, (v >> 16) & 0xFFFF, v & 0xFFFF)
        });
        out.push(json!({
            "name": String::from_utf16_lossy(&desc.Description[..len]),
            "vendor_id": format!("{:04x}", desc.VendorId),
            "device_id": format!("{:04x}", desc.DeviceId),
            "dedicated_mb": (desc.DedicatedVideoMemory as f64 / MB).round(),
            "shared_mb": (desc.SharedSystemMemory as f64 / MB).round(),
            "software": desc.Flags & 2 != 0,
            "driver": driver,
        }));
    }
    out
}

fn displays() -> Vec<Value> {
    use windows::Win32::Graphics::Gdi::{
        DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW, EnumDisplaySettingsW,
    };
    let mut out = Vec::new();
    for index in 0..16 {
        let mut device = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(None, index, &mut device, 0) }.as_bool() {
            break;
        }
        let name_len = device.DeviceName.iter().position(|&c| c == 0).unwrap_or(32);
        let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        let active = unsafe {
            EnumDisplaySettingsW(windows::core::PCWSTR(device.DeviceName.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode)
        }
        .as_bool();
        if !active {
            continue;
        }
        let adapter_len = device.DeviceString.iter().position(|&c| c == 0).unwrap_or(128);
        out.push(json!({
            "name": String::from_utf16_lossy(&device.DeviceName[..name_len]),
            "adapter": String::from_utf16_lossy(&device.DeviceString[..adapter_len]),
            "width": mode.dmPelsWidth,
            "height": mode.dmPelsHeight,
            "hz": mode.dmDisplayFrequency,
            "bits": mode.dmBitsPerPel,
            "primary": device.StateFlags.0 & 4 != 0,
        }));
    }
    out
}

pub fn system_info(daemon: &Daemon) -> Value {
    use windows::core::w;
    let nt = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let (cores, memory) = unsafe {
        let mut info = SYSTEM_INFO::default();
        GetSystemInfo(&mut info);
        let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let _ = GlobalMemoryStatusEx(&mut status);
        (info.dwNumberOfProcessors, (status.ullTotalPhys as f64 / MB).round())
    };
    let mut fields = Map::new();
    fields.insert("clippy_version".into(), Value::from(env!("CARGO_PKG_VERSION")));
    fields.insert(
        "windows".into(),
        json!({
            "product": registry_string(nt, w!("ProductName")),
            "display_version": registry_string(nt, w!("DisplayVersion")),
            "build": registry_string(nt, w!("CurrentBuild")),
            "ubr": registry_dword(nt, w!("UBR")),
        }),
    );
    fields.insert(
        "cpu".into(),
        json!({
            "name": registry_string(w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("ProcessorNameString")),
            "logical_cores": cores,
        }),
    );
    fields.insert("ram_mb".into(), Value::from(memory));
    fields.insert("gpus".into(), Value::from(gpus()));
    fields.insert("displays".into(), Value::from(displays()));
    fields.insert("config".into(), serde_json::to_value(&*daemon.lock_config()).unwrap_or(Value::Null));
    fields.insert("status".into(), daemon.status().to_json());
    Value::Object(fields)
}

fn write_system_info(daemon: &Daemon) {
    let Some(dir) = logs_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("system-{}.json", clippy_core::library::now_id_stamp()));
    let text = serde_json::to_string_pretty(&system_info(daemon)).unwrap_or_default();
    if let Err(error) = std::fs::write(&path, text) {
        tracing::warn!(%error, "could not write the system snapshot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sampler_reads_this_process_and_the_machine() {
        let me = std::process::id();
        let (cpu, ws, private, handles) = process_counters(me).expect("own process");
        assert!(ws > 1 << 20 && private > 0 && handles > 0, "{cpu} {ws} {private} {handles}");
        assert!(processes().iter().any(|p| p.pid == me));
        assert!(!gpus().is_empty(), "at least the basic render adapter");
    }

    #[test]
    fn gpu_instance_names_carry_the_pid() {
        assert_eq!(gpu::Gpu::pid_of("pid_1234_luid_0x0_phys_0_eng_0_engtype_3D"), Some(1234));
        assert_eq!(gpu::Gpu::pid_of("luid_0x0_phys_0"), None);
    }
}
