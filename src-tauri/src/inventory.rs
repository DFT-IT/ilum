//! Builds the app and tool inventory and resolves scan-scoped action targets.

use crate::residues::{app_bundles, read_bundle, AppBundle};
use crate::{
    dev::impact_roots,
    dev_impact::{self, ImpactReport},
    system_actions::play_completion,
    tool_removal::Removal,
    toolchain::{self, DevCacheEntry, ToolReport},
};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::c_void,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

#[derive(Default)]
pub struct InventoryState(Mutex<InventoryPaths>, Mutex<()>);

#[derive(Default)]
struct InventoryPaths {
    next_run: u64,
    stored_run: u64,
    paths: HashMap<u32, PathBuf>,
    tool_removals: HashMap<usize, Removal>,
    dev_caches: HashMap<usize, DevCacheEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInventory {
    scan: u64,
    scanned_at: u64,
    apps: Vec<AppRow>,
    cache_bytes: u64,
    skipped: usize,
    tools: ToolReport,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppRow {
    id: String,
    name: String,
    version: Option<String>,
    installed: bool,
    system_app: bool,
    last_used: Option<u64>,
    cache_bytes: u64,
    cache_updated: Option<u64>,
    incomplete: bool,
    app_target: Option<u32>,
    locations: Vec<Location>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    target: u32,
    path: String,
    bytes: u64,
    can_review: bool,
}

fn is_user_cache(path: &Path, home: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let mut parts = relative.iter();
    if parts.next().is_none_or(|part| part != "Library") {
        return false;
    }
    let Some(kind) = parts.next().and_then(|part| part.to_str()) else {
        return false;
    };
    let Some(owner) = parts.next().and_then(|part| part.to_str()) else {
        return false;
    };
    let owner = owner.to_ascii_lowercase();
    let apple = owner.starts_with("com.apple.")
        || matches!(owner.as_str(), "siritts" | "geoservices" | "cloudkit");
    if apple
        && !matches!(
            owner.as_str(),
            "com.apple.safari" | "com.apple.safaritechnologypreview"
        )
    {
        return false;
    }
    match kind {
        "Caches" => true,
        "Containers" => parts.eq(["Data", "Library", "Caches"]),
        _ => false,
    }
}

pub(crate) fn resolve_cache_target(
    app: &AppHandle,
    home: &Path,
    scan: u64,
    target: u32,
) -> Result<PathBuf, String> {
    let state = app.state::<InventoryState>();
    let state = state
        .0
        .lock()
        .map_err(|_| "Der Systemscan ist vorübergehend nicht verfügbar.")?;
    if state.stored_run != scan {
        return Err("Bitte den Systemscan aktualisieren.".into());
    }
    let path = state
        .paths
        .get(&target)
        .ok_or("Dieser Cache gehört nicht zum aktuellen Systemscan.")?;
    if !is_user_cache(path, home)
        || !path.is_dir()
        || path.canonicalize().ok().as_ref() != Some(path)
    {
        return Err("Dieser Cache kann nur im Finder geprüft werden.".into());
    }
    Ok(path.clone())
}

pub(crate) fn display(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|tail| format!("~/{}", tail.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

pub(crate) fn timestamp(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}

pub(crate) fn now() -> u64 {
    timestamp(SystemTime::now()).unwrap_or_default()
}

type CFRef = *const c_void;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn MDItemCreate(allocator: CFRef, path: CFRef) -> CFRef;
    fn MDItemCopyAttribute(item: CFRef, attribute: CFRef) -> CFRef;
    static kMDItemLastUsedDate: CFRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        allocator: CFRef,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: u8,
    ) -> CFRef;
    fn CFGetTypeID(value: CFRef) -> usize;
    fn CFDateGetTypeID() -> usize;
    fn CFDateGetAbsoluteTime(value: CFRef) -> f64;
    fn CFRelease(value: CFRef);
}

fn launch_services_date(path: &Path) -> Option<u64> {
    let bytes = path.to_str()?.as_bytes();
    // Every successful Create or Copy is released on the same thread.
    unsafe {
        let string = CFStringCreateWithBytes(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0x0800_0100,
            0,
        );
        if string.is_null() {
            return None;
        }
        let item = MDItemCreate(std::ptr::null(), string);
        CFRelease(string);
        if item.is_null() {
            return None;
        }
        let date = MDItemCopyAttribute(item, kMDItemLastUsedDate);
        CFRelease(item);
        if date.is_null() {
            return None;
        }
        let result = if CFGetTypeID(date) == CFDateGetTypeID() {
            let unix = CFDateGetAbsoluteTime(date) + 978_307_200.0;
            (unix.is_finite() && unix > 0.0 && unix <= now() as f64 + 86_400.0)
                .then_some(unix as u64)
        } else {
            None
        };
        CFRelease(date);
        result
    }
}

fn indexed_apps(home: &Path) -> Vec<AppBundle> {
    let output = Command::new("/usr/bin/mdfind")
        .arg("-0")
        .arg("-onlyin")
        .arg(home)
        .arg("kMDItemContentType == 'com.apple.application-bundle'")
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .map(Path::new)
        .filter(|path| {
            path.strip_prefix(home).is_ok_and(|relative| {
                !relative.starts_with("Library")
                    && !relative.starts_with("Applications")
                    && !relative
                        .components()
                        .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
                    && !relative.parent().is_some_and(|parent| {
                        parent.components().any(|part| {
                            Path::new(part.as_os_str())
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
                        })
                    })
            })
        })
        .take(500)
        .filter_map(read_bundle)
        .collect()
}

#[derive(Default)]
pub(crate) struct Measure {
    pub bytes: u64,
    pub(crate) newest: Option<u64>,
    pub skipped: usize,
}

pub(crate) fn measure(path: &Path) -> Measure {
    let mut result = Measure::default();
    let mut seen = HashSet::new();
    for entry in WalkDir::new(path)
        .same_file_system(true)
        .follow_links(false)
    {
        let entry = match entry {
            Ok(entry) if entry.file_type().is_file() => entry,
            Ok(_) => continue,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        match entry.metadata() {
            Ok(meta) if seen.insert((meta.dev(), meta.ino())) => {
                result.bytes = result
                    .bytes
                    .saturating_add(meta.blocks().saturating_mul(512));
                result.newest = result.newest.max(meta.modified().ok().and_then(timestamp));
            }
            Ok(_) => {}
            Err(_) => result.skipped += 1,
        }
    }
    result
}

fn list(path: &Path, skipped: &mut usize) -> Vec<fs::DirEntry> {
    match path.read_dir() {
        Ok(entries) => entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(_) => {
                    *skipped += 1;
                    None
                }
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => {
            *skipped += 1;
            Vec::new()
        }
    }
}

fn containers(root: &Path, skipped: &mut usize) -> Vec<(String, PathBuf)> {
    let mut caches = Vec::new();
    for entry in list(root, skipped) {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for suffix in ["Data/Library/Caches", "Library/Caches"] {
            let path = entry.path().join(suffix);
            match path.symlink_metadata() {
                Ok(meta) if meta.is_dir() => {
                    caches.push((entry.file_name().to_string_lossy().into_owned(), path))
                }
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => *skipped += 1,
                _ => {}
            }
        }
    }
    caches
}

fn cache_roots(home: &Path) -> (Vec<(String, PathBuf)>, usize) {
    let mut skipped = 0;
    let mut roots = Vec::new();
    for root in [
        home.join("Library/Caches"),
        "/Library/Caches".into(),
        "/System/Library/Caches".into(),
    ] {
        for entry in list(&root, &mut skipped) {
            if entry.file_name() == ".DS_Store" {
                continue;
            }
            if entry.file_name() == "Google" && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                for child in list(&entry.path(), &mut skipped) {
                    let name = child.file_name();
                    let owner = match name.to_str() {
                        Some("Chrome") => "com.google.chrome",
                        Some("Chrome Beta") => "com.google.chrome.beta",
                        Some("Chrome Canary") => "com.google.chrome.canary",
                        Some("Chrome Dev") => "com.google.chrome.dev",
                        _ => "Google",
                    };
                    if child
                        .file_type()
                        .is_ok_and(|kind| kind.is_dir() || kind.is_file())
                    {
                        roots.push((owner.into(), child.path()));
                    }
                }
                continue;
            }
            if entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_file())
            {
                roots.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                ));
            }
        }
    }
    roots.extend(containers(&home.join("Library/Containers"), &mut skipped));
    roots.extend(containers(
        &home.join("Library/Group Containers"),
        &mut skipped,
    ));
    let mut seen = HashSet::new();
    roots.retain(|(_, path)| {
        if !seen.insert(path.clone()) {
            return false;
        }
        let valid = path.canonicalize().is_ok_and(|real| real == *path);
        if !valid {
            skipped += 1;
        }
        valid
    });
    (roots, skipped)
}

fn cache_owner(
    name: &str,
    apps: &BTreeMap<String, AppBundle>,
    unique_names: &HashMap<String, Option<String>>,
) -> Option<String> {
    let name = name.to_ascii_lowercase();
    if apps.contains_key(&name) {
        return Some(name);
    }
    if matches!(
        name.as_str(),
        "com.google.chrome.beta" | "com.google.chrome.dev" | "com.google.chrome.canary"
    ) {
        return None;
    }
    if name.contains('.') {
        return apps
            .keys()
            .filter(|key| {
                name.strip_prefix(key.as_str())
                    .is_some_and(|tail| tail.starts_with('.'))
            })
            .max_by_key(|key| key.len())
            .cloned();
    }
    // A shared vendor directory such as "Google" stays unattributed.
    unique_names.get(&name).and_then(Clone::clone)
}

fn collect(home: &Path, scan: u64, commands: bool) -> (SystemInventory, HashMap<u32, PathBuf>) {
    let ((apps, mut skipped), (caches, missed)) =
        rayon::join(|| app_bundles(home), || cache_roots(home));
    skipped += missed;
    let mut installed = BTreeMap::<String, AppBundle>::new();
    for app in apps.into_iter().chain(indexed_apps(home)) {
        installed.entry(app.id.clone()).or_insert(app);
    }
    let mut unique_names = HashMap::<_, Option<String>>::new();
    for app in installed.values() {
        let name = app.name.to_ascii_lowercase();
        unique_names
            .entry(name)
            .and_modify(|entry| *entry = None)
            .or_insert_with(|| Some(app.id.clone()));
    }
    let ((app_dates, caches), mut tools) = rayon::join(
        || {
            rayon::join(
                || {
                    installed
                        .par_iter()
                        .map(|(id, app)| (id.clone(), launch_services_date(&app.path)))
                        .collect::<HashMap<_, _>>()
                },
                || {
                    caches
                        .into_par_iter()
                        .map(|(name, path)| {
                            let detail = measure(&path);
                            (name, path, detail)
                        })
                        .collect::<Vec<_>>()
                },
            )
        },
        || toolchain::scan(home, installed.values(), commands),
    );

    tools.record_app_usage(
        home,
        installed.values().filter_map(|app| {
            app_dates
                .get(&app.id)
                .copied()
                .flatten()
                .map(|date| (app.path.clone(), date))
        }),
    );

    let mut paths = HashMap::new();
    let mut register = |path: PathBuf| {
        let id = paths.len() as u32 + 1;
        paths.insert(id, path);
        id
    };
    let mut rows = BTreeMap::new();
    for (id, app) in &installed {
        let app_target = app.path.canonicalize().ok().map(&mut register);
        rows.insert(
            id.clone(),
            AppRow {
                id: id.clone(),
                name: app.name.clone(),
                version: app.version.clone(),
                installed: true,
                system_app: app.path.starts_with("/System"),
                last_used: app_dates.get(id).copied().flatten(),
                cache_bytes: 0,
                cache_updated: None,
                incomplete: false,
                app_target,
                locations: Vec::new(),
            },
        );
    }
    let mut cache_bytes = 0u64;
    for (name, path, detail) in caches {
        skipped += detail.skipped;
        if detail.bytes == 0 && detail.skipped == 0 {
            continue;
        }
        let owner = cache_owner(&name, &installed, &unique_names);
        let id = owner.unwrap_or_else(|| format!("cache:{}", name.to_ascii_lowercase()));
        let row = rows.entry(id.clone()).or_insert_with(|| AppRow {
            id,
            name,
            version: None,
            installed: false,
            system_app: false,
            last_used: None,
            cache_bytes: 0,
            cache_updated: None,
            incomplete: false,
            app_target: None,
            locations: Vec::new(),
        });
        cache_bytes = cache_bytes.saturating_add(detail.bytes);
        row.cache_bytes = row.cache_bytes.saturating_add(detail.bytes);
        row.cache_updated = row.cache_updated.max(detail.newest);
        row.incomplete |= detail.skipped != 0;
        let display = display(&path, home);
        let can_review = is_user_cache(&path, home) && path.is_dir();
        row.locations.push(Location {
            target: register(path),
            path: display,
            bytes: detail.bytes,
            can_review,
        });
    }
    let mut apps: Vec<_> = rows.into_values().collect();
    for row in &mut apps {
        row.locations
            .sort_by_key(|location| std::cmp::Reverse(location.bytes));
    }
    apps.sort_by(|a, b| {
        b.cache_bytes
            .cmp(&a.cache_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    (
        SystemInventory {
            scan,
            scanned_at: now(),
            apps,
            cache_bytes,
            skipped,
            tools,
        },
        paths,
    )
}

/// Collects installed apps, cache locations, and development tools. CLI history is opt-in.
#[tauri::command]
pub async fn scan_inventory(
    app: AppHandle,
    command_history: bool,
) -> Result<SystemInventory, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = app
            .path()
            .home_dir()
            .map_err(|error| error.to_string())?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let state = app.state::<InventoryState>();
        let scan = {
            let mut state = state
                .0
                .lock()
                .map_err(|_| "Der Systemscan ist vorübergehend nicht verfügbar.")?;
            state.next_run += 1;
            state.next_run
        };
        let (report, paths) = collect(&home, scan, command_history);
        let mut state = state
            .0
            .lock()
            .map_err(|_| "Der Systemscan ist vorübergehend nicht verfügbar.")?;
        if scan < state.stored_run {
            return Err("Ein neuerer Systemscan liegt bereits vor.".into());
        }
        state.stored_run = scan;
        state.paths = paths;
        state.tool_removals = report.tools.removal_plans();
        state.dev_caches = report.tools.cache_entries();
        Ok(report)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn reveal_inventory_item(app: AppHandle, scan: u64, target: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<InventoryState>();
        let path = {
            let state = state
                .0
                .lock()
                .map_err(|_| "Der Systemscan ist vorübergehend nicht verfügbar.")?;
            if state.stored_run != scan {
                return Err("Bitte den Systemscan aktualisieren.".into());
            }
            state
                .paths
                .get(&target)
                .cloned()
                .ok_or("Dieser Fund gehört nicht zum aktuellen Systemscan.")?
        };
        if path.canonicalize().ok().as_ref() != Some(&path) {
            return Err("Der Pfad wurde seit dem Scan verändert.".into());
        }
        Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .status()
            .map_err(|error| error.to_string())?
            .success()
            .then_some(())
            .ok_or("Der Fund konnte nicht im Finder angezeigt werden.".into())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn inventory_home(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .home_dir()
        .and_then(|path| path.canonicalize().map_err(Into::into))
        .map_err(|error| error.to_string())
}

fn resolve_dev_cache(app: &AppHandle, scan: u64, id: usize) -> Result<DevCacheEntry, String> {
    let inventory = app.state::<InventoryState>();
    let state = inventory
        .0
        .lock()
        .map_err(|_| "Das Inventar ist vorübergehend nicht verfügbar.")?;
    if state.stored_run != scan {
        return Err("Aktualisiere zuerst das Inventar.".into());
    }
    state
        .dev_caches
        .get(&id)
        .cloned()
        .ok_or("Dieser Cache gehört nicht zum aktuellen Inventar.".into())
}

fn resolve_dev_tool(app: &AppHandle, scan: u64, id: usize) -> Result<Removal, String> {
    let inventory = app.state::<InventoryState>();
    let state = inventory
        .0
        .lock()
        .map_err(|_| "Das Inventar ist vorübergehend nicht verfügbar.")?;
    if state.stored_run != scan {
        return Err("Aktualisiere zuerst das Inventar.".into());
    }
    state
        .tool_removals
        .get(&id)
        .cloned()
        .ok_or("Für dieses Werkzeug ist keine eindeutige Deinstallation verfügbar.".into())
}

#[tauri::command]
pub async fn check_dev_impact(
    app: AppHandle,
    scan: u64,
    kind: String,
    id: usize,
) -> Result<ImpactReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = inventory_home(&app)?;
        let roots = impact_roots(&app, &home);
        match kind.as_str() {
            "cache" => {
                let entry = resolve_dev_cache(&app, scan, id)?;
                Ok(dev_impact::inspect(
                    &home,
                    &roots,
                    &entry.path,
                    None,
                    Some(entry.name),
                ))
            }
            "tool" => {
                let plan = resolve_dev_tool(&app, scan, id)?;
                Ok(dev_impact::inspect(
                    &home,
                    &roots,
                    plan.path(),
                    Some(&plan),
                    None,
                ))
            }
            "project" => {
                let id = u32::try_from(id).map_err(|_| "Unbekannter Projekteintrag.")?;
                let target = crate::dev::project_target(&app, scan, id)?;
                Ok(dev_impact::inspect(&home, &roots, &target, None, None))
            }
            _ => Err("Unbekannter Prüfbereich.".into()),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn reveal_dev_cache(app: AppHandle, scan: u64, id: usize) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = resolve_dev_cache(&app, scan, id)?;
        if entry.path.canonicalize().ok().as_ref() != Some(&entry.path) {
            return Err("Der Cache-Pfad wurde seit dem Scan verändert.".into());
        }
        Command::new("/usr/bin/open")
            .arg("-R")
            .arg(entry.path)
            .status()
            .map_err(|error| error.to_string())?
            .success()
            .then_some(())
            .ok_or("Der Cache konnte nicht im Finder angezeigt werden.".into())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheCleanup {
    processed: u64,
    remaining: u64,
    in_trash: bool,
}

#[tauri::command]
pub async fn clean_dev_cache(app: AppHandle, scan: u64, id: usize) -> Result<CacheCleanup, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let inventory = app.state::<InventoryState>();
        let _operation = inventory.1.try_lock().map_err(|_| "Eine Bereinigung läuft bereits.")?;
        let entry = resolve_dev_cache(&app, scan, id)?;
        let plan = entry.removal.as_ref().ok_or("Für diesen Cache ist keine direkte Bereinigung verfügbar.")?;
        let home = inventory_home(&app)?;
        let impact = dev_impact::inspect(&home, &impact_roots(&app, &home), &entry.path, None, Some(entry.name));
        if impact.blocking {
            return Err("Ein laufender Prozess oder Projektlink verweist auf den Cache. Prüfe diese Nutzung zuerst.".into());
        }
        let before = measure(&entry.path);
        if before.skipped > 0 { return Err("Der Cache konnte nicht vollständig gelesen werden.".into()); }
        plan.remove()?;
        let in_trash = plan.label() == "Papierkorb";
        let remaining = if in_trash { 0 } else { measure(&entry.path).bytes };
        let processed = before.bytes.saturating_sub(remaining);
        if in_trash || processed > 0 { play_completion(); }
        Ok(CacheCleanup { processed, remaining, in_trash })
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn remove_inventory_tool(app: AppHandle, scan: u64, id: usize) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let shared = app.state::<InventoryState>();
        let _operation = shared
            .1
            .try_lock()
            .map_err(|_| "Ein Werkzeug wird bereits entfernt.")?;
        let plan = resolve_dev_tool(&app, scan, id)?;
        let home = inventory_home(&app)?;
        let impact = dev_impact::inspect(
            &home,
            &impact_roots(&app, &home),
            plan.path(),
            Some(&plan),
            None,
        );
        if impact.blocking {
            return Err(
                "Ein laufender Prozess oder Projektlink verweist auf dieses Werkzeug.".into(),
            );
        }
        plan.remove()?;
        let mut state = shared
            .0
            .lock()
            .map_err(|_| "Das Inventar ist vorübergehend nicht verfügbar.")?;
        if state.stored_run == scan {
            state.tool_removals.remove(&id);
        }
        play_completion();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_cleanup_accepts_only_user_app_cache_locations() {
        let home = Path::new("/Users/local");
        for path in [
            "Library/Caches/vendor/Browser",
            "Library/Containers/com.apple.Safari/Data/Library/Caches",
            "Library/Containers/app.example.foo/Data/Library/Caches",
        ] {
            assert!(is_user_cache(&home.join(path), home));
        }
        for path in [
            "Library/Caches",
            "Library/Caches/SiriTTS",
            "Library/Caches/com.apple.bookassetd",
            "Library/CachesBackup/app",
            "Library/Preferences/app.plist",
            "Library/Containers/app.example.foo/Data/Library",
            "Library/Group Containers/group.foo/Library/Caches",
        ] {
            assert!(!is_user_cache(&home.join(path), home));
        }
        assert!(!is_user_cache(Path::new("/Library/Caches/vendor"), home));
    }

    #[test]
    fn matches_bundle_ids_and_helpers_without_guessing_shared_vendor_folders() {
        let app = AppBundle {
            id: "com.vendor.editor".into(),
            name: "Editor".into(),
            version: None,
            path: "/Applications/Editor.app".into(),
        };
        let apps = [(app.id.clone(), app)].into();
        assert_eq!(
            cache_owner("com.vendor.Editor.helper", &apps, &HashMap::new()).as_deref(),
            Some("com.vendor.editor")
        );
        assert_eq!(cache_owner("Vendor", &apps, &HashMap::new()), None);
    }
}
