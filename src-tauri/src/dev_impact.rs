//! Collects project and process references before developer cleanup actions.

use crate::{inventory::display, tool_removal::Removal};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use walkdir::WalkDir;

const PATH_LIMIT: usize = 16_000;
const DISPLAY_LIMIT: usize = 20;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImpactReport {
    references: Vec<Reference>,
    checked_projects: usize,
    skipped: usize,
    truncated: bool,
    pub blocking: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
enum Signal {
    Link,
    Running,
    InstalledPackage,
    Project,
}

#[derive(Serialize)]
struct Reference {
    kind: Signal,
    name: String,
    path: String,
}

struct Assessment<'a> {
    home: &'a Path,
    found: HashSet<(Signal, PathBuf, String)>,
    checked: HashSet<PathBuf>,
    skipped: usize,
    truncated: bool,
}

impl Assessment<'_> {
    fn add(&mut self, signal: Signal, name: impl Into<String>, path: &Path) {
        self.found.insert((signal, path.to_owned(), name.into()));
    }

    fn finish(mut self) -> ImpactReport {
        let blocking = self
            .found
            .iter()
            .any(|(kind, _, _)| matches!(kind, Signal::Link | Signal::Running));
        let mut found: Vec<_> = self.found.into_iter().collect();
        let order = |signal: Signal| match signal {
            Signal::Running => 0,
            Signal::Link => 1,
            Signal::InstalledPackage => 2,
            Signal::Project => 3,
        };
        found.sort_by(|a, b| {
            order(a.0)
                .cmp(&order(b.0))
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        self.truncated |= found.len() > DISPLAY_LIMIT;
        let references = found
            .into_iter()
            .take(DISPLAY_LIMIT)
            .map(|(kind, path, name)| Reference {
                kind,
                name,
                path: display(&path, self.home),
            })
            .collect();
        ImpactReport {
            references,
            checked_projects: self.checked.len(),
            skipped: self.skipped,
            truncated: self.truncated,
            blocking,
        }
    }
}

fn bounded_text(path: &Path) -> io::Result<String> {
    const LIMIT: u64 = 768 * 1024;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err(io::Error::other("Datei liegt außerhalb des Prüfumfangs."));
    }
    let mut result = String::new();
    file.take(LIMIT).read_to_string(&mut result)?;
    Ok(result)
}

fn manifests_dependency(project: &Value, package: &str) -> bool {
    [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]
    .into_iter()
    .any(|section| {
        project
            .get(section)
            .and_then(|entries| entries.get(package))
            .is_some()
    })
}

fn points_to(link: &Path, target: &Path) -> bool {
    if link.starts_with(target) {
        return false;
    }
    if !link
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return false;
    }
    link.canonicalize()
        .is_ok_and(|resolved| resolved.starts_with(target))
}

fn check_project_link(record: &mut Assessment<'_>, path: &Path, target: &Path) {
    if points_to(path, target) {
        record.add(Signal::Link, "Symbolischer Link zum Ziel", path);
    }
}

fn shallow_module_links(
    record: &mut Assessment<'_>,
    root: &Path,
    target: &Path,
    remaining: &mut usize,
) {
    let Ok(entries) = root.read_dir() else {
        return;
    };
    for item in entries.take(180) {
        if *remaining == 0 {
            record.truncated = true;
            break;
        }
        *remaining -= 1;
        let Ok(item) = item else {
            record.skipped += 1;
            continue;
        };
        check_project_link(record, &item.path(), target);
        if item.file_name().to_string_lossy().starts_with('@')
            && item.file_type().is_ok_and(|kind| kind.is_dir())
        {
            if let Ok(children) = item.path().read_dir() {
                for child in children.take(80) {
                    if *remaining == 0 {
                        record.truncated = true;
                        break;
                    }
                    *remaining -= 1;
                    if let Ok(child) = child {
                        check_project_link(record, &child.path(), target);
                    }
                }
            }
        }
    }
}

fn ecosystem_markers(cache: &str) -> &[&str] {
    match cache {
        "npm" => &["package-lock.json", "npm-shrinkwrap.json"],
        "Yarn" => &["yarn.lock"],
        "pnpm" => &["pnpm-lock.yaml"],
        "Pip" => &["requirements.txt", "pyproject.toml"],
        "uv" => &["uv.lock"],
        "Homebrew" => &["Brewfile"],
        "Cargo Registry" => &["Cargo.toml"],
        "Go-Module" => &["go.mod"],
        "Gradle" => &["build.gradle", "build.gradle.kts"],
        "Maven" => &["pom.xml"],
        "Bun" => &["bun.lock", "bun.lockb"],
        "SwiftPM" => &["Package.swift"],
        _ => &[],
    }
}

fn scan_projects(
    record: &mut Assessment<'_>,
    roots: &[PathBuf],
    target: &Path,
    package: Option<&str>,
    cache: Option<&str>,
) {
    let mut remaining = PATH_LIMIT;
    for root in roots {
        let mut walk = WalkDir::new(root)
            .max_depth(10)
            .same_file_system(true)
            .follow_links(false)
            .into_iter();
        while let Some(entry) = walk.next() {
            if remaining == 0 {
                record.truncated = true;
                return;
            }
            remaining -= 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    record.skipped += 1;
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy();
            if entry.file_type().is_symlink() {
                check_project_link(record, path, target);
                continue;
            }
            if entry.file_type().is_dir() {
                if name == "node_modules" {
                    shallow_module_links(record, path, target, &mut remaining);
                    walk.skip_current_dir();
                } else if entry.depth() > 0
                    && (name.starts_with('.')
                        || matches!(
                            name.as_ref(),
                            "target" | "dist" | "build" | "vendor" | "Library"
                        )
                        || [".app", ".photoslibrary", ".bundle", ".pvm", ".vmwarevm"]
                            .iter()
                            .any(|suffix| name.ends_with(suffix)))
                {
                    walk.skip_current_dir();
                }
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            if let Some(cache) =
                cache.filter(|cache| ecosystem_markers(cache).contains(&name.as_ref()))
            {
                record
                    .checked
                    .insert(path.parent().unwrap_or(root).to_owned());
                record.add(
                    Signal::Project,
                    format!("Projekt verwendet das {cache}-Ökosystem"),
                    path,
                );
            }
            if let Some(package) = package.filter(|_| name == "package.json") {
                match bounded_text(path)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                {
                    Some(json) => {
                        record
                            .checked
                            .insert(path.parent().unwrap_or(root).to_owned());
                        if manifests_dependency(&json, package) {
                            record.add(Signal::Project, "Dasselbe Paket im Projektmanifest", path);
                        }
                    }
                    None => record.skipped += 1,
                }
            }
        }
    }
}

fn installed_brew_dependents(record: &mut Assessment<'_>, plan: &Removal) {
    let path = plan.path();
    let Some(cellar) = path
        .parent()
        .and_then(Path::parent)
        .filter(|path| path.ends_with("Cellar"))
    else {
        return;
    };
    let package = plan.target();
    let Ok(formulas) = cellar.read_dir() else {
        record.skipped += 1;
        return;
    };
    for formula in formulas.flatten().take(2_000) {
        if formula.file_name() == package || !formula.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(kegs) = formula.path().read_dir() else {
            record.skipped += 1;
            continue;
        };
        for keg in kegs.flatten().take(12) {
            let receipt = keg.path().join("INSTALL_RECEIPT.json");
            let Some(json) = bounded_text(&receipt)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            else {
                continue;
            };
            let relevant = json
                .get("runtime_dependencies")
                .and_then(Value::as_array)
                .is_some_and(|dependencies| {
                    dependencies.iter().any(|dependency| {
                        dependency
                            .get("full_name")
                            .or_else(|| dependency.get("name"))
                            .and_then(Value::as_str)
                            .is_some_and(|name| name.rsplit('/').next() == Some(package))
                    })
                });
            if relevant {
                record.add(
                    Signal::InstalledPackage,
                    formula.file_name().to_string_lossy(),
                    &receipt,
                );
                break;
            }
        }
    }
}

pub(crate) fn running_in(path: &Path) -> Vec<(String, PathBuf)> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet),
    );
    let uid = unsafe { libc::geteuid() };
    system
        .processes()
        .values()
        .filter(|process| {
            process.pid().as_u32() != std::process::id()
                && process.user_id().is_some_and(|owner| **owner == uid)
        })
        .filter_map(|process| {
            process
                .exe()
                .into_iter()
                .chain(process.cwd())
                .find(|candidate| candidate.starts_with(path))
                .map(|candidate| {
                    (
                        process.name().to_string_lossy().into_owned(),
                        candidate.to_owned(),
                    )
                })
        })
        .collect()
}

pub(crate) fn inspect(
    home: &Path,
    roots: &[PathBuf],
    target: &Path,
    plan: Option<&Removal>,
    cache: Option<&str>,
) -> ImpactReport {
    let mut record = Assessment {
        home,
        found: HashSet::new(),
        checked: HashSet::new(),
        skipped: 0,
        truncated: false,
    };
    for (name, path) in running_in(target) {
        record.add(Signal::Running, name, &path);
    }
    if let Some(brew) = plan.filter(|plan| plan.label() == "Homebrew") {
        installed_brew_dependents(&mut record, brew);
    }
    let package = plan
        .filter(|plan| plan.label() == "npm")
        .map(Removal::target);
    scan_projects(&mut record, roots, target, package, cache);
    record.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_actions::ScratchDir;
    use std::os::unix::fs::symlink;

    #[test]
    fn distinguishes_project_hints_from_real_links_into_the_cache() {
        let temp = ScratchDir::new().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let project = home.join("test-project");
        let cache = home.join("target-cache");
        let package = cache.join("actual-package");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir(&project).unwrap();
        fs::write(project.join("pnpm-lock.yaml"), "fixture").unwrap();
        let mut result = inspect(
            &home,
            std::slice::from_ref(&project),
            &cache,
            None,
            Some("pnpm"),
        );
        assert_eq!(result.references.len(), 1);
        assert!(!result.blocking);
        fs::create_dir(project.join("node_modules")).unwrap();
        symlink(&package, project.join("node_modules/example")).unwrap();
        result = inspect(&home, &[project], &cache, None, Some("pnpm"));
        assert_eq!(result.references.len(), 2);
        assert!(result.blocking);
    }

    #[test]
    fn links_inside_a_folder_moved_together_are_not_external_references() {
        let temp = ScratchDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("node_modules");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside"), "fixture").unwrap();
        let internal = target.join("alias");
        symlink(target.join("inside"), &internal).unwrap();
        assert!(!points_to(&internal, &target));
        let external = root.join("external");
        symlink(target.join("inside"), &external).unwrap();
        assert!(points_to(&external, &target));
    }

    #[test]
    fn npm_manifest_detection_uses_exact_package_keys() {
        let json = serde_json::json!({"devDependencies": {"@testing/tool": "^1.0.0"}});
        assert!(manifests_dependency(&json, "@testing/tool"));
        assert!(!manifests_dependency(&json, "tool"));
    }
}
