//! Records local storage measurements and compares snapshots with matching coverage.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::Disks;
use tauri::{AppHandle, Emitter, Manager};
use walkdir::WalkDir;

const INTERVAL: u64 = 6 * 60 * 60;
const DAY: u64 = 24 * 60 * 60;
const MAX_ENTRIES: usize = 200_000;
const MAX_CHILDREN: usize = 1_000;
const MAX_CHANGES: usize = 50;
const HISTORY_FILENAME: &str = "storage-growth-v1.json";
const ILUM_IDENTIFIER: &str = "com.dft-it.ilum";
const LEGACY_IDENTIFIER: &str = "de.dft-it.the-bad-batch";

const AREAS: &[(&str, &str)] = &[
    ("downloads", "Downloads"),
    ("desktop", "Desktop"),
    ("documents", "Documents"),
    ("appCaches", "Library/Caches"),
    ("xcode", "Library/Developer/Xcode/DerivedData"),
    ("docker", "Library/Containers/com.docker.docker/Data/vms"),
    ("npm", ".npm/_cacache"),
    ("cargo", ".cargo/registry"),
];

#[derive(Default)]
pub struct GrowthState(Mutex<Option<Vec<Snapshot>>>);

#[derive(Clone, Default, Deserialize, Serialize)]
struct Area {
    listed: bool,
    entries: BTreeMap<String, Option<u64>>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Snapshot {
    at: u64,
    disk_free: Option<u64>,
    areas: BTreeMap<String, Area>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthReport {
    measured_at: Option<u64>,
    free_bytes: Option<u64>,
    comparisons: Vec<Comparison>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Comparison {
    period: &'static str,
    from: u64,
    free_delta: Option<i64>,
    growing: Vec<Change>,
    total_changes: usize,
    partial_areas: usize,
}

#[derive(Serialize)]
struct Change {
    area: String,
    name: String,
    before: u64,
    after: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn measure_area(home: &Path, location: &str, home_device: u64) -> Area {
    let root = home.join(location);
    match root.canonicalize() {
        Ok(resolved) if resolved == root => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Area {
                listed: true,
                ..Area::default()
            };
        }
        _ => return Area::default(),
    }
    let Ok(entries) = fs::read_dir(&root) else {
        return Area::default();
    };
    let mut area = Area {
        listed: true,
        ..Area::default()
    };
    let mut children = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == MAX_CHILDREN {
            return Area::default();
        }
        match entry {
            Ok(entry) => children.push(entry),
            Err(_) => area.listed = false,
        }
    }
    children.sort_unstable_by_key(|entry| entry.file_name());
    let mut remaining = MAX_ENTRIES;
    let deadline = Instant::now() + Duration::from_secs(12);
    for child in children {
        let Ok(name) = child.file_name().into_string() else {
            area.listed = false;
            continue;
        };
        let mut bytes = 0_u64;
        let mut complete = true;
        let mut child_links = HashSet::new();
        let walker = WalkDir::new(child.path())
            .follow_links(false)
            .follow_root_links(false)
            .same_file_system(true)
            .max_open(12);
        for entry in walker {
            if remaining == 0 || Instant::now() >= deadline {
                complete = false;
                break;
            }
            remaining -= 1;
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                complete = false;
                continue;
            };
            if metadata.dev() != home_device {
                continue;
            }
            if metadata.nlink() > 1 {
                let inode = (metadata.dev(), metadata.ino());
                if !child_links.insert(inode) {
                    continue;
                }
            }
            bytes = bytes.saturating_add(metadata.blocks().saturating_mul(512));
        }
        area.entries.insert(name, complete.then_some(bytes));
    }
    area
}

fn capture(home: &Path) -> Result<Snapshot, String> {
    let home = home.canonicalize().map_err(|error| error.to_string())?;
    let device = fs::metadata(&home)
        .map_err(|error| error.to_string())?
        .dev();
    let areas = AREAS
        .par_iter()
        .map(|&(key, location)| (key.to_owned(), measure_area(&home, location, device)))
        .collect();
    let disks = Disks::new_with_refreshed_list();
    Ok(Snapshot {
        at: now(),
        disk_free: crate::monitor::startup_disk(&disks).map(|disk| disk.available_space()),
        areas,
    })
}

fn compare(latest: &Snapshot, baseline: &Snapshot, period: &'static str) -> Comparison {
    let free_delta = latest
        .disk_free
        .zip(baseline.disk_free)
        .map(|(current, past)| (current as i64).saturating_sub(past as i64));
    let mut growing = Vec::new();
    let mut partial_areas = 0;
    for &(key, _) in AREAS {
        let Some((current, past)) = latest.areas.get(key).zip(baseline.areas.get(key)) else {
            partial_areas += 1;
            continue;
        };
        if !current.listed
            || !past.listed
            || current
                .entries
                .values()
                .chain(past.entries.values())
                .any(Option::is_none)
        {
            partial_areas += 1;
        }
        for (name, &after) in &current.entries {
            let before = match past.entries.get(name) {
                Some(&bytes) => bytes,
                None if past.listed => Some(0),
                None => None,
            };
            if let Some((before, after)) =
                before.zip(after).filter(|(before, after)| after > before)
            {
                growing.push(Change {
                    area: key.to_owned(),
                    name: name.clone(),
                    before,
                    after,
                });
            }
        }
    }
    growing.sort_unstable_by(|left, right| {
        (right.after - right.before)
            .cmp(&(left.after - left.before))
            .then_with(|| left.area.cmp(&right.area))
            .then_with(|| left.name.cmp(&right.name))
    });
    let total_changes = growing.len();
    growing.truncate(MAX_CHANGES);
    Comparison {
        period,
        from: baseline.at,
        free_delta,
        growing,
        total_changes,
        partial_areas,
    }
}

fn report(history: &[Snapshot]) -> GrowthReport {
    let mut comparisons = Vec::new();
    if let Some((latest, previous)) = history.split_last() {
        if let Some(baseline) = previous.last() {
            comparisons.push(compare(latest, baseline, "previous"));
        }
        for (period, target, tolerance) in [("day", DAY, INTERVAL), ("week", 7 * DAY, DAY)] {
            if let Some(baseline) = previous.iter().rev().find(|baseline| {
                (target..=target + tolerance).contains(&latest.at.saturating_sub(baseline.at))
            }) {
                comparisons.push(compare(latest, baseline, period));
            }
        }
    }
    GrowthReport {
        measured_at: history.last().map(|snapshot| snapshot.at),
        free_bytes: history.last().and_then(|snapshot| snapshot.disk_free),
        comparisons,
    }
}

fn read_history(path: &Path) -> Result<Vec<Snapshot>, String> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|_| "Der lokale Speicherverlauf konnte nicht gelesen werden.".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn save_history(path: &Path, history: &[Snapshot]) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(path.parent().expect("history path has a parent"))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        let written = serde_json::to_writer(&mut file, history)
            .map_err(io::Error::other)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temp, path));
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        }
        written
    })();
    result.map_err(|error| format!("Der Speicherverlauf konnte nicht gesichert werden. {error}"))
}

fn load_history(path: &Path) -> Result<Vec<Snapshot>, String> {
    if path.try_exists().map_err(|error| error.to_string())? {
        return read_history(path);
    }
    if let Some(directory) = path.parent().filter(|directory| {
        directory
            .file_name()
            .is_some_and(|name| name == ILUM_IDENTIFIER)
    }) {
        let legacy = directory
            .with_file_name(LEGACY_IDENTIFIER)
            .join(HISTORY_FILENAME);
        if let Ok(history) = read_history(&legacy) {
            if !history.is_empty() {
                save_history(path, &history)?;
                return Ok(history);
            }
        }
    }
    Ok(Vec::new())
}

fn update(app: &AppHandle, requested: Option<u64>) -> Result<GrowthReport, String> {
    let state = app.state::<GrowthState>();
    let mut state = state
        .0
        .lock()
        .map_err(|_| "Der Speicherverlauf ist nicht verfügbar.")?;
    let path = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join(HISTORY_FILENAME);
    if state.is_none() {
        *state = Some(load_history(&path)?);
    }
    let history = state.as_mut().expect("history was loaded");
    let due = history.last().is_none_or(|last| {
        requested.map_or_else(
            || now().saturating_sub(last.at) >= INTERVAL,
            |started| started > last.at,
        )
    });
    if due {
        let home = app.path().home_dir().map_err(|error| error.to_string())?;
        let next = capture(&home)?;
        let mut retained: Vec<_> = history
            .iter()
            .filter(|sample| next.at.saturating_sub(sample.at) <= 15 * DAY)
            .rev()
            .take(95)
            .cloned()
            .collect();
        retained.reverse();
        retained.push(next);
        save_history(&path, &retained)?;
        *history = retained;
        let _ = app.emit("storage-growth-updated", ());
    }
    Ok(report(history))
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        if let Err(error) = update(&app, None) {
            eprintln!("Speicherverlauf: {error}");
        }
        std::thread::sleep(Duration::from_secs(15 * 60));
    });
}

/// Returns recent storage comparisons and optionally records a fresh measurement.
#[tauri::command]
pub async fn storage_growth(app: AppHandle, refresh: bool) -> Result<GrowthReport, String> {
    let requested = refresh.then(now);
    tauri::async_runtime::spawn_blocking(move || update(&app, requested))
        .await
        .map_err(|error| error.to_string())?
}

fn found_path(home: &Path, area: &str, name: &str) -> Option<PathBuf> {
    let (_, location) = AREAS.iter().find(|(key, _)| *key == area)?;
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(part)) if part == name)
        || parts.next().is_some()
    {
        return None;
    }
    let path = home.join(location).join(name);
    (path.canonicalize().ok().as_ref() == Some(&path)).then_some(path)
}

#[tauri::command]
pub async fn reveal_storage_growth(
    app: AppHandle,
    area: String,
    name: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = app
            .path()
            .home_dir()
            .map_err(|error| error.to_string())?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let path = found_path(&home, &area, &name)
            .ok_or("Der Eintrag wurde seit der Messung verschoben oder ist nicht zugänglich.")?;
        Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .status()
            .map_err(|error| error.to_string())?
            .success()
            .then_some(())
            .ok_or_else(|| "Der Eintrag konnte nicht im Finder angezeigt werden.".into())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_actions::ScratchDir;

    #[test]
    fn migrates_legacy_history_without_overwriting_current_measurements() {
        let directory = ScratchDir::new().unwrap();
        let legacy = directory
            .path()
            .join(LEGACY_IDENTIFIER)
            .join(HISTORY_FILENAME);
        let current = directory
            .path()
            .join(ILUM_IDENTIFIER)
            .join(HISTORY_FILENAME);
        save_history(&legacy, &[snapshot(1, 900, true, &[])]).unwrap();

        assert_eq!(load_history(&current).unwrap()[0].at, 1);
        assert_eq!(read_history(&current).unwrap()[0].at, 1);

        save_history(&current, &[snapshot(2, 800, true, &[])]).unwrap();
        assert_eq!(load_history(&current).unwrap()[0].at, 2);
        assert_eq!(read_history(&legacy).unwrap()[0].at, 1);
    }

    fn snapshot(at: u64, free: u64, listed: bool, entries: &[(&str, Option<u64>)]) -> Snapshot {
        Snapshot {
            at,
            disk_free: Some(free),
            areas: BTreeMap::from([(
                "downloads".into(),
                Area {
                    listed,
                    entries: entries
                        .iter()
                        .map(|&(name, size)| (name.into(), size))
                        .collect(),
                },
            )]),
        }
    }

    #[test]
    fn compares_complete_entries_and_does_not_guess_an_unreadable_baseline() {
        let first = snapshot(1, 900, true, &[("archive", Some(20)), ("unknown", None)]);
        let next = snapshot(
            2,
            600,
            true,
            &[
                ("archive", Some(80)),
                ("image", Some(150)),
                ("unknown", Some(30)),
            ],
        );
        let delta = compare(&next, &first, "previous");
        assert_eq!(delta.free_delta, Some(-300));
        assert_eq!(
            delta
                .growing
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["image", "archive"]
        );
        assert_eq!(delta.growing[1].after - delta.growing[1].before, 60);

        let incomplete = snapshot(1, 900, false, &[("archive", Some(20))]);
        let delta = compare(&next, &incomplete, "previous");
        assert_eq!(delta.growing.len(), 1);
        assert_eq!(delta.growing[0].name, "archive");
    }

    #[test]
    fn keeps_history_on_disk_and_requires_a_baseline_near_each_period() {
        let directory = ScratchDir::new().unwrap();
        let path = directory.path().join("storage-growth-v1.json");
        let history: Vec<_> = [DAY, 6 * DAY, 7 * DAY, 8 * DAY]
            .into_iter()
            .map(|at| snapshot(at, 900, true, &[]))
            .collect();
        save_history(&path, &history).unwrap();
        let stored = read_history(&path).unwrap();
        let comparisons = report(&stored).comparisons;
        assert_eq!(
            comparisons
                .iter()
                .map(|item| (item.period, item.from))
                .collect::<Vec<_>>(),
            [("previous", 7 * DAY), ("day", 7 * DAY), ("week", DAY)]
        );
        save_history(&path, &[history[0].clone(), history[3].clone()]).unwrap();
        let comparisons = report(&read_history(&path).unwrap()).comparisons;
        assert!(comparisons.iter().all(|item| item.period != "day"));
    }

    #[test]
    fn measures_allocated_blocks_without_following_symlinks() {
        let directory = ScratchDir::new().unwrap();
        let home = directory.path().canonicalize().unwrap();
        let downloads = home.join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let file = downloads.join("archive");
        fs::write(&file, vec![0_u8; 32_768]).unwrap();
        let metadata = file.metadata().unwrap();
        std::os::unix::fs::symlink(&file, downloads.join("alias")).unwrap();
        let area = measure_area(&home, "Downloads", metadata.dev());
        assert!(area.listed);
        assert_eq!(area.entries["archive"], Some(metadata.blocks() * 512));
        assert_eq!(area.entries["alias"], Some(0));
        assert_eq!(found_path(&home, "downloads", "archive"), Some(file));
        assert!(found_path(&home, "downloads", "alias").is_none());
        assert!(found_path(&home, "downloads", "../elsewhere").is_none());
    }
}
