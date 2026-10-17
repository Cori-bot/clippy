//! Recording single applications: finding them, listing the ones that make
//! sound, and keeping a capture open on each while it runs.
//!
//! Windows can hand out the sound of one process and everything it started
//! ("process loopback"). A browser or a music player is many processes with one
//! root, and its process ids change on every launch, so the user names an
//! executable (`spotify.exe`) and this module turns that into the root ids
//! that exist right now, again every second.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use wasapi::{DeviceEnumerator, Direction, SessionState};
use windows::Win32::{
    Foundation::CloseHandle,
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    },
};

use super::{
    AudioPacket,
    source::{AudioCapture, AudioSourceKind},
};

/// How often the running applications are looked up again. A music player
/// started mid-game is heard within about this long; shorter would cost a
/// process snapshot per tick for no audible difference.
const SCAN_EVERY: Duration = Duration::from_millis(1000);

/// Applications recorded at the same time, as a ceiling on open streams.
const MAX_RUNNING: usize = 16;

/// One running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent: u32,
    /// File name, lower-cased: what [`crate::config::Config::audio_apps`] holds.
    pub exe: String,
    /// File name without `.exe`, as Windows spells it, for showing to people.
    pub display: String,
}

/// An application that has an audio session on this PC.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AudioApp {
    /// The value to put in `audio_apps`.
    pub exe: String,
    /// What to show: the executable without its extension.
    pub name: String,
    /// True while it is producing sound at this moment.
    pub playing: bool,
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

fn entry(pid: u32, parent: u32, file_name: &str) -> ProcessEntry {
    let display = file_name
        .rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("exe"))
        .map_or(file_name, |(stem, _)| stem)
        .to_string();
    ProcessEntry { pid, parent, exe: file_name.to_lowercase(), display }
}

/// Every running process, from one toolhelp snapshot.
pub fn processes() -> Result<Vec<ProcessEntry>> {
    // SAFETY: `snapshot` is a handle this function creates and closes; the
    // entry struct is sized as the API requires before the first call.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .context("could not list the running processes")?;
        let mut item =
            PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut out = Vec::new();
        let mut more = Process32FirstW(snapshot, &mut item).is_ok();
        while more {
            out.push(entry(
                item.th32ProcessID,
                item.th32ParentProcessID,
                &wide_to_string(&item.szExeFile),
            ));
            more = Process32NextW(snapshot, &mut item).is_ok();
        }
        let _ = CloseHandle(snapshot);
        Ok(out)
    }
}

/// The process ids to capture for `wanted`: for each application the topmost
/// processes whose name matches, so a browser's dozens of helpers are covered
/// by their one root and not opened a second time.
///
/// A parent only counts if it is itself a match in this snapshot: ids are
/// reused, and a stale parent id pointing at an unrelated process must not hide
/// a real root.
pub fn matching_roots(processes: &[ProcessEntry], wanted: &[String]) -> Vec<u32> {
    let is_match = |p: &ProcessEntry| wanted.iter().any(|w| *w == p.exe);
    let matching: HashSet<u32> = processes.iter().filter(|p| is_match(p)).map(|p| p.pid).collect();
    processes
        .iter()
        .filter(|p| is_match(p) && !matching.contains(&p.parent))
        .map(|p| p.pid)
        .collect()
}

/// The applications that have an audio session on any playback device, sound
/// playing or not, one row per executable.
///
/// Runs on a thread of its own because it needs COM in the multithreaded
/// apartment and the caller's thread may already be in another.
pub fn list_audio_apps() -> Result<Vec<AudioApp>> {
    std::thread::Builder::new()
        .name("clippy-audio-apps".into())
        .spawn(list_on_this_thread)
        .context("failed to spawn the audio application lookup")?
        .join()
        .map_err(|_| anyhow!("the audio application lookup panicked"))?
}

fn list_on_this_thread() -> Result<Vec<AudioApp>> {
    wasapi::initialize_mta().ok().context("COM MTA init failed")?;
    let by_pid: HashMap<u32, ProcessEntry> = processes()?.into_iter().map(|p| (p.pid, p)).collect();
    let enumerator = DeviceEnumerator::new().map_err(|e| anyhow!("device enumerator: {e}"))?;
    let devices = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|e| anyhow!("playback devices: {e}"))?;

    let mut apps: HashMap<String, AudioApp> = HashMap::new();
    for device in &devices {
        let Ok(device) = device else { continue };
        let Ok(manager) = device.get_iaudiosessionmanager() else { continue };
        let Ok(sessions) = manager.get_audiosessionenumerator() else { continue };
        let Ok(count) = sessions.get_count() else { continue };
        for index in 0..count {
            let Ok(session) = sessions.get_session(index) else { continue };
            let Ok(state) = session.get_state() else { continue };
            if state == SessionState::Expired {
                continue;
            }
            // 0 is the "system sounds" session, which belongs to no program.
            let Ok(pid) = session.get_process_id() else { continue };
            // Toolhelp lists pid 0 as "[System Process]", so it has to be refused here.
            if pid == 0 {
                continue;
            }
            let Some(process) = by_pid.get(&pid) else { continue };
            // Recording Clippy's own sound is never what anyone wants.
            if process.exe.starts_with("clippy") {
                continue;
            }
            let app = apps.entry(process.exe.clone()).or_insert_with(|| AudioApp {
                exe: process.exe.clone(),
                name: process.display.clone(),
                playing: false,
            });
            app.playing |= state == SessionState::Active;
        }
    }
    let mut list: Vec<AudioApp> = apps.into_values().collect();
    list.sort_by(|a, b| {
        b.playing.cmp(&a.playing).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(list)
}

/// A capture that has just been opened for a running application. The mixer
/// adds it to the mix; `gone` is raised when the application quits.
pub struct NewApp {
    pub rx: Receiver<AudioPacket>,
    pub gone: Arc<AtomicBool>,
}

struct Running {
    capture: AudioCapture,
    gone: Arc<AtomicBool>,
}

impl Running {
    fn close(self) {
        self.gone.store(true, Ordering::Relaxed);
        if let Err(e) = self.capture.stop() {
            tracing::warn!(error = %format!("{e:#}"), "an application's sound capture ended with an error");
        }
    }
}

/// Keeps one capture open per running instance of the named applications.
///
/// Owns the captures, not the mixer: a process that quits leaves its stream
/// open and silent, so something has to notice and close it, and the thread
/// that does the looking is the one that knows.
pub struct AppWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    new_apps: Receiver<NewApp>,
}

impl AppWatcher {
    pub fn start(apps: Vec<String>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, new_apps) = channel();
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("clippy-audio-watch".into())
            .spawn(move || watch(&apps, &flag, &tx))
            .context("failed to spawn the application watcher")?;
        Ok(Self { stop, thread: Some(thread), new_apps })
    }

    /// The next capture opened since the last call, if any.
    pub fn next_new_app(&self) -> Option<NewApp> {
        self.new_apps.try_recv().ok()
    }

    /// Stops the watcher and every capture it opened. Idempotent.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AppWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn watch(apps: &[String], stop: &AtomicBool, tx: &Sender<NewApp>) {
    let mut running: HashMap<u32, Running> = HashMap::new();
    // Ids whose capture failed to open: not retried until the process is gone,
    // or a broken one would be reopened, and logged, every second.
    let mut failed: HashSet<u32> = HashSet::new();
    let mut next_scan = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if Instant::now() >= next_scan {
            next_scan = Instant::now() + SCAN_EVERY;
            match processes() {
                Ok(list) => {
                    let roots = matching_roots(&list, apps);
                    failed.retain(|pid| roots.contains(pid));

                    let ended: Vec<u32> = running
                        .iter()
                        .filter(|(pid, r)| !roots.contains(pid) || r.capture.is_finished())
                        .map(|(pid, _)| *pid)
                        .collect();
                    for pid in ended {
                        let Some(app) = running.remove(&pid) else { continue };
                        if roots.contains(&pid) {
                            // Its thread ended while the process lives: the stream
                            // could not be opened or broke. Say so once.
                            failed.insert(pid);
                            tracing::warn!(pid, "an application's sound could not be captured");
                        }
                        app.close();
                    }

                    for pid in roots {
                        if running.len() >= MAX_RUNNING {
                            break;
                        }
                        if running.contains_key(&pid) || failed.contains(&pid) {
                            continue;
                        }
                        match AudioCapture::start(AudioSourceKind::Application { pid }) {
                            Ok((capture, rx)) => {
                                let gone = Arc::new(AtomicBool::new(false));
                                let app = Running { capture, gone: Arc::clone(&gone) };
                                if tx.send(NewApp { rx, gone }).is_err() {
                                    app.close();
                                    return;
                                }
                                tracing::info!(pid, "recording the sound of an application");
                                running.insert(pid, app);
                            }
                            Err(e) => {
                                failed.insert(pid);
                                tracing::warn!(pid, error = %format!("{e:#}"), "could not start an application's sound capture");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "could not look up running applications")
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for (_, app) in running {
        app.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, exe: &str) -> ProcessEntry {
        entry(pid, parent, exe)
    }

    fn wanted(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_display_name_drops_the_extension_and_keeps_the_case() {
        let e = p(1, 0, "Spotify.exe");
        assert_eq!(e.exe, "spotify.exe");
        assert_eq!(e.display, "Spotify");
        assert_eq!(p(2, 0, "weird").display, "weird");
    }

    #[test]
    fn a_browser_with_many_helpers_is_one_root() {
        let list = [
            p(10, 1, "brave.exe"),
            p(11, 10, "brave.exe"),
            p(12, 10, "brave.exe"),
            p(13, 11, "brave.exe"),
            p(20, 1, "Spotify.exe"),
            p(21, 20, "Spotify.exe"),
            p(30, 1, "explorer.exe"),
        ];
        let mut roots = matching_roots(&list, &wanted(&["spotify.exe"]));
        roots.sort_unstable();
        assert_eq!(roots, vec![20]);
        let mut both = matching_roots(&list, &wanted(&["spotify.exe", "brave.exe"]));
        both.sort_unstable();
        assert_eq!(both, vec![10, 20]);
    }

    #[test]
    fn two_separate_instances_are_two_roots() {
        let list = [p(10, 1, "spotify.exe"), p(50, 2, "spotify.exe")];
        let mut roots = matching_roots(&list, &wanted(&["spotify.exe"]));
        roots.sort_unstable();
        assert_eq!(roots, vec![10, 50]);
    }

    #[test]
    fn a_reused_parent_id_does_not_hide_a_real_root() {
        // The parent died and its id now belongs to an unrelated process.
        let list = [p(10, 777, "spotify.exe"), p(777, 1, "notepad.exe")];
        assert_eq!(matching_roots(&list, &wanted(&["spotify.exe"])), vec![10]);
    }

    #[test]
    fn nothing_is_captured_when_the_application_is_not_running() {
        let list = [p(30, 1, "explorer.exe")];
        assert!(matching_roots(&list, &wanted(&["spotify.exe"])).is_empty());
        assert!(matching_roots(&list, &[]).is_empty());
    }

    /// Against the real machine: this very process must be in the snapshot,
    /// and the listing must not fail even when nothing is playing.
    #[test]
    fn the_snapshot_contains_this_process() {
        let me = std::process::id();
        assert!(processes().expect("snapshot").iter().any(|p| p.pid == me));
    }

    #[test]
    fn listing_audio_applications_does_not_fail() {
        list_audio_apps().expect("session enumeration");
    }
}
