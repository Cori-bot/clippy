//! Bounded, daemon-owned exports. Client disconnection never abandons a write.
use crate::state::{Daemon, Shaping};
use anyhow::{Result, bail};
use clippy_core::export::{ExportCancelled, ExportControl, ExportStage};
use clippy_proto::{
    ClipMeta,
    command::{ExportOutput, ExportTrack},
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const HISTORY: usize = 16;
static NEXT: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone, PartialEq, Eq)]
struct ExportRequest {
    clip_id: String, start_ms: u64, end_ms: u64, mode: String,
    output: ExportOutput, tracks: Option<Vec<ExportTrack>>, shaping: Shaping,
}
struct Job {
    id: String,
    request_token: Option<String>,
    request: Option<ExportRequest>,
    state: State,
    control: Arc<ExportControl>,
    clip: Option<ClipMeta>,
    error: Option<String>,
}
impl Job {
    fn active(&self) -> bool {
        matches!(self.state, State::Queued | State::Running)
    }
    fn snapshot(&self) -> Result<Value> {
        let progress = self.control.progress();
        let state = match self.state {
            State::Queued | State::Running if self.control.is_cancelled() => "cancelling",
            State::Queued => "queued",
            State::Running => "running",
            State::Completed => "completed",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        };
        let stage = match progress.stage {
            ExportStage::Indexing => "indexing",
            ExportStage::Decoding => "decoding",
            ExportStage::Mixing => "mixing",
            ExportStage::Writing => "writing",
            ExportStage::Finalizing => "finalizing",
        };
        let mut value = json!({"job_id": self.id, "state": state, "percent": if self.state == State::Completed {100} else {progress.percent}, "stage": stage});
        if let Some(token) = &self.request_token { value["request_token"] = json!(token); }
        if let Some(clip) = &self.clip {
            value["clip"] = serde_json::to_value(clip)?;
        }
        if let Some(error) = &self.error {
            value["error"] = json!(error);
        }
        Ok(value)
    }
}
#[derive(Default)]
pub struct ExportJobs {
    jobs: Mutex<VecDeque<Job>>,
}
impl ExportJobs {
    #[cfg(test)]
    fn reserve(&self) -> Result<(String, Arc<ExportControl>, Value)> { self.reserve_for(None, None) }
    fn matching_request(&self, token: Option<&str>, request: &ExportRequest) -> Result<Option<Value>> {
        let Some(token) = token else { return Ok(None) };
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = jobs.iter().find(|job| job.request_token.as_deref() == Some(token)) else { return Ok(None) };
        if job.request.as_ref() != Some(request) { bail!("ce request_token correspond déjà à un autre export"); }
        job.snapshot().map(Some)
    }
    fn reserve_for(&self, request_token: Option<String>, request: Option<ExportRequest>) -> Result<(String, Arc<ExportControl>, Value)> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.iter().any(Job::active) {
            bail!("un export est déjà en cours");
        }
        while jobs.len() >= HISTORY {
            jobs.pop_front();
        }
        let id = format!(
            "{:x}-{:x}-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let control = Arc::new(ExportControl::default());
        let job = Job {
            id: id.clone(),
            request_token,
            request,
            state: State::Queued,
            control: control.clone(),
            clip: None,
            error: None,
        };
        let snapshot = job.snapshot()?;
        jobs.push_back(job);
        Ok((id, control, snapshot))
    }
    fn running(&self, id: &str) {
        if let Some(job) =
            self.jobs.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().find(|j| j.id == id)
        {
            job.state = State::Running;
        }
    }
    fn finish(&self, id: &str, result: Result<ClipMeta>) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            match result {
                Ok(clip) => {
                    job.state = State::Completed;
                    job.clip = Some(clip);
                }
                Err(error) if error.is::<ExportCancelled>() => {
                    job.state = State::Cancelled;
                }
                Err(error) => {
                    job.state = State::Failed;
                    job.error = Some(format!("{error:#}"));
                }
            }
        }
    }
    pub fn status(&self, id: Option<&str>) -> Result<Option<Value>> { self.status_for(id, None) }
    pub fn status_for(&self, id: Option<&str>, token: Option<&str>) -> Result<Option<Value>> {
        if id.is_some() && token.is_some() { bail!("choisissez job_id ou request_token"); }
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = token { return jobs.iter().find(|job| job.request_token.as_deref() == Some(token)).map(Job::snapshot).transpose(); }
        let job = match id {
            Some(id) => Some(
                jobs.iter()
                    .find(|j| j.id == id)
                    .ok_or_else(|| anyhow::anyhow!("export inconnu : {id}"))?,
            ),
            None => jobs.iter().find(|j| j.active()).or_else(|| jobs.back()),
        };
        job.map(Job::snapshot).transpose()
    }
    pub fn cancel(&self, id: &str) -> Result<Value> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| anyhow::anyhow!("export inconnu : {id}"))?;
        if job.active() {
            job.control.cancel();
        }
        job.snapshot()
    }
}
pub(crate) struct ExportPermit(Arc<AtomicBool>);
impl Drop for ExportPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Daemon {
    /// Config is acquired before claiming activity, matching set_config's order.
    /// No scheduler lock is held while acquiring config or processing media.
    pub(crate) fn acquire_export_permit(&self) -> Result<ExportPermit> {
        let _config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .export_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            bail!("un export est déjà en cours");
        }
        Ok(ExportPermit(self.export_active.clone()))
    }

    pub(crate) fn start_export(
        self: &Arc<Self>,
        clip_id: String,
        start_ms: u64,
        end_ms: u64,
        mode: String,
        output: ExportOutput,
        tracks: Option<Vec<ExportTrack>>,
        shaping: Shaping,
        request_token: Option<String>,
    ) -> Result<Value> {
        let request = ExportRequest { clip_id: clip_id.clone(), start_ms, end_ms, mode: mode.clone(), output, tracks: tracks.clone(), shaping };
        if let Some(existing) = self.export_jobs.matching_request(request_token.as_deref(), &request)? { return Ok(existing); }
        let permit = match self.acquire_export_permit() {
            Ok(permit) => permit,
            Err(error) => return match self.export_jobs.matching_request(request_token.as_deref(), &request)? {
                Some(existing) => Ok(existing), None => Err(error),
            },
        };
        // Recheck after acquiring the permit: an earlier worker may have completed
        // between lookup and acquisition. Replays must never create a second copy.
        if let Some(existing) = self.export_jobs.matching_request(request_token.as_deref(), &request)? { return Ok(existing); }
        self.validate_export_request(&clip_id, start_ms, end_ms, &mode, output, shaping)?;
        let (id, control, snapshot) = self.export_jobs.reserve_for(request_token, Some(request))?;
        let daemon = self.clone();
        let worker_id = id.clone();
        let spawn = std::thread::Builder::new().name("clip-export".into()).spawn(move || {
            let _permit = permit;
            daemon.export_jobs.running(&worker_id);
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<ClipMeta> {
                    if control.is_cancelled() {
                        return Err(ExportCancelled.into());
                    }
                    let _apartment = crate::com::Apartment::enter()?;
                    use windows::Win32::Media::MediaFoundation::{
                        MF_VERSION, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
                    };
                    // SAFETY: this fresh worker owns a live COM apartment. The
                    // guard below balances exactly this successful startup.
                    unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }?;
                    struct Media;
                    impl Drop for Media {
                        fn drop(&mut self) {
                            // SAFETY: Media exists only after successful startup;
                            // all reader/writer locals have already been dropped.
                            if let Err(error) = unsafe { MFShutdown() } {
                                tracing::warn!(%error, "export MFShutdown failed");
                            }
                        }
                    }
                    let _media = Media;
                    daemon.export_clip_with_control(
                        &clip_id,
                        start_ms,
                        end_ms,
                        &mode,
                        output,
                        tracks.as_deref(),
                        shaping,
                        &control,
                    )
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("le processus d’export a échoué")));
            daemon.export_jobs.finish(&worker_id, result);
        });
        if let Err(error) = spawn {
            self.export_jobs.finish(&id, Err(error.into()));
            return self
                .export_jobs
                .status(Some(&id))?
                .ok_or_else(|| anyhow::anyhow!("export réservé introuvable : {id}"));
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> ExportRequest {
        ExportRequest {clip_id:"20261026_120000".into(),start_ms:0,end_ms:1000,mode:"fast".into(),output:ExportOutput::Share,tracks:None,shaping:Shaping::default()}
    }
    #[test]
    fn token_lookup_never_returns_unrelated_latest_job() {
        let jobs = ExportJobs::default();
        let (id, _, _) = jobs.reserve_for(Some("older".into()), Some(request())).unwrap();
        jobs.finish(&id, Err(anyhow::anyhow!("old failure")));
        assert!(jobs.status_for(None, Some("new-request")).unwrap().is_none());
        assert_eq!(jobs.status(None).unwrap().unwrap()["job_id"], id);
        assert!(jobs.status_for(Some(&id), Some("older")).is_err());
    }
    #[test]
    fn repeated_token_is_idempotent_for_active_and_terminal_jobs_and_rejects_changed_args() {
        use clippy_core::config::Config;
        let root = std::env::temp_dir().join(format!("clippy-token-job-{}", std::process::id()));
        let daemon = Arc::new(Daemon::new_at(Config {clip_dir:root.to_string_lossy().into_owned(), ..Default::default()}, None));
        let requested = request();
        let (id, _, _) = daemon.export_jobs.reserve_for(Some("retry-token".into()), Some(requested.clone())).unwrap();
        let invoke = |end| daemon.start_export(requested.clip_id.clone(),requested.start_ms,end,requested.mode.clone(),requested.output,requested.tracks.clone(),Shaping::default(),Some("retry-token".into()));
        assert_eq!(invoke(1000).unwrap()["job_id"], id);
        assert!(invoke(999).is_err());
        daemon.export_jobs.finish(&id, Err(anyhow::anyhow!("terminal")));
        assert_eq!(invoke(1000).unwrap()["state"], "failed");
        assert_eq!(daemon.export_jobs.jobs.lock().unwrap().len(),1);
        let refused = daemon.start_export("../invalid".into(),0,1000,"fast".into(),ExportOutput::Share,None,Shaping::default(),Some("refused-token".into()));
        assert!(refused.is_err());
        assert!(daemon.export_jobs.status_for(None,Some("refused-token")).unwrap().is_none());
        assert_eq!(daemon.export_jobs.status(None).unwrap().unwrap()["job_id"],id);
        assert_eq!(daemon.export_jobs.status_for(None,Some("retry-token")).unwrap().unwrap()["request_token"],"retry-token");
    }
    #[test]
    #[ignore = "requires CLIPPY_TEST_CLIP synthetic MP4 fixture and native Media Foundation"]
    fn native_async_share_job_finishes_and_publishes_one_audio_track() {
        use crate::pipe::ClientHandler;
        use clippy_core::{config::Config, library};
        for mode in ["fast", "precise"] {
        let fixture = std::env::var_os("CLIPPY_TEST_CLIP").expect("CLIPPY_TEST_CLIP fixture path");
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("clippy-native-job-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let id = "20261026_120000";
        std::fs::copy(fixture, library::mp4_path(&root, id)).unwrap();
        let meta = ClipMeta {
            id: id.into(),
            title: "Synthetic export fixture".into(),
            created: library::now_rfc3339_local(),
            duration_ms: 3000,
            bytes: std::fs::metadata(library::mp4_path(&root, id)).unwrap().len(),
            width: 160,
            height: 96,
            fps: 10,
            encoder: "fixture".into(),
            has_audio: true,
            favorite: false,
            audio_tracks: vec!["Mix".into(), "Game".into(), "Mic".into()],
            game: String::new(),
            folder: String::new(),
        };
        library::write_sidecar(&root, &meta).unwrap();
        let daemon = Arc::new(Daemon::new_at(
            Config { clip_dir: root.to_string_lossy().into_owned(), ..Default::default() },
            None,
        ));
        let (start, end) = if mode == "precise" { (550, 650) } else { (0, 2500) };
        let request: clippy_proto::Request = serde_json::from_value(json!({"id":1,"cmd":"library.export.start","request_token":"native-share-test","clip_id":id,"start_ms":start,"end_ms":end,"mode":mode,"output":"share"})).unwrap();
        let started = daemon.dispatch(1, &request);
        assert!(started.ok, "{:?}", started.error);
        let job = started.data.unwrap()["job_id"].as_str().unwrap().to_string();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let completed = loop {
            let snapshot = daemon.export_jobs.status(Some(&job)).unwrap().unwrap();
            if matches!(snapshot["state"].as_str(), Some("completed" | "failed" | "cancelled")) {
                break snapshot;
            }
            assert!(std::time::Instant::now() < deadline, "export timed out: {snapshot}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(completed["state"], "completed", "{completed}");
        assert_eq!(completed["percent"], 100);
        let retry = daemon.dispatch(1, &request);
        assert!(retry.ok, "{:?}", retry.error);
        assert_eq!(retry.data.unwrap()["job_id"], job);
        assert_eq!(daemon.library.lock().unwrap().len(), 2, "one source and one export, even after retry");
        let exported: ClipMeta = serde_json::from_value(completed["clip"].clone()).unwrap();
        assert!(exported.has_audio);
        assert_eq!(exported.audio_tracks, vec!["Mix"]);
        let _apartment = crate::com::Apartment::enter().unwrap();
        use windows::Win32::Media::MediaFoundation::{
            MF_VERSION, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
        };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.unwrap();
        let count = clippy_core::export::audio_track_count(
            library::mp4_path(&root, &exported.id).as_path(),
        );
        if mode == "precise" {
            let frames = clippy_core::export::frame_index(&library::mp4_path(&root, &exported.id)).unwrap();
            assert_eq!(frames.timestamps_100ns, vec![0]);
            assert_eq!(frames.end_100ns, 1_000_000);
            assert_eq!(exported.duration_ms, 100);
        }
        unsafe { MFShutdown() }.unwrap();
        assert_eq!(count.unwrap(), 1);
        assert!(daemon.library.lock().unwrap().iter().any(|clip| clip.id == exported.id));
        }
    }
    #[test]
    fn concurrent_reservations_admit_only_one_worker() {
        let jobs = Arc::new(ExportJobs::default());
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let jobs = jobs.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    jobs.reserve().is_ok()
                })
            })
            .collect();
        assert_eq!(
            threads.into_iter().map(|thread| thread.join().unwrap()).filter(|won| *won).count(),
            1
        );
    }
    #[test]
    fn shared_permit_blocks_sync_and_folder_move_but_allows_same_folder() {
        let root =
            std::env::temp_dir().join(format!("clippy-export-permit-{}", std::process::id()));
        let config = clippy_core::config::Config {
            clip_dir: root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let daemon = Daemon::new_at(config, Some(root.join("config.toml")));
        let permit = daemon.acquire_export_permit().unwrap();
        assert!(daemon.acquire_export_permit().is_err());
        let moved =
            serde_json::from_value(json!({"clip_dir":root.join("different").to_string_lossy()}))
                .unwrap();
        assert!(
            format!("{:#}", daemon.set_config(&moved).unwrap_err()).contains("attendez la fin")
        );
        let same = serde_json::from_value(json!({"clip_dir":root.to_string_lossy()})).unwrap();
        assert!(daemon.set_config(&same).is_ok());
        drop(permit);
        assert!(daemon.acquire_export_permit().is_ok());
    }
    #[test]
    fn reservation_is_exclusive_until_worker_acknowledges_cancel() {
        let jobs = ExportJobs::default();
        let (id, control, _) = jobs.reserve().unwrap();
        assert!(jobs.reserve().is_err());
        assert_eq!(jobs.cancel(&id).unwrap()["state"], "cancelling");
        assert!(control.is_cancelled());
        assert!(jobs.reserve().is_err());
        jobs.finish(&id, Err(ExportCancelled.into()));
        assert_eq!(jobs.status(None).unwrap().unwrap()["state"], "cancelled");
        assert!(jobs.reserve().is_ok());
    }
    #[test]
    fn history_is_bounded_and_unknown_jobs_fail() {
        let jobs = ExportJobs::default();
        let (first, _, _) = jobs.reserve().unwrap();
        jobs.finish(&first, Err(anyhow::anyhow!("first failure")));
        for _ in 0..HISTORY {
            let (id, _, _) = jobs.reserve().unwrap();
            jobs.running(&id);
            jobs.finish(&id, Err(anyhow::anyhow!("failure")));
        }
        assert_eq!(jobs.jobs.lock().unwrap().len(), HISTORY);
        assert!(jobs.status(Some(&first)).is_err());
        assert!(jobs.cancel("missing").is_err());
        let latest = jobs.status(None).unwrap().unwrap();
        assert_eq!(latest["state"], "failed");
        assert_eq!(latest["error"], "failure");
        assert_eq!(jobs.cancel(latest["job_id"].as_str().unwrap()).unwrap()["state"], "failed");
    }
}
