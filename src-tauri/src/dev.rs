//! Finds rebuildable project artifacts and reads Docker Desktop storage information.

use crate::{
    inventory::timestamp,
    system_actions::{move_to_trash as trash_quietly, play_completion},
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, Metadata},
    io::{self, Read},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use walkdir::WalkDir;

const MIB: u64 = 1024 * 1024;
const MAX_RESULTS: usize = 250;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Kind {
    NodeModules,
    Venv,
    RustTarget,
    NextBuild,
    SwiftBuild,
    XcodeDerivedData,
}

impl Kind {
    fn identify(path: &Path, xcode_root: &Path) -> Option<Self> {
        let parent = path.parent()?;
        let name = path.file_name()?.to_str()?;
        if parent == xcode_root
            && path.join("info.plist").is_file()
            && (path.join("Build").is_dir() || path.join("Index.noindex").is_dir())
        {
            return Some(Self::XcodeDerivedData);
        }
        match name {
            "node_modules" if parent.join("package.json").is_file() => Some(Self::NodeModules),
            "target"
                if parent.join("Cargo.toml").is_file()
                    && (path.join(".rustc_info.json").is_file()
                        || path.join("debug/.fingerprint").is_dir()) =>
            {
                Some(Self::RustTarget)
            }
            ".next"
                if parent.join("package.json").is_file()
                    && (path.join("BUILD_ID").is_file()
                        || path.join("cache").is_dir()
                        || path.join("server").is_dir()) =>
            {
                Some(Self::NextBuild)
            }
            ".build"
                if parent.join("Package.swift").is_file()
                    && (path.join("workspace-state.json").is_file()
                        || path.join("build.db").is_file()) =>
            {
                Some(Self::SwiftBuild)
            }
            _ if path.join("pyvenv.cfg").is_file() => Some(Self::Venv),
            _ => None,
        }
    }

    fn rebuild(self, path: &Path) -> Option<&'static str> {
        let project = path.parent()?;
        match self {
            Self::NodeModules if project.join("pnpm-lock.yaml").is_file() => {
                Some("pnpm install --frozen-lockfile")
            }
            Self::NodeModules if project.join("yarn.lock").is_file() => Some("yarn install"),
            Self::NodeModules
                if project.join("bun.lock").is_file() || project.join("bun.lockb").is_file() =>
            {
                Some("bun install --frozen-lockfile")
            }
            Self::NodeModules
                if project.join("package-lock.json").is_file()
                    || project.join("npm-shrinkwrap.json").is_file() =>
            {
                Some("npm ci")
            }
            Self::Venv if project.join("requirements.txt").is_file() => {
                Some("Virtuelle Umgebung neu anlegen, dann pip install -r requirements.txt")
            }
            Self::Venv
                if project.join("uv.lock").is_file()
                    && project.join("pyproject.toml").is_file() =>
            {
                Some("uv sync")
            }
            Self::Venv
                if project.join("poetry.lock").is_file()
                    && project.join("pyproject.toml").is_file() =>
            {
                Some("poetry install")
            }
            Self::RustTarget => Some("cargo build"),
            Self::NextBuild => Some("Den Build-Befehl des Projekts erneut ausführen"),
            Self::SwiftBuild => Some("swift build"),
            Self::XcodeDerivedData => Some("Projekt erneut in Xcode bauen"),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct DevState(Mutex<DevSession>);

#[derive(Default)]
struct DevSession {
    folder: Option<PathBuf>,
    scan: u64,
    entries: HashMap<u32, DevEntry>,
}

struct DevEntry {
    path: PathBuf,
    kind: Kind,
    digest: blake3::Hash,
    modified_at: Option<u64>,
    can_trash: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRow {
    id: u32,
    name: String,
    path: String,
    kind: Kind,
    bytes: u64,
    modified_at: Option<u64>,
    rebuild: Option<&'static str>,
    can_trash: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevReport {
    scan: u64,
    roots: Vec<String>,
    rows: Vec<ProjectRow>,
    total: u64,
    skipped: usize,
    truncated: bool,
}

fn display(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

fn fingerprint(metadata: &Metadata, digest: &mut blake3::Hasher) {
    for value in [
        metadata.dev(),
        metadata.ino(),
        u64::from(metadata.mode()),
        metadata.len(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ] {
        digest.update(&value.to_le_bytes());
    }
}

fn project_contents(path: &Path) -> io::Result<(u64, blake3::Hash, Option<u64>)> {
    if path.canonicalize()? != path || !path.symlink_metadata()?.is_dir() {
        return Err(io::Error::other("Der Ordner wurde seit dem Scan ersetzt."));
    }
    let device = path.symlink_metadata()?.dev();
    let mut digest = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut newest = None;
    for entry in WalkDir::new(path)
        .same_file_system(true)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry?;
        let relative = entry.path().strip_prefix(path).map_err(io::Error::other)?;
        let key = relative.as_os_str().as_encoded_bytes();
        digest.update(&(key.len() as u64).to_le_bytes());
        digest.update(key);
        let metadata = entry.metadata()?;
        if metadata.dev() != device {
            return Err(io::Error::other(
                "Der Ordner enthält ein weiteres eingebundenes Laufwerk.",
            ));
        }
        fingerprint(&metadata, &mut digest);
        if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
            newest = newest.max(metadata.modified().ok().and_then(timestamp));
        }
    }
    Ok((bytes, digest.finalize(), newest))
}

fn scan_root(root: &Path, xcode_root: &Path) -> (Vec<(DevEntry, u64)>, usize) {
    let mut rows = Vec::new();
    let mut skipped = 0;
    let mut walk = WalkDir::new(root)
        .max_depth(16)
        .same_file_system(true)
        .follow_links(false)
        .into_iter();
    while let Some(entry) = walk.next() {
        let entry = match entry {
            Ok(entry) if entry.file_type().is_dir() => entry,
            Ok(_) => continue,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy();
        if let Some(kind) = Kind::identify(path, xcode_root) {
            walk.skip_current_dir();
            match project_contents(path) {
                Ok((bytes, digest, modified_at)) if bytes >= MIB => rows.push((
                    DevEntry {
                        path: path.to_owned(),
                        kind,
                        digest,
                        modified_at,
                        can_trash: kind.rebuild(path).is_some(),
                    },
                    bytes,
                )),
                Ok(_) => {}
                Err(_) => skipped += 1,
            }
        } else if entry.depth() > 0
            && (name.starts_with('.')
                || matches!(name.as_ref(), "node_modules" | "Library")
                || [
                    ".app",
                    ".photoslibrary",
                    ".bundle",
                    ".sparsebundle",
                    ".vmwarevm",
                    ".pvm",
                ]
                .iter()
                .any(|suffix| name.ends_with(suffix)))
        {
            walk.skip_current_dir();
        }
    }
    (rows, skipped)
}

#[tauri::command]
pub async fn pick_dev_folder(
    app: AppHandle,
    english: Option<bool>,
) -> Result<Option<String>, String> {
    let chosen = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || {
            app.dialog()
                .file()
                .set_title(if english == Some(true) {
                    "Choose Project Folder"
                } else {
                    "Projektordner auswählen"
                })
                .blocking_pick_folder()
        }
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(chosen) = chosen else {
        return Ok(None);
    };
    let path = chosen
        .into_path()
        .map_err(|error| error.to_string())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !path.is_dir() {
        return Err("Bitte einen Projektordner auswählen.".into());
    }
    let state = app.state::<DevState>();
    let mut session = state
        .0
        .lock()
        .map_err(|_| "Dev-Scan ist vorübergehend nicht verfügbar.")?;
    session.folder = Some(path.clone());
    session.scan += 1;
    session.entries.clear();
    let home = app.path().home_dir().map_err(|error| error.to_string())?;
    Ok(Some(display(&path, &home)))
}

pub(crate) fn project_roots(home: &Path) -> Vec<PathBuf> {
    [
        "Developer",
        "Development",
        "Projects",
        "Projekte",
        "Code",
        "Repos",
        "src",
        "workspace",
        "Sites",
        "Desktop",
        "Documents",
        "Downloads",
    ]
    .into_iter()
    .map(|part| home.join(part))
    .filter(|path| path.symlink_metadata().is_ok_and(|meta| meta.is_dir()))
    .collect()
}

pub(crate) fn impact_roots(app: &AppHandle, home: &Path) -> Vec<PathBuf> {
    let mut roots = project_roots(home);
    let state = app.state::<DevState>();
    if let Ok(session) = state.0.lock() {
        if let Some(folder) = session.folder.as_ref().filter(|path| path.is_dir()) {
            if !roots.iter().any(|root| folder.starts_with(root)) {
                roots.retain(|root| !root.starts_with(folder));
                roots.push(folder.clone());
            }
        }
    }
    roots
}

#[tauri::command]
pub async fn scan_dev(app: AppHandle, custom: bool) -> Result<DevReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = app.path().home_dir().map_err(|error| error.to_string())?;
        let state = app.state::<DevState>();
        let xcode_root = home.join("Library/Developer/Xcode/DerivedData");
        let (scan, roots) = {
            let mut session = state
                .0
                .lock()
                .map_err(|_| "Dev-Scan ist vorübergehend nicht verfügbar.")?;
            let roots = if custom {
                vec![session
                    .folder
                    .clone()
                    .ok_or("Bitte zuerst einen Projektordner auswählen.")?]
            } else {
                let mut roots = project_roots(&home);
                if xcode_root
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.is_dir())
                {
                    roots.push(xcode_root.clone());
                }
                roots
            };
            session.scan += 1;
            (session.scan, roots)
        };
        let scanned: Vec<_> = roots
            .par_iter()
            .map(|root| scan_root(root, &xcode_root))
            .collect();
        let mut found = Vec::new();
        let mut skipped = 0;
        for (entries, errors) in scanned {
            found.extend(entries);
            skipped += errors;
        }
        found.sort_unstable_by(|(a, a_bytes), (b, b_bytes)| {
            b_bytes.cmp(a_bytes).then_with(|| a.path.cmp(&b.path))
        });
        let total = found.iter().map(|(_, bytes)| bytes).sum();
        let truncated = found.len() > MAX_RESULTS;
        let mut entries = HashMap::new();
        let mut rows = Vec::new();
        for (index, (entry, bytes)) in found.into_iter().take(MAX_RESULTS).enumerate() {
            let id = (index + 1) as u32;
            rows.push(ProjectRow {
                id,
                name: entry
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: display(&entry.path, &home),
                kind: entry.kind,
                bytes,
                modified_at: entry.modified_at,
                rebuild: entry.kind.rebuild(&entry.path),
                can_trash: entry.can_trash,
            });
            entries.insert(id, entry);
        }
        let mut session = state
            .0
            .lock()
            .map_err(|_| "Dev-Scan ist vorübergehend nicht verfügbar.")?;
        if session.scan != scan {
            return Err("Dieser Dev-Scan wurde ersetzt.".into());
        }
        session.entries = entries;
        Ok(DevReport {
            scan,
            roots: roots.iter().map(|root| display(root, &home)).collect(),
            rows,
            total,
            skipped,
            truncated,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) fn project_target(app: &AppHandle, scan: u64, id: u32) -> Result<PathBuf, String> {
    let data = app.state::<DevState>();
    let session = data
        .0
        .lock()
        .map_err(|_| "Der Projektscan ist vorübergehend nicht verfügbar.")?;
    if session.scan != scan {
        return Err("Aktualisiere zuerst den Projektscan.".into());
    }
    session
        .entries
        .get(&id)
        .map(|entry| entry.path.clone())
        .ok_or("Dieser Ordner gehört nicht zum aktuellen Projektscan.".into())
}

#[tauri::command]
pub async fn act_on_dev_entry(
    app: AppHandle,
    scan: u64,
    id: u32,
    move_to_trash: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if move_to_trash {
            let path = project_target(&app, scan, id)?;
            let home = app.path().home_dir().map_err(|error| error.to_string())?;
            let impact =
                crate::dev_impact::inspect(&home, &impact_roots(&app, &home), &path, None, None);
            if impact.blocking {
                return Err(
                    "Ein laufender Prozess oder Projektlink verweist auf diesen Ordner.".into(),
                );
            }
        }
        let state = app.state::<DevState>();
        let mut session = state
            .0
            .lock()
            .map_err(|_| "Dev-Scan ist vorübergehend nicht verfügbar.")?;
        if session.scan != scan {
            return Err("Bitte den Dev-Scan aktualisieren.".into());
        }
        let entry = session
            .entries
            .get(&id)
            .ok_or("Der Ordner ist im aktuellen Scan nicht verfügbar.")?;
        if move_to_trash {
            let xcode_root = app
                .path()
                .home_dir()
                .map_err(|error| error.to_string())?
                .join("Library/Developer/Xcode/DerivedData");
            if !entry.can_trash
                || Kind::identify(&entry.path, &xcode_root) != Some(entry.kind)
                || entry.kind.rebuild(&entry.path).is_none()
            {
                return Err("Für diesen Ordner ist derzeit kein Wiederaufbau erkennbar.".into());
            }
            let (_, current, _) =
                project_contents(&entry.path).map_err(|error| error.to_string())?;
            if current != entry.digest {
                return Err(
                    "Der Ordner hat sich seit dem Scan verändert. Bitte erneut scannen.".into(),
                );
            }
            trash_quietly(&entry.path)?;
            session.entries.remove(&id);
            play_completion();
        } else if entry.path.canonicalize().ok().as_ref() == Some(&entry.path) {
            Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&entry.path)
                .status()
                .map_err(|error| error.to_string())?
                .success()
                .then_some(())
                .ok_or("Der Ordner konnte nicht im Finder angezeigt werden.")?;
        } else {
            return Err("Der Ordnerpfad hat sich seit dem Scan verändert.".into());
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "PascalCase", serialize = "camelCase"))]
struct DockerImage {
    #[serde(rename(deserialize = "ID"))]
    id: String,
    repository: String,
    tag: String,
    size: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "PascalCase", serialize = "camelCase"))]
struct DockerUsage {
    #[serde(rename(deserialize = "Type"))]
    label: String,
    size: String,
    reclaimable: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerReport {
    status: &'static str,
    images: Vec<DockerImage>,
    usage: Vec<DockerUsage>,
    truncated: bool,
}

fn docker_output(executable: &Path, socket: &Path, args: &[&str]) -> io::Result<String> {
    let endpoint = format!("unix://{}", socket.display());
    let mut child = Command::new(executable)
        .env_remove("DOCKER_CONTEXT")
        .env_remove("DOCKER_HOST")
        .arg("--host")
        .arg(endpoint)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped").take(6 * MIB);
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output)?;
        Ok::<_, io::Error>(output)
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if start.elapsed() < Duration::from_secs(7) => {
                std::thread::sleep(Duration::from_millis(35))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("Lokale Docker-Abfrage fehlgeschlagen."));
            }
        }
    }
    reader
        .join()
        .map_err(|_| io::Error::other("Docker-Antwort konnte nicht gelesen werden."))?
}

fn docker_report(home: &Path) -> DockerReport {
    let mut report = DockerReport {
        status: "missing",
        images: Vec::new(),
        usage: Vec::new(),
        truncated: false,
    };
    let Some(executable) = [
        home.join(".docker/bin/docker"),
        "/Applications/Docker.app/Contents/Resources/bin/docker".into(),
        "/opt/homebrew/bin/docker".into(),
        "/usr/local/bin/docker".into(),
    ]
    .into_iter()
    .find(|path| path.is_file()) else {
        return report;
    };
    report.status = "offline";
    let socket = home.join(".docker/run/docker.sock");
    if !fs::symlink_metadata(&socket).is_ok_and(|meta| meta.file_type().is_socket()) {
        return report;
    }
    let (images, usage) = rayon::join(
        || {
            docker_output(
                &executable,
                &socket,
                &["image", "ls", "--no-trunc", "--format", "{{json .}}"],
            )
        },
        || {
            docker_output(
                &executable,
                &socket,
                &["system", "df", "--format", "{{json .}}"],
            )
        },
    );
    let Ok(images) = images else {
        return report;
    };
    report.status = "ready";
    for line in images.lines() {
        if report.images.len() == 150 {
            report.truncated = true;
            break;
        }
        if let Ok(row) = serde_json::from_str(line) {
            report.images.push(row);
        }
    }
    if let Ok(output) = usage {
        report.usage = output
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
    }
    report
}

#[tauri::command]
pub async fn scan_docker(app: AppHandle) -> Result<DockerReport, String> {
    let home = app.path().home_dir().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || docker_report(&home))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_docker() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        Command::new("/usr/bin/open")
            .args(["-a", "Docker"])
            .status()
            .map_err(|error| error.to_string())?
            .success()
            .then_some(())
            .ok_or("Docker Desktop konnte nicht geöffnet werden.".into())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn dev_artifacts_require_project_markers_and_detect_changes_without_following_links() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bad-batch-dev-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let modules = root.join("node_modules");
        fs::create_dir(&modules).unwrap();
        let xcode = root.join("DerivedData");
        assert!(Kind::identify(&modules, &xcode).is_none());
        fs::write(root.join("package.json"), "{}").unwrap();
        assert!(Kind::identify(&modules, &xcode) == Some(Kind::NodeModules));
        assert!(Kind::NodeModules.rebuild(&modules).is_none());
        fs::write(root.join("package-lock.json"), "{}").unwrap();
        assert_eq!(Kind::NodeModules.rebuild(&modules), Some("npm ci"));

        let outside = root.join("outside.bin");
        fs::write(&outside, vec![0; 4096]).unwrap();
        symlink(outside, modules.join("outside.bin")).unwrap();
        let (size, original, _) = project_contents(&modules).unwrap();
        assert_eq!(size, 0);
        fs::write(modules.join("module.js"), "test").unwrap();
        assert_ne!(project_contents(&modules).unwrap().1, original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn docker_cli_fields_are_parsed_with_the_actual_id_spelling() {
        let row: DockerImage = serde_json::from_str(
            r#"{"ID":"sha256:abc","Repository":"example/app","Tag":"dev","Size":"2GB"}"#,
        )
        .unwrap();
        assert_eq!(row.id, "sha256:abc");
        let row: DockerUsage =
            serde_json::from_str(r#"{"Type":"Images","Size":"4GB","Reclaimable":"1GB (25%)"}"#)
                .unwrap();
        assert_eq!(row.label, "Images");
        assert_eq!(row.reclaimable, "1GB (25%)");
    }
}
