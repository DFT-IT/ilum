use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Default)]
pub struct InstalledApps {
    identifiers: HashSet<String>,
    reliable: bool,
}

pub(crate) struct AppBundle {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub path: PathBuf,
}

pub(crate) fn read_bundle(path: &Path) -> Option<AppBundle> {
    let plist_path = path.join("Contents/Info.plist");
    let info_file = plist_path.metadata().ok()?;
    if !info_file.is_file() || info_file.len() > 2 * 1024 * 1024 {
        return None;
    }
    let value = plist::Value::from_file(plist_path).ok()?;
    let info = value.as_dictionary()?;
    let get = |key| info.get(key).and_then(plist::Value::as_string);
    let id = get("CFBundleIdentifier")?.to_ascii_lowercase();
    let name = get("CFBundleDisplayName")
        .or_else(|| get("CFBundleName"))
        .or_else(|| path.file_stem().and_then(|name| name.to_str()))?
        .to_owned();
    let version = get("CFBundleShortVersionString").map(str::to_owned);
    Some(AppBundle {
        id,
        name,
        version,
        path: path.to_owned(),
    })
}

pub(crate) fn app_bundles(home: &Path) -> (Vec<AppBundle>, usize) {
    let mut bundles = Vec::new();
    let mut skipped = 0;
    for (root, required) in [
        ("/Applications".into(), true),
        ("/System/Applications".into(), true),
        ("/System/Library/CoreServices/Applications".into(), false),
        (home.join("Applications"), false),
    ] {
        if let Err(error) = root.read_dir() {
            if required || error.kind() != std::io::ErrorKind::NotFound {
                skipped += 1;
            }
            continue;
        }
        let mut entries = WalkDir::new(&root)
            .max_depth(5)
            .follow_links(false)
            .into_iter();
        while let Some(entry) = entries.next() {
            let Ok(entry) = entry else {
                skipped += 1;
                continue;
            };
            if entry
                .path()
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case("app"))
            {
                if let Some(bundle) = read_bundle(entry.path()) {
                    bundles.push(bundle);
                }
                if entry.file_type().is_dir() {
                    entries.skip_current_dir();
                }
            }
        }
    }
    (bundles, skipped)
}

impl InstalledApps {
    pub fn load(home: &Path) -> Self {
        let (apps, skipped) = app_bundles(home);
        Self {
            identifiers: apps.into_iter().map(|app| app.id).collect(),
            reliable: skipped == 0,
        }
    }

    pub fn might_be_leftover(&self, entry: &str) -> bool {
        let identifier = entry
            .trim_end_matches(".savedState")
            .trim_end_matches(".plist")
            .trim_end_matches(".log")
            .to_ascii_lowercase();
        if !self.reliable || identifier.starts_with("com.apple.") {
            return false;
        }
        let mut parts = identifier.split('.');
        let Some(prefix) = parts.next() else {
            return false;
        };
        if !matches!(
            prefix,
            "com" | "org" | "net" | "io" | "dev" | "app" | "co" | "de" | "eu" | "me" | "ai"
        ) || parts.count() < 2
            || !identifier.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return false;
        }
        !self.identifiers.iter().any(|known| {
            identifier == *known
                || identifier
                    .strip_prefix(known)
                    .is_some_and(|suffix| suffix.starts_with('.'))
                || known
                    .strip_prefix(&identifier)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    }

    pub fn reliable(&self) -> bool {
        self.reliable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn possible_residues_exclude_platform_ids_and_installed_apps_or_helpers() {
        let apps = InstalledApps {
            reliable: true,
            identifiers: ["com.vendor.editor".to_string()].into(),
        };
        assert!(apps.might_be_leftover("io.example.removed.plist"));
        assert!(!apps.might_be_leftover("com.vendor.editor.helper"));
        assert!(!apps.might_be_leftover("com.apple.Safari"));
        assert!(!apps.might_be_leftover("Google"));
        assert!(!InstalledApps::default().might_be_leftover("io.example.removed"));
    }
}
