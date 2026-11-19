//! Scans known file locations and revalidates eligible results before file actions.

use crate::{
    residues::InstalledApps,
    system_actions::{move_to_trash, play_completion, wait_until, ScratchDir},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    fs::{self, Metadata},
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use walkdir::WalkDir;

const MIB: u64 = 1024 * 1024;
const HEAVY: u64 = 100 * MIB;
const ROW_LIMIT: usize = 160;
const DUPLICATE_MIN: u64 = MIB;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Mode {
    Cache,
    AppCache,
    Storage,
    Files,
}

impl Mode {
    fn parse(mode: &str) -> Result<Self, String> {
        match mode {
            "cache" => Ok(Self::Cache),
            "appCache" => Ok(Self::AppCache),
            "storage" => Ok(Self::Storage),
            "files" => Ok(Self::Files),
            _ => Err("Unbekannter Scanbereich.".into()),
        }
    }
}

#[derive(Default)]
pub struct ScanState {
    next_id: AtomicU64,
    next_scan: AtomicU64,
    items: Mutex<HashMap<Mode, (u64, HashMap<u64, Candidate>)>>,
}

#[derive(Clone, Copy, PartialEq)]
struct Fingerprint {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<&Metadata> for Fingerprint {
    fn from(meta: &Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
            size: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

struct Candidate {
    path: PathBuf,
    fingerprint: Fingerprint,
    category: &'static str,
    old_download: bool,
    hard_links: u64,
    finding: Finding,
    duplicate: Option<String>,
    app_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Finding {
    Standard,
    OldTemp,
    OldLog,
    OldCache,
    ResidualCache,
    ResidualData,
}

impl Finding {
    fn recommended(self) -> bool {
        matches!(self, Self::OldTemp | Self::OldLog | Self::OldCache)
    }

    fn residual(self) -> bool {
        matches!(self, Self::ResidualCache | Self::ResidualData)
    }
}

impl Candidate {
    fn validate(&self) -> io::Result<()> {
        let metadata = self.path.symlink_metadata()?;
        if metadata.is_file()
            && self.fingerprint == Fingerprint::from(&metadata)
            && self.path.canonicalize()? == self.path
        {
            Ok(())
        } else {
            Err(io::Error::other("Datei seit dem Scan verändert."))
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    rows: Vec<Row>,
    groups: Vec<Group>,
    total: u64,
    count: usize,
    skipped: usize,
    truncated: bool,
    old_downloads: u64,
    recommended_bytes: u64,
    recommended_count: usize,
    residue_count: usize,
    inventory_available: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    id: u64,
    name: String,
    path: String,
    bytes: u64,
    hard_links: u64,
    modified_at: Option<u64>,
    category: &'static str,
    old_download: bool,
    duplicate: Option<String>,
    finding: Finding,
    app_id: Option<String>,
}

#[derive(Serialize)]
struct Group {
    name: &'static str,
    bytes: u64,
    count: usize,
}

#[derive(Serialize)]
pub struct TrashResult {
    moved: Vec<u64>,
    failed: Vec<String>,
}

fn roots(mode: Mode) -> &'static [(&'static str, &'static str)] {
    if mode == Mode::Cache {
        &[
            ("Library/Caches", "App-Caches"),
            ("Library/Logs", "Protokolle"),
            ("Library/Application Support", "Anwendungsdaten"),
            ("Library/Preferences", "Einstellungen"),
            ("Library/Saved Application State", "Gesicherter App-Zustand"),
            (
                "Library/Containers/com.apple.Safari/Data/Library/Caches",
                "Safari",
            ),
        ]
    } else {
        &[
            ("Downloads", "Downloads"),
            ("Documents", "Dokumente"),
            ("Desktop", "Schreibtisch"),
            ("Movies", "Filme"),
            ("Music", "Musik"),
            ("Pictures", "Bilder"),
        ]
    }
}

fn browser_category(name: &str) -> &'static str {
    match name {
        "com.apple.Safari" | "com.apple.SafariTechnologyPreview" => "Safari",
        "Google" | "com.google.Chrome" => "Chrome",
        "Firefox" | "org.mozilla.firefox" => "Firefox",
        "Microsoft Edge" | "com.microsoft.edgemac" => "Edge",
        "BraveSoftware" => "Brave",
        _ => "App-Caches",
    }
}

fn rebuildable_cache_file(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if relative.starts_with("Homebrew/downloads") {
        return true;
    }
    let mut parts = relative.iter();
    let Some(owner) = parts.next().and_then(|part| part.to_str()) else {
        return false;
    };
    owner.contains('.')
        && !owner.to_ascii_lowercase().starts_with("com.apple.")
        && parts.any(|part| {
            part.to_str()
                .is_some_and(|name| matches!(name, "Cache" | "Code Cache" | "GPUCache"))
        })
}

fn scan_root(
    root: PathBuf,
    category: &'static str,
    mode: Mode,
    apps: &InstalledApps,
) -> (Vec<Candidate>, usize) {
    match root.canonicalize() {
        Ok(resolved) if resolved == root && resolved.is_dir() => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Vec::new(), 0),
        _ => return (Vec::new(), 1),
    }
    let mut candidates = Vec::new();
    let mut skipped = 0;
    let mut residue_by_name = HashMap::new();
    let data = matches!(
        category,
        "Anwendungsdaten" | "Einstellungen" | "Gesicherter App-Zustand"
    );
    let entries = WalkDir::new(&root)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            entry.depth() == 0
                || (!name.starts_with('.')
                    && !entry.file_type().is_symlink()
                    && (!data || entry.depth() > 1 || apps.might_be_leftover(&name))
                    && !(matches!(mode, Mode::Files | Mode::Storage)
                        && entry.file_type().is_dir()
                        && [
                            ".photoslibrary",
                            ".app",
                            ".bundle",
                            ".sparsebundle",
                            ".musiclibrary",
                            ".fcpbundle",
                            ".vmwarevm",
                            ".pvm",
                        ]
                        .iter()
                        .any(|suffix| name.to_ascii_lowercase().ends_with(suffix))))
        });
    for entry in entries {
        let entry = match entry {
            Ok(entry) if entry.file_type().is_file() => entry,
            Ok(_) => continue,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let meta = match entry.metadata() {
            Ok(meta) if meta.is_file() && meta.len() > 0 => meta,
            Ok(_) => continue,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let age = meta
            .modified()
            .ok()
            .and_then(|time| SystemTime::now().duration_since(time).ok());
        if category == "Temporäre Dateien"
            && (meta.uid() != unsafe { libc::geteuid() }
                || !age.is_some_and(|age| age >= Duration::from_secs(7 * 86400)))
        {
            continue;
        }
        let first = entry
            .path()
            .strip_prefix(&root)
            .ok()
            .and_then(|path| path.iter().next())
            .and_then(|part| part.to_str())
            .unwrap_or_default();
        let category = if category == "App-Caches" && mode == Mode::Cache {
            browser_category(first)
        } else {
            category
        };
        let old_download = category == "Downloads"
            && age.is_some_and(|age| age >= Duration::from_secs(90 * 86400));
        let is_old = age.is_some_and(|age| age >= Duration::from_secs(30 * 86400));
        let finding = match category {
            "Temporäre Dateien" => Finding::OldTemp,
            _ if data => Finding::ResidualData,
            "App-Caches" | "Protokolle"
                if *residue_by_name
                    .entry(first.to_owned())
                    .or_insert_with(|| apps.might_be_leftover(first)) =>
            {
                Finding::ResidualCache
            }
            "Protokolle" if is_old => Finding::OldLog,
            "App-Caches"
                if is_old && mode == Mode::Cache && rebuildable_cache_file(entry.path(), &root) =>
            {
                Finding::OldCache
            }
            _ => Finding::Standard,
        };
        let category = if finding.residual() {
            "Mögliche App-Reste"
        } else {
            category
        };
        let app_id = finding.residual().then(|| {
            first
                .trim_end_matches(".savedState")
                .trim_end_matches(".plist")
                .trim_end_matches(".log")
                .to_owned()
        });
        candidates.push(Candidate {
            path: entry.into_path(),
            fingerprint: Fingerprint::from(&meta),
            category,
            old_download,
            hard_links: u64::from(meta.nlink()),
            finding,
            duplicate: None,
            app_id,
        });
    }
    (candidates, skipped)
}

fn content_hash(candidate: &Candidate) -> io::Result<blake3::Hash> {
    candidate.validate()?;
    let mut file = File::open(&candidate.path)?;
    if Fingerprint::from(&file.metadata()?) != candidate.fingerprint {
        return Err(io::Error::other("Datei seit dem Scan verändert."));
    }
    let mut hash = blake3::Hasher::new();
    io::copy(&mut file, &mut hash)?;
    if Fingerprint::from(&file.metadata()?) != candidate.fingerprint {
        return Err(io::Error::other("Datei beim Lesen verändert."));
    }
    Ok(hash.finalize())
}

fn collect_report(
    state: &ScanState,
    home: &Path,
    mode: Mode,
    scan: u64,
    app_cache: Option<PathBuf>,
) -> Result<Report, String> {
    let mut skipped = 0;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let mut scan_roots: Vec<_> = match app_cache {
        Some(path) => vec![(path, "App-Caches")],
        None => roots(mode)
            .iter()
            .map(|(path, label)| (home.join(path), *label))
            .collect(),
    };
    if mode == Mode::Cache {
        if let Ok(temporary) = std::env::temp_dir().canonicalize() {
            if temporary.starts_with("/private/var/folders")
                && temporary.file_name().is_some_and(|name| name == "T")
                && temporary
                    .metadata()
                    .is_ok_and(|meta| meta.uid() == unsafe { libc::geteuid() })
            {
                scan_roots.push((temporary, "Temporäre Dateien"));
            }
        }
    }
    let apps = if mode == Mode::Cache {
        InstalledApps::load(home)
    } else {
        InstalledApps::default()
    };
    let results: Vec<_> = scan_roots
        .into_par_iter()
        .map(|(root, category)| scan_root(root, category, mode, &apps))
        .collect();
    for (files, errors) in results {
        skipped += errors;
        candidates.extend(
            files
                .into_iter()
                .filter(|item| seen.insert((item.fingerprint.device, item.fingerprint.inode))),
        );
    }
    candidates.sort_unstable_by(|a, b| {
        b.fingerprint
            .size
            .cmp(&a.fingerprint.size)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut duplicates = HashMap::new();
    if mode == Mode::Files {
        let mut sizes = HashMap::<_, Vec<_>>::new();
        for (index, item) in candidates
            .iter()
            .enumerate()
            .filter(|(_, item)| item.fingerprint.size >= DUPLICATE_MIN)
        {
            sizes.entry(item.fingerprint.size).or_default().push(index);
        }
        let comparable: Vec<_> = sizes
            .into_values()
            .filter(|set| set.len() > 1)
            .flatten()
            .collect();
        let hashed: Vec<_> = comparable
            .par_iter()
            .map(|&index| (index, content_hash(&candidates[index])))
            .collect();
        let mut hashes = HashMap::<_, Vec<_>>::new();
        for (index, result) in hashed {
            match result {
                Ok(hash) => hashes.entry(hash).or_default().push(index),
                Err(_) => skipped += 1,
            }
        }
        for (hash, indices) in hashes.into_iter().filter(|(_, set)| set.len() > 1) {
            let group = hash.to_hex().to_string();
            duplicates.extend(indices.into_iter().map(|index| (index, group.clone())));
        }
    }
    let mut groups = BTreeMap::new();
    let mut report = Report {
        rows: Vec::new(),
        groups: Vec::new(),
        total: 0,
        count: 0,
        skipped,
        truncated: false,
        old_downloads: 0,
        recommended_bytes: 0,
        recommended_count: 0,
        residue_count: 0,
        inventory_available: apps.reliable(),
    };
    let mut allowed = HashMap::new();
    let mut per_category = HashMap::<_, usize>::new();
    for (index, mut item) in candidates.into_iter().enumerate() {
        let duplicate = duplicates.remove(&index);
        if mode == Mode::Files
            && item.fingerprint.size < HEAVY
            && duplicate.is_none()
            && !item.old_download
        {
            continue;
        }
        let bytes = item.fingerprint.size;
        let group = groups.entry(item.category).or_insert(Group {
            name: item.category,
            bytes: 0,
            count: 0,
        });
        group.bytes += bytes;
        group.count += 1;
        report.total += bytes;
        report.count += 1;
        if item.old_download {
            report.old_downloads += bytes;
        }
        if item.finding.residual() {
            report.residue_count += 1;
        }
        let keys = if mode == Mode::Files {
            [
                (bytes >= HEAVY).then_some("heavy"),
                duplicate.is_some().then_some("duplicate"),
                item.old_download.then_some("old"),
            ]
        } else {
            [
                Some(item.category),
                item.finding.recommended().then_some("recommended"),
                (item.finding == Finding::ResidualCache).then_some("residualCache"),
            ]
        };
        let mut include = false;
        for key in keys.into_iter().flatten() {
            let shown = per_category.entry(key).or_default();
            include |= *shown < ROW_LIMIT;
            *shown += 1;
            report.truncated |= *shown > ROW_LIMIT;
        }
        if !include {
            continue;
        }
        if item.finding.recommended() {
            report.recommended_count += 1;
            report.recommended_bytes += bytes;
        }
        let id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let display_path = item
            .path
            .strip_prefix(home)
            .map(|relative| format!("~/{}", relative.display()))
            .unwrap_or_else(|_| item.path.display().to_string());
        report.rows.push(Row {
            id,
            name: item
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: display_path,
            bytes,
            hard_links: item.hard_links,
            modified_at: u64::try_from(item.fingerprint.modified.0).ok(),
            category: item.category,
            old_download: item.old_download,
            duplicate,
            finding: item.finding,
            app_id: item.app_id.clone(),
        });
        item.duplicate = report.rows.last().and_then(|row| row.duplicate.clone());
        allowed.insert(id, item);
    }
    report.groups = groups.into_values().collect();
    report
        .groups
        .sort_by_key(|group| std::cmp::Reverse(group.bytes));
    let mut snapshots = state
        .items
        .lock()
        .map_err(|_| "Scan-Ergebnis nicht verfügbar.")?;
    if snapshots
        .get(&mode)
        .is_some_and(|(previous, _)| *previous > scan)
    {
        return Err("Dieser Scan wurde durch einen neueren ersetzt.".into());
    }
    snapshots.insert(mode, (scan, allowed));
    Ok(report)
}

/// Scans the requested scope and records action targets for the resulting report.
#[tauri::command]
pub async fn scan_files(
    app: AppHandle,
    mode: String,
    inventory_scan: Option<u64>,
    inventory_target: Option<u32>,
) -> Result<Report, String> {
    let mode = Mode::parse(&mode)?;
    let home = app
        .path()
        .home_dir()
        .and_then(|path| path.canonicalize().map_err(Into::into))
        .map_err(|error| error.to_string())?;
    let scan = app
        .state::<ScanState>()
        .next_scan
        .fetch_add(1, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let app_cache = if mode == Mode::AppCache {
            let (inventory_scan, inventory_target) = inventory_scan
                .zip(inventory_target)
                .ok_or("Bitte einen App-Cache auswählen.")?;
            Some(crate::inventory::resolve_cache_target(
                &app,
                &home,
                inventory_scan,
                inventory_target,
            )?)
        } else {
            None
        };
        collect_report(&app.state::<ScanState>(), &home, mode, scan, app_cache)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Clone, Serialize)]
struct TrashProgress {
    operation: u64,
    completed: usize,
    total: usize,
}

/// Revalidates the selected scan results before moving eligible files to the Trash.
#[tauri::command]
pub async fn trash_files(
    app: AppHandle,
    window: WebviewWindow,
    mode: String,
    ids: Vec<u64>,
    operation: u64,
) -> Result<TrashResult, String> {
    let mode = Mode::parse(&mode)?;
    if ids.len() > 2000 {
        return Err("Zu viele Dateien in einer Auswahl.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ScanState>();
        let mut snapshots = state
            .items
            .lock()
            .map_err(|_| "Scan-Ergebnis nicht verfügbar.")?;
        let (_, candidates) = snapshots
            .get_mut(&mode)
            .ok_or("Bitte zuerst einen Scan starten.")?;
        let mut result = TrashResult {
            moved: Vec::new(),
            failed: Vec::new(),
        };
        let selected: HashSet<_> = ids.into_iter().collect();
        let kept_duplicates: HashSet<_> = candidates
            .iter()
            .filter(|(id, _)| !selected.contains(id))
            .filter_map(|(_, candidate)| {
                candidate
                    .duplicate
                    .as_ref()
                    .filter(|_| candidate.validate().is_ok())
                    .cloned()
            })
            .collect();
        let total = selected.len();
        let step = (total / 48).max(1);
        let emit_progress = |completed| {
            let _ = window.emit(
                "trash-progress",
                TrashProgress {
                    operation,
                    completed,
                    total,
                },
            );
        };
        for (index, id) in selected.into_iter().enumerate() {
            if index % step == 0 {
                emit_progress(index);
            }
            let Some(candidate) = candidates.get(&id) else {
                result
                    .failed
                    .push("Ein Eintrag ist nicht mehr im aktuellen Scan enthalten.".into());
                continue;
            };
            if candidate
                .duplicate
                .as_ref()
                .is_some_and(|group| !kept_duplicates.contains(group))
            {
                result.failed.push(format!(
                    "Mindestens eine bestätigte Kopie von „{}“ muss erhalten bleiben.",
                    candidate
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ));
                continue;
            }
            if candidate.finding == Finding::ResidualData {
                result
                    .failed
                    .push("Mögliche App-Daten können hier nur im Finder geprüft werden.".into());
                continue;
            }
            let moved = candidate
                .validate()
                .map_err(|error| error.to_string())
                .and_then(|_| move_to_trash(&candidate.path));
            match moved {
                Ok(()) => {
                    result.moved.push(id);
                    candidates.remove(&id);
                }
                Err(error) => result.failed.push(format!(
                    "{} – {error}",
                    candidate
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                )),
            }
        }
        emit_progress(total);
        if !result.moved.is_empty() && result.failed.is_empty() {
            play_completion();
        }
        Ok(result)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn inspect_file(
    state: State<'_, ScanState>,
    mode: String,
    id: u64,
    reveal: bool,
) -> Result<(), String> {
    let mode = Mode::parse(&mode)?;
    let snapshots = state
        .items
        .lock()
        .map_err(|_| "Scan-Ergebnis nicht verfügbar.")?;
    let candidate = snapshots
        .get(&mode)
        .and_then(|(_, items)| items.get(&id))
        .ok_or("Datei nicht im aktuellen Scan.")?;
    candidate.validate().map_err(|error| error.to_string())?;
    let (executable, flag) = if reveal {
        ("/usr/bin/open", "-R")
    } else {
        ("/usr/bin/qlmanage", "-p")
    };
    let mut child = Command::new(executable)
        .arg(flag)
        .arg(&candidate.path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn previewable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg"
                    | "jpeg"
                    | "png"
                    | "gif"
                    | "webp"
                    | "heic"
                    | "heif"
                    | "avif"
                    | "tif"
                    | "tiff"
                    | "pdf"
                    | "mov"
                    | "mp4"
                    | "m4v"
                    | "docx"
                    | "pptx"
                    | "key"
                    | "pages"
                    | "numbers"
            )
        })
}

#[tauri::command]
pub async fn file_thumbnail(
    app: AppHandle,
    mode: String,
    id: u64,
) -> Result<Option<String>, String> {
    let mode = Mode::parse(&mode)?;
    // Only personal file and storage scans opt in to thumbnail rendering.
    if !matches!(mode, Mode::Files | Mode::Storage) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ScanState>();
        let path = {
            let snapshots = state
                .items
                .lock()
                .map_err(|_| "Scan-Ergebnis nicht verfügbar.")?;
            let Some(candidate) = snapshots.get(&mode).and_then(|(_, files)| files.get(&id)) else {
                return Ok(None);
            };
            if !previewable(&candidate.path) || candidate.validate().is_err() {
                return Ok(None);
            }
            candidate.path.clone()
        };
        static ACTIVE: AtomicU64 = AtomicU64::new(0);
        if ACTIVE.fetch_add(1, Ordering::AcqRel) >= 4 {
            ACTIVE.fetch_sub(1, Ordering::Release);
            return Ok(None);
        }
        struct Slot;
        impl Drop for Slot {
            fn drop(&mut self) {
                ACTIVE.fetch_sub(1, Ordering::Release);
            }
        }
        let _slot = Slot;
        let Ok(directory) = ScratchDir::new() else {
            return Ok(None);
        };
        let Ok(mut quicklook) = Command::new("/usr/bin/qlmanage")
            .args(["-t", "-s", "144", "-o"])
            .arg(directory.path())
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Ok(None);
        };
        if !wait_until(&mut quicklook, Duration::from_secs(7)).is_ok_and(|status| status.success())
        {
            return Ok(None);
        }
        let preview = fs::read_dir(directory.path())
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .find(|entry| {
                entry.path().extension().is_some_and(|value| value == "png")
                    && entry.file_type().is_ok_and(|kind| kind.is_file())
                    && entry.metadata().is_ok_and(|meta| meta.len() <= MIB)
            })
            .and_then(|entry| fs::read(entry.path()).ok())
            .filter(|data| data.starts_with(b"\x89PNG\r\n\x1a\n"))
            .map(|data| STANDARD.encode(data));
        Ok(preview)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let name = format!(
                "bad-batch-scan-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            let root = std::env::temp_dir().canonicalize().unwrap().join(name);
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("Downloads")).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_identical_files_group_and_links_do_not_count_as_copies() {
        let fixture = Fixture::new();
        let downloads = fixture.0.join("Downloads");
        fs::write(downloads.join("a"), vec![11; MIB as usize]).unwrap();
        fs::write(downloads.join("b"), vec![11; MIB as usize]).unwrap();
        fs::write(downloads.join("different"), vec![12; MIB as usize]).unwrap();
        fs::hard_link(downloads.join("a"), downloads.join("hard-link")).unwrap();
        symlink(downloads.join("a"), downloads.join("symlink")).unwrap();

        let state = ScanState::default();
        let storage = collect_report(&state, &fixture.0, Mode::Storage, 0, None).unwrap();
        assert_eq!(storage.count, 3);
        assert_eq!(
            storage
                .rows
                .iter()
                .filter(|file| file.hard_links == 2)
                .count(),
            1
        );
        let files = collect_report(&state, &fixture.0, Mode::Files, 1, None).unwrap();
        assert_eq!(files.rows.len(), 2);
        assert!(files.rows[0].duplicate.is_some());
        assert_eq!(files.rows[0].duplicate, files.rows[1].duplicate);
    }

    #[test]
    fn changed_paths_are_rejected_after_scan() {
        let fixture = Fixture::new();
        let file = fixture.0.join("Downloads/document.txt");
        fs::write(&file, "Original").unwrap();
        let state = ScanState::default();
        let report = collect_report(&state, &fixture.0, Mode::Storage, 0, None).unwrap();

        let allowed = state.items.lock().unwrap();
        let candidate = &allowed[&Mode::Storage].1[&report.rows[0].id];
        assert!(candidate.validate().is_ok());
        fs::rename(&file, file.with_extension("old")).unwrap();
        symlink(file.with_extension("old"), &file).unwrap();
        assert!(candidate.validate().is_err());
    }
}
