//! Background watcher — detects running DAWs, watches open project files,
//! and emits parse events when content changes.
//!
//! # Architecture
//! A single background thread:
//! 1. Polls `sysinfo` every 2 s for known DAW processes.
//! 2. Finds each process's open project file (lsof on macOS; command-line /
//!    recent-file heuristic on Windows).
//! 3. Registers a `notify` watcher on the file (or package directory for
//!    Logic Pro's `.logicx`).
//! 4. On any file-system event or a 60 s catch-all timer, computes a
//!    SHA-256 hash; if it changed, parses the project and emits a
//!    [`WatchEvent`].
//!
//! # Logic Pro note
//! `.logicx` is a macOS package directory, not a single file. The watcher
//! detects the package root from handle paths and watches it recursively.
//! The hash covers only `ProjectData` (the main binary inside the package)
//! to avoid spurious re-parses from Finder metadata writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use sysinfo::{ProcessRefreshKind, System};

use crate::daw_readers::{ableton, dawproject, logicpro, reaper};

// ─── Timing constants ─────────────────────────────────────────────────────────

/// How often to poll sysinfo for new/exited DAW processes.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How often to re-run the open-file check for already-tracked processes
/// (detects in-session project switches).
const RECHECK_INTERVAL: Duration = Duration::from_secs(10);
/// Catch-all: re-hash every watched file even without a notify event.
const TIMER_INTERVAL: Duration = Duration::from_secs(60);
/// Wait after a notify event before reading the file — lets the DAW finish
/// its write before we compute the hash.
const SETTLE_DELAY: Duration = Duration::from_millis(300);

// ─── DAW process table ────────────────────────────────────────────────────────

/// Which DAW wrote / owns a project file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DawKind {
    Ableton,
    Logic,
    Reaper,
    Bitwig,
    StudioOne,
}

struct DawEntry {
    /// Substrings matched against process name + exe path (case-insensitive).
    fragments: &'static [&'static str],
    kind: DawKind,
    extensions: &'static [&'static str],
}

static DAW_TABLE: &[DawEntry] = &[
    DawEntry {
        // macOS binary: "Live"  |  Windows: "Ableton Live 12 Suite"
        fragments: &["ableton", "live"],
        kind: DawKind::Ableton,
        extensions: &[".als", ".adg"],
    },
    DawEntry {
        fragments: &["logic pro", "logic"],
        kind: DawKind::Logic,
        extensions: &[".logicx"],
    },
    DawEntry {
        fragments: &["reaper"],
        kind: DawKind::Reaper,
        extensions: &[".rpp", ".rpp-bak"],
    },
    DawEntry {
        fragments: &["bitwig"],
        kind: DawKind::Bitwig,
        extensions: &[".bwproject", ".dawproject"],
    },
    DawEntry {
        fragments: &["studio one"],
        kind: DawKind::StudioOne,
        extensions: &[".song", ".dawproject"],
    },
];

fn match_daw(name: &str, exe: &str) -> Option<&'static DawEntry> {
    let name_l = name.to_lowercase();
    let exe_l = exe.to_lowercase();
    DAW_TABLE.iter().find(|d| {
        d.fragments
            .iter()
            .any(|f| name_l.contains(f) || exe_l.contains(f))
    })
}

// ─── Public types ─────────────────────────────────────────────────────────────

/// Parsed representation of a DAW project.
#[derive(Debug)]
pub enum ParsedProject {
    Ableton(ableton::AbletonFile),
    Logic(logicpro::LogicProject),
    Reaper(reaper::ReaperProject),
    DawProject(dawproject::DawProject),
    /// Detected project type that has no reader yet (e.g. `.bwproject`, `.song`).
    Unrecognized { daw: DawKind, path: PathBuf },
}

/// Events emitted by the background watcher.
#[derive(Debug)]
pub enum WatchEvent {
    /// A new project was detected as open in a DAW.
    ProjectOpened { path: PathBuf, daw: DawKind },
    /// A project was re-parsed after a content change.
    ProjectChanged { parsed: ParsedProject },
    /// The DAW that held this project has exited.
    ProjectClosed { path: PathBuf, daw: DawKind },
    /// Content changed but parsing failed; last successfully parsed state
    /// remains valid.
    ParseError { path: PathBuf, error: String },
}

// ─── Internal ─────────────────────────────────────────────────────────────────

struct ActiveProject {
    path: PathBuf,
    daw: DawKind,
    /// SHA-256 of the last content that was successfully parsed.
    last_hash: Option<[u8; 32]>,
    /// True for `.logicx` package directories.
    is_directory: bool,
}

// ─── Hash helpers ─────────────────────────────────────────────────────────────

fn hash_file(path: &Path) -> Option<[u8; 32]> {
    let data = std::fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&data);
    Some(h.finalize().into())
}

fn hash_logic_package(dir: &Path) -> Option<[u8; 32]> {
    // Use file mtime rather than content SHA-256 — ProjectData can be
    // 50–200 MB; hashing content on every notify event (Finder writes,
    // autosave bookkeeping, etc.) causes continuous high CPU usage.
    // Mtime changes precisely when Logic Pro saves the project, which is
    // the only moment we care about re-parsing.
    //
    // Logic 10.1+: ProjectData lives in Alternatives/000/.
    let modern = dir.join("Alternatives").join("000").join("ProjectData");
    if let Some(h) = mtime_hash(&modern) {
        return Some(h);
    }
    // Pre-10.1: ProjectData at the package root.
    for name in &["ProjectData", "projectData"] {
        if let Some(h) = mtime_hash(&dir.join(name)) {
            return Some(h);
        }
    }
    // Last resort: hash all file modification times in the package root.
    let mut hasher = Sha256::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in &entries {
        if let Ok(meta) = e.metadata() {
            if let Ok(t) = meta.modified() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    hasher.update(d.as_nanos().to_le_bytes());
                }
            }
        }
        hasher.update(e.file_name().as_encoded_bytes());
    }
    Some(hasher.finalize().into())
}

fn mtime_hash(path: &Path) -> Option<[u8; 32]> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let nanos = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let mut h = Sha256::new();
    h.update(nanos.to_le_bytes());
    h.update(path.as_os_str().as_encoded_bytes());
    Some(h.finalize().into())
}

fn hash_project(path: &Path, is_directory: bool) -> Option<[u8; 32]> {
    if is_directory {
        hash_logic_package(path)
    } else {
        hash_file(path)
    }
}

// ─── Parse dispatch ───────────────────────────────────────────────────────────

fn parse_project(path: &Path, daw: DawKind) -> Result<ParsedProject, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match daw {
        DawKind::Ableton => ableton::read_file(path)
            .map(ParsedProject::Ableton)
            .map_err(|e| e.to_string()),

        DawKind::Logic => logicpro::read_logicx(path)
            .map(ParsedProject::Logic)
            .map_err(|e| e.to_string()),

        DawKind::Reaper => reaper::read_rpp(path)
            .map(ParsedProject::Reaper)
            .map_err(|e| e.to_string()),

        DawKind::Bitwig | DawKind::StudioOne => {
            if ext == "dawproject" {
                dawproject::read_dawproject(path)
                    .map(ParsedProject::DawProject)
                    .map_err(|e| e.to_string())
            } else {
                Ok(ParsedProject::Unrecognized {
                    daw,
                    path: path.to_owned(),
                })
            }
        }
    }
}

// ─── Platform: open-file detection ────────────────────────────────────────────

fn find_open_project(pid: u32, entry: &DawEntry) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return find_via_lsof(pid, entry.extensions);

    #[cfg(target_os = "windows")]
    return find_via_cmdline(pid, entry.extensions)
        .or_else(|| find_via_recent_dirs(entry));

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return None;
}

// ─── macOS: lsof ─────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn find_via_lsof(pid: u32, extensions: &[&str]) -> Option<PathBuf> {
    let out = std::process::Command::new("lsof")
        .args(["-p", &pid.to_string(), "-F", "n"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut best: Option<PathBuf> = None;

    for line in stdout.lines() {
        let Some(path_str) = line.strip_prefix('n') else { continue };
        let p = PathBuf::from(path_str);

        // Direct extension match (e.g. .als, .rpp).
        let ext_dot = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        if extensions.iter().any(|e| *e == ext_dot) && p.exists() {
            best = Some(p);
            continue;
        }

        // Ancestor match for .logicx packages — Logic opens many files inside
        // the package; we want the root directory.
        if extensions.contains(&".logicx") {
            for ancestor in p.ancestors().skip(1) {
                if ancestor.extension().and_then(|e| e.to_str()) == Some("logicx")
                    && ancestor.exists()
                {
                    best = Some(ancestor.to_owned());
                    break;
                }
            }
        }
    }

    best
}

// ─── Windows: command-line args + recent-files heuristic ─────────────────────

#[cfg(target_os = "windows")]
fn find_via_cmdline(pid: u32, extensions: &[&str]) -> Option<PathBuf> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::everything());
    let proc = sys.process(sysinfo::Pid::from_u32(pid))?;
    for arg in proc.cmd().iter().skip(1) {
        let p = PathBuf::from(arg);
        let ext_dot = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        if extensions.iter().any(|e| *e == ext_dot) && p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn find_via_recent_dirs(entry: &DawEntry) -> Option<PathBuf> {
    // Scan standard project directories for the most recently modified file
    // whose extension matches and whose mtime is within 8 hours (i.e. opened
    // in the current working session). 90 s was too tight — Ableton doesn't
    // always write the project file within 90 s of launch.
    //
    // This is a heuristic fallback. A full implementation would enumerate
    // kernel file handles via NtQuerySystemInformation / NtQueryObject.
    let home = std::env::var("USERPROFILE").ok().map(PathBuf::from)?;
    let docs = home.join("Documents");

    let search_dirs: &[PathBuf] = &match entry.kind {
        DawKind::Ableton => vec![
            docs.join("Ableton").join("Projects"),
            docs.join("Ableton"),
            // Users frequently store projects outside Documents
            PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
                .join("Music").join("Ableton"),
        ],
        DawKind::Reaper => vec![docs.clone()],
        DawKind::Bitwig => vec![docs.join("Bitwig Studio").join("Projects")],
        DawKind::StudioOne => vec![docs.join("Studio One").join("Songs")],
        DawKind::Logic => vec![], // Logic does not run on Windows
    };

    // 8 hours: covers a full working session without stale file risk.
    let cutoff = std::time::SystemTime::now()
        .checked_sub(Duration::from_secs(8 * 3600))
        .unwrap_or(std::time::UNIX_EPOCH);

    for dir in search_dirs {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    let ext_dot = p
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| format!(".{x}"))
                        .unwrap_or_default();
                    if !entry.extensions.iter().any(|x| *x == ext_dot) {
                        return None;
                    }
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    (mtime >= cutoff).then_some((mtime, p))
                })
                .collect();
            candidates.sort_by(|a, b| b.0.cmp(&a.0));
            if let Some((_, p)) = candidates.into_iter().next() {
                return Some(p);
            }
        }
    }
    None
}

// ─── Worker: check-and-emit helper ────────────────────────────────────────────

fn check_and_emit(
    path: &Path,
    active: &mut HashMap<u32, ActiveProject>,
    events_tx: &Sender<WatchEvent>,
) {
    if let Some(ap) = active.values_mut().find(|ap| ap.path == path) {
        let new_hash = hash_project(&ap.path, ap.is_directory);
        if new_hash == ap.last_hash {
            return;
        }
        match parse_project(&ap.path, ap.daw) {
            Ok(parsed) => {
                ap.last_hash = new_hash;
                let _ = events_tx.send(WatchEvent::ProjectChanged { parsed });
            }
            Err(e) => {
                // Do not update last_hash — let the next event retry.
                let _ = events_tx.send(WatchEvent::ParseError {
                    path: path.to_owned(),
                    error: e,
                });
            }
        }
    }
}

// ─── Worker: process refresh ──────────────────────────────────────────────────

fn refresh_active(
    sys: &System,
    active: &mut HashMap<u32, ActiveProject>,
    watcher: &mut RecommendedWatcher,
    events_tx: &Sender<WatchEvent>,
    check_existing: bool,
) {
    let mut live_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();

    for (&pid, process) in sys.processes() {
        let pid_u32 = pid.as_u32();
        let name = process.name().to_string();
        let exe = process
            .exe()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_lowercase();

        let Some(entry) = match_daw(&name, &exe) else {
            continue;
        };
        live_pids.insert(pid_u32);

        if active.contains_key(&pid_u32) {
            if check_existing {
                // Re-detect in case the user opened a different project.
                if let Some(new_path) = find_open_project(pid_u32, entry) {
                    let old_path = active[&pid_u32].path.clone();
                    if new_path != old_path {
                        let daw = entry.kind;
                        let _ = watcher.unwatch(&old_path);
                        let _ = events_tx.send(WatchEvent::ProjectClosed {
                            path: old_path,
                            daw,
                        });
                        register_project(pid_u32, new_path, entry, active, watcher, events_tx);
                    }
                }
            }
        } else if let Some(path) = find_open_project(pid_u32, entry) {
            register_project(pid_u32, path, entry, active, watcher, events_tx);
        }
    }

    // Remove entries whose DAW process is no longer running.
    let stale: Vec<u32> = active
        .keys()
        .filter(|pid| !live_pids.contains(*pid))
        .copied()
        .collect();
    for pid in stale {
        if let Some(ap) = active.remove(&pid) {
            let _ = watcher.unwatch(&ap.path);
            let _ = events_tx.send(WatchEvent::ProjectClosed {
                path: ap.path,
                daw: ap.daw,
            });
        }
    }
}

fn register_project(
    pid: u32,
    path: PathBuf,
    entry: &'static DawEntry,
    active: &mut HashMap<u32, ActiveProject>,
    watcher: &mut RecommendedWatcher,
    events_tx: &Sender<WatchEvent>,
) {
    let is_directory = path.is_dir();
    let mode = if is_directory {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    // Watch failure is non-fatal: we still do the initial parse so the
    // frontend sees the current state. Future file-change events just won't
    // arrive — the 60 s catch-all timer will re-hash periodically instead.
    if let Err(e) = watcher.watch(&path, mode) {
        let _ = events_tx.send(WatchEvent::ParseError {
            path: path.clone(),
            error: format!("notify watch failed (continuing without live updates): {e}"),
        });
    }
    let hash = hash_project(&path, is_directory);
    let _ = events_tx.send(WatchEvent::ProjectOpened {
        path: path.clone(),
        daw: entry.kind,
    });
    // Parse immediately so the frontend sees the current state without waiting
    // for a file-change event (which never fires if nothing changes after launch).
    match parse_project(&path, entry.kind) {
        Ok(parsed) => {
            let _ = events_tx.send(WatchEvent::ProjectChanged { parsed });
        }
        Err(e) => {
            let _ = events_tx.send(WatchEvent::ParseError {
                path: path.clone(),
                error: format!("initial parse failed: {e}"),
            });
        }
    }
    active.insert(
        pid,
        ActiveProject {
            path,
            daw: entry.kind,
            last_hash: hash,
            is_directory,
        },
    );
}

// ─── Worker thread ────────────────────────────────────────────────────────────

fn worker(events_tx: Sender<WatchEvent>, stop_rx: Receiver<()>) {
    let (notify_tx, notify_rx) = mpsc::channel::<notify::Result<notify::Event>>();

    let mut watcher = match RecommendedWatcher::new(
        move |res| {
            let _ = notify_tx.send(res);
        },
        notify::Config::default(),
    ) {
        Ok(w) => w,
        Err(e) => {
            let _ = events_tx.send(WatchEvent::ParseError {
                path: PathBuf::new(),
                error: format!("notify init error: {e}"),
            });
            return;
        }
    };

    let mut sys = System::new();
    let mut active: HashMap<u32, ActiveProject> = HashMap::new();
    // Paths awaiting the settle delay before we read them.
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    let mut last_recheck = Instant::now();
    let mut last_timer = Instant::now();

    loop {
        match stop_rx.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }

        // ── Refresh process list ──────────────────────────────────────────────
        // Refresh exe once (immutable after launch) so match_daw can check
        // both the process name and exe path. We don't need cpu/memory/environ.
        sys.refresh_processes_specifics(
            ProcessRefreshKind::new().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
        );
        let check_existing = last_recheck.elapsed() >= RECHECK_INTERVAL;
        if check_existing {
            last_recheck = Instant::now();
        }
        refresh_active(&sys, &mut active, &mut watcher, &events_tx, check_existing);

        // ── Drain notify events (up to POLL_INTERVAL) ─────────────────────────
        let poll_end = Instant::now() + POLL_INTERVAL;
        loop {
            let remaining = poll_end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            // Clamp so we don't block longer than 50 ms at a time — keeps
            // pending / timer checks responsive.
            let timeout = remaining.min(Duration::from_millis(50));
            match notify_rx.recv_timeout(timeout) {
                Ok(Ok(event)) => {
                    // Only care about data-modifying events.
                    // macOS FSEvents backend emits ModifyKind::Any for most
                    // writes, so accept that alongside the more specific variant.
                    let is_write = matches!(
                        event.kind,
                        notify::EventKind::Modify(notify::event::ModifyKind::Data(_))
                            | notify::EventKind::Modify(notify::event::ModifyKind::Any)
                            | notify::EventKind::Create(_)
                    );
                    if !is_write {
                        continue;
                    }
                    for changed_path in event.paths {
                        // Resolve notify path back to the project root
                        // (important for .logicx directories).
                        let project_root = active
                            .values()
                            .find(|ap| {
                                if ap.is_directory {
                                    changed_path.starts_with(&ap.path)
                                } else {
                                    changed_path == ap.path
                                }
                            })
                            .map(|ap| ap.path.clone());

                        if let Some(root) = project_root {
                            pending.insert(root, Instant::now() + SETTLE_DELAY);
                        }
                    }
                }
                Ok(Err(e)) => {
                    let _ = events_tx.send(WatchEvent::ParseError {
                        path: PathBuf::new(),
                        error: format!("notify error: {e}"),
                    });
                }
                Err(_) => break, // timeout — fall through to pending / timer checks
            }
        }

        // ── Process settled pending paths ─────────────────────────────────────
        let now = Instant::now();
        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, &deadline)| now >= deadline)
            .map(|(p, _)| p.clone())
            .collect();
        for path in ready {
            pending.remove(&path);
            check_and_emit(&path, &mut active, &events_tx);
        }

        // ── 60 s catch-all timer ──────────────────────────────────────────────
        if last_timer.elapsed() >= TIMER_INTERVAL {
            last_timer = Instant::now();
            let paths: Vec<PathBuf> = active.values().map(|ap| ap.path.clone()).collect();
            for path in paths {
                check_and_emit(&path, &mut active, &events_tx);
            }
        }
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Handle to the background DAW watcher thread.
///
/// Drop (or call [`stop`](WatcherService::stop)) to shut the thread down
/// cleanly. Events are available via [`events`](WatcherService::events).
pub struct WatcherService {
    events_rx: Receiver<WatchEvent>,
    stop_tx: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl WatcherService {
    /// Spawn the background thread and begin watching.
    pub fn start() -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("patchbay-watcher".into())
            .spawn(move || worker(events_tx, stop_rx))
            .expect("failed to spawn watcher thread");

        WatcherService {
            events_rx,
            stop_tx: Some(stop_tx),
            handle: Some(handle),
        }
    }

    /// Receive end of the event channel. Poll with `try_recv` or `recv`.
    pub fn events(&self) -> &Receiver<WatchEvent> {
        &self.events_rx
    }

    /// Signal the background thread to stop and wait for it to exit.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WatcherService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
