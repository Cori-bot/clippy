//! Everything the webview is allowed to ask the Rust half to do.
//!
//! Three commands, and two of them are about the daemon being absent. There is
//! deliberately no per-protocol-command wrapper: `clippy_call` is a straight
//! pass-through, so a command added to the daemon is usable from the frontend
//! the same day without touching this file. The daemon validates its own
//! arguments and says so in words meant for a person; re-validating here would
//! only produce a second, worse message.

use std::sync::Arc;

use serde_json::{Map, Value};
use tauri::State;

use crate::daemon::Supervisor;

#[tauri::command]
pub async fn clippy_call(
    supervisor: State<'_, Arc<Supervisor>>,
    cmd: String,
    args: Map<String, Value>,
) -> Result<Value, String> {
    let supervisor = Arc::clone(&supervisor);
    // spawn_blocking, because `call` parks until the daemon answers and an
    // `arm` takes seconds: doing that on a runtime worker would stall every
    // other command behind it.
    //
    // A `config.set` used to be special-cased right here to re-grant the
    // asset scope on a successful `clip_dir` change — but that only ever ran
    // for a `config.set` this app itself issued, and missed the tray's
    // "Change clips folder…" entirely. The daemon now broadcasts
    // `config_changed` for every accepted `config.set` regardless of who sent
    // it, and `daemon.rs`'s `Connection::start` event closure grants off that
    // instead, so this is a plain pass-through again — one path responsible
    // for the grant, not two.
    tauri::async_runtime::spawn_blocking(move || supervisor.call(&cmd, args))
        .await
        .map_err(|e| format!("l’appel n’a pas pu être planifié\u{a0}: {e}"))?
}

#[tauri::command]
pub fn start_daemon(supervisor: State<'_, Arc<Supervisor>>) -> Result<(), String> {
    // Refused for the length of an install, and refused here rather than by
    // hiding the button. An update stops the daemon before it renames
    // `clippy-daemon.exe`, and the supervisor emits `clippy-disconnected` the
    // moment it does -- so for those few seconds the window is showing spec
    // §4.5's "not running" panel, whose entire content is an offer to start
    // the thing the updater just stopped. Taking that offer launches the old
    // daemon into the gap `update::install`'s `pipe_exists` guard just
    // cleared, and Windows lets a running executable be renamed: the swap
    // would report success and leave the previous build executing out of
    // `clippy-daemon.exe.old`, where the next launch's cleanup cannot delete it
    // either.
    //
    // The updater puts the recorder back itself when it was the thing that
    // stopped it and the install left a `clippy-daemon.exe` fit to run. It does
    // not always manage both -- an update that fails because the recorder will
    // not stop puts nothing back, and neither does one whose rollback strands
    // the daemon binary. What holds in every case is that this refusal lasts
    // no longer than the install does: the flag is given up on the way out
    // however the install ended, so the offer on that panel is live again by
    // the time the user can act on it.
    if crate::update::install_in_progress() {
        return Err(
            "Clippy installe une mise à jour\u{a0}: l’enregistreur ne peut pas être démarré \
                    pour le moment. Réessayer une fois la mise à jour terminée."
                .to_string(),
        );
    }
    supervisor.launch()
}

#[tauri::command]
pub fn daemon_connected(supervisor: State<'_, Arc<Supervisor>>) -> bool {
    supervisor.is_connected()
}

/// Appends lines to `%APPDATA%\clippy\logs\ui-<day>.jsonl` for diagnostics
/// mode (see clippy-daemon's `diag.rs`). The window only calls this while
/// `diagnostics` is on; the daemon owns pruning the folder.
///
/// `day` comes from the window (`YYYYMMDD`, its local date) and is checked to
/// be exactly that, so it cannot name another file.
#[tauri::command]
pub fn diag_append(day: String, lines: Vec<String>) -> Result<(), String> {
    use std::io::Write as _;
    if day.len() != 8 || !day.bytes().all(|b| b.is_ascii_digit()) {
        return Err("jour invalide".into());
    }
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or("APPDATA absent")?
        .join("clippy")
        .join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("ui-{day}.jsonl"));
    // Same per-file ceiling as the daemon's logs.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 200 * 1024 * 1024) {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut out = String::new();
    for line in lines.iter().take(2000) {
        // One record per line, whatever the window sent.
        let line: String = line.chars().take(8192).filter(|c| *c != '\n' && *c != '\r').collect();
        out.push_str(&line);
        out.push('\n');
    }
    file.write_all(out.as_bytes()).map_err(|e| e.to_string())
}
