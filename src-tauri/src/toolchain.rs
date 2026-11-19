use crate::{
    inventory::{display, measure, now},
    residues::AppBundle,
    tool_removal::{self, Removal},
};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env, fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
enum Kind {
    Environment,
    Runtime,
    Cli,
    Package,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tool {
    id: usize,
    name: String,
    kind: Kind,
    source: String,
    version: Option<String>,
    path: String,
    last_used: Option<u64>,
    indirect: Option<bool>,
    removal: Option<&'static str>,
    removal_target: Option<String>,
    #[serde(skip)]
    removal_plan: Option<Removal>,
    #[serde(skip)]
    actual_path: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DevCache {
    id: usize,
    name: &'static str,
    path: String,
    bytes: u64,
    updated_at: Option<u64>,
    incomplete: bool,
    removal: Option<&'static str>,
    #[serde(skip)]
    entry: DevCacheEntry,
}

#[derive(Clone)]
pub(crate) struct DevCacheEntry {
    pub name: &'static str,
    pub path: PathBuf,
    pub removal: Option<Removal>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolReport {
    rows: Vec<Tool>,
    caches: Vec<DevCache>,
    history: &'static str,
}

impl ToolReport {
    pub(crate) fn cache_entries(&self) -> HashMap<usize, DevCacheEntry> {
        self.caches
            .iter()
            .map(|cache| (cache.id, cache.entry.clone()))
            .collect()
    }

    pub(crate) fn removal_plans(&self) -> HashMap<usize, Removal> {
        self.rows
            .iter()
            .filter_map(|tool| tool.removal_plan.clone().map(|plan| (tool.id, plan)))
            .collect()
    }

    pub(crate) fn record_app_usage(
        &mut self,
        home: &Path,
        apps: impl Iterator<Item = (PathBuf, u64)>,
    ) {
        let dates: HashMap<_, _> = apps
            .map(|(path, date)| (display(&path, home), date))
            .collect();
        for row in &mut self.rows {
            if row.kind == Kind::Environment {
                row.last_used = dates.get(&row.path).copied();
            }
        }
    }
}

struct Catalog<'a> {
    home: &'a Path,
    rows: Vec<Tool>,
    seen: HashSet<(Kind, String, PathBuf)>,
}

impl Catalog<'_> {
    fn add(
        &mut self,
        name: &str,
        kind: Kind,
        source: &str,
        path: &Path,
        version: Option<String>,
        indirect: Option<bool>,
    ) {
        let real = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        if !self.seen.insert((kind, name.to_owned(), real.clone())) {
            return;
        }
        let removal_plan = tool_removal::plan(self.home, name, source, path);
        self.rows.push(Tool {
            id: self.rows.len() + 1,
            name: name.into(),
            kind,
            source: source.into(),
            version,
            path: display(path, self.home),
            last_used: None,
            indirect,
            removal: removal_plan.as_ref().map(Removal::label),
            removal_target: removal_plan.as_ref().map(|plan| plan.target().to_owned()),
            removal_plan,
            actual_path: real,
        });
    }
}

fn entries(path: &Path) -> impl Iterator<Item = fs::DirEntry> {
    path.read_dir()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
}

fn dirs(path: impl AsRef<Path>) -> Vec<PathBuf> {
    entries(path.as_ref())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

fn leaf(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

fn json(path: &Path) -> Option<Value> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    serde_json::from_reader(file.take(4 * 1024 * 1024)).ok()
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn homebrew(catalog: &mut Catalog, prefix: &Path) {
    for formula in dirs(prefix.join("Cellar")) {
        for keg in dirs(&formula) {
            let receipt = json(&keg.join("INSTALL_RECEIPT.json"));
            let indirect = receipt
                .as_ref()
                .and_then(|value| value.get("installed_on_request"))
                .and_then(Value::as_bool)
                .map(|direct| !direct);
            catalog.add(
                leaf(&formula),
                Kind::Package,
                "Homebrew",
                &keg,
                Some(leaf(&keg).into()),
                indirect,
            );
        }
    }
    for cask in dirs(prefix.join("Caskroom")) {
        for version in dirs(&cask) {
            if leaf(&version).starts_with('.') {
                continue;
            }
            catalog.add(
                leaf(&cask),
                Kind::Package,
                "Homebrew Cask",
                &version,
                Some(leaf(&version).into()),
                None,
            );
        }
    }
}

const RUNTIMES: &[&str] = &[
    "node", "python", "python3", "pypy3", "ruby", "go", "rustc", "swift", "java", "javac", "php",
    "perl", "dotnet", "deno", "bun", "elixir", "erl", "lua", "r", "julia", "clang", "gcc",
    "kotlinc", "scala", "zig", "ocaml", "ghc", "dart",
];
const CLIS: &[&str] = &[
    "git",
    "gh",
    "glab",
    "docker",
    "docker-compose",
    "podman",
    "colima",
    "orb",
    "kubectl",
    "helm",
    "terraform",
    "tofu",
    "aws",
    "gcloud",
    "az",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "corepack",
    "pip",
    "pip3",
    "pipx",
    "uv",
    "poetry",
    "conda",
    "mamba",
    "cargo",
    "rustup",
    "rustfmt",
    "swiftlint",
    "xcodebuild",
    "xcrun",
    "brew",
    "port",
    "make",
    "cmake",
    "ninja",
    "gradle",
    "mvn",
    "ant",
    "fastlane",
    "ansible",
    "vagrant",
    "mise",
    "asdf",
    "volta",
    "fnm",
    "pyenv",
    "rbenv",
    "fn",
    "composer",
    "gem",
    "bundle",
    "protoc",
    "jq",
    "rg",
    "fd",
    "fzf",
    "bat",
    "code",
    "cursor",
    "zed",
    "nvim",
    "vim",
    "bunx",
    "vite",
    "eslint",
    "tsc",
    "node-gyp",
    "codex",
    "claude",
    "gemini",
    "opencode",
    "flutter",
    "adb",
    "sdkmanager",
    "vercel",
    "wrangler",
    "psql",
    "mysql",
    "redis-cli",
    "sqlite3",
    "mongosh",
];

fn managed_binary_version(binary: &Path) -> Option<String> {
    let real = binary.canonicalize().ok()?;
    let parts: Vec<_> = real.iter().filter_map(|part| part.to_str()).collect();
    if let Some(index) = parts.iter().position(|&part| part == "Cellar") {
        return parts.get(index + 2).map(|part| part.to_string());
    }
    let bin = parts.len().checked_sub(2)?;
    if parts.get(bin) == Some(&"bin")
        && parts
            .iter()
            .any(|part| matches!(*part, ".nvm" | ".pyenv" | ".rbenv" | ".asdf" | "mise"))
    {
        return parts.get(bin.checked_sub(1)?).map(|part| part.to_string());
    }
    None
}

fn commands(catalog: &mut Catalog, path: &Path, source: &str, include_unknown: bool) {
    for entry in entries(path) {
        let binary = entry.path();
        let name = leaf(&binary);
        let kind = if RUNTIMES.contains(&name) || name == "R" {
            Kind::Runtime
        } else if CLIS.contains(&name) || include_unknown {
            Kind::Cli
        } else {
            continue;
        };
        let Ok(meta) = entry.metadata().and_then(|_| binary.metadata()) else {
            continue;
        };
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            continue;
        }
        let label = if path == Path::new("/usr/bin")
            && matches!(
                name,
                "swift"
                    | "git"
                    | "clang"
                    | "gcc"
                    | "python3"
                    | "xcodebuild"
                    | "xcrun"
                    | "java"
                    | "javac"
            ) {
            "macOS-Starter"
        } else {
            source
        };
        catalog.add(
            name,
            kind,
            label,
            &binary,
            managed_binary_version(&binary),
            None,
        );
    }
}

fn runtimes(catalog: &mut Catalog, base: &Path, name: &str, source: &str) -> Vec<PathBuf> {
    let installs = dirs(base);
    for path in &installs {
        let version = leaf(path);
        if version.starts_with('.') {
            continue;
        }
        catalog.add(
            name,
            Kind::Runtime,
            source,
            path,
            Some(version.to_owned()),
            None,
        );
    }
    installs
}

fn npm_packages(catalog: &mut Catalog, base: &Path) {
    for folder in dirs(base)
        .into_iter()
        .filter(|path| !leaf(path).starts_with('.'))
    {
        let packages = if leaf(&folder).starts_with('@') {
            dirs(folder)
        } else {
            vec![folder]
        };
        for package in packages {
            let Some(manifest) = json(&package.join("package.json")) else {
                continue;
            };
            let Some(name) = manifest.get("name").and_then(Value::as_str) else {
                continue;
            };
            catalog.add(
                name,
                Kind::Package,
                "npm global",
                &package,
                text(manifest.get("version")),
                None,
            );
        }
    }
}

fn package_bins(catalog: &mut Catalog, root: &Path, source: &str) {
    for record in entries(root) {
        let path = record.path();
        let Some(name) = leaf(&path).strip_suffix(".dist-info") else {
            continue;
        };
        let Some((package, version)) = name.rsplit_once('-') else {
            continue;
        };
        catalog.add(
            package,
            Kind::Package,
            source,
            &path,
            Some(version.into()),
            None,
        );
    }
}

fn installed_ecosystems(catalog: &mut Catalog) {
    let home = catalog.home;
    for (manager, path) in [
        ("asdf", ".asdf/installs"),
        ("mise", ".local/share/mise/installs"),
        ("mise", ".mise/installs"),
    ] {
        for family in dirs(home.join(path)) {
            runtimes(catalog, &family, leaf(&family), manager);
        }
    }
    for (source, path, name) in [
        ("rustup", ".rustup/toolchains", "Rust"),
        ("pyenv", ".pyenv/versions", "Python"),
        ("rbenv", ".rbenv/versions", "Ruby"),
        ("SDKMAN!", ".sdkman/candidates/java", "Java"),
        ("Volta", ".volta/tools/image/node", "Node.js"),
        ("Nodenv", ".nodenv/versions", "Node.js"),
    ] {
        runtimes(catalog, &home.join(path), name, source);
    }
    for path in [
        "/Library/Java/JavaVirtualMachines".into(),
        home.join("Library/Java/JavaVirtualMachines"),
    ] {
        runtimes(catalog, &path, "Java", "JDK");
    }
    let nvm = runtimes(catalog, &home.join(".nvm/versions/node"), "Node.js", "nvm");
    let fnm = runtimes(
        catalog,
        &home.join(".local/share/fnm/node-versions"),
        "Node.js",
        "fnm",
    );
    for path in nvm
        .into_iter()
        .map(|path| path.join("lib/node_modules"))
        .chain(
            fnm.into_iter()
                .map(|path| path.join("installation/lib/node_modules")),
        )
        .chain([
            "/opt/homebrew/lib/node_modules".into(),
            "/usr/local/lib/node_modules".into(),
            home.join(".npm-global/lib/node_modules"),
            home.join(".local/lib/node_modules"),
        ])
    {
        npm_packages(catalog, &path);
    }
    for root in [
        ".local/share/pipx/venvs",
        ".local/pipx/venvs",
        ".local/share/uv/tools",
    ] {
        let source = if root.contains("/uv/") {
            "uv tool"
        } else {
            "pipx"
        };
        for path in dirs(home.join(root)) {
            let data = json(&path.join("pipx_metadata.json"));
            let version = data
                .as_ref()
                .and_then(|value| value.pointer("/main_package/package_version"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if path.join("pyvenv.cfg").is_file() {
                catalog.add(leaf(&path), Kind::Package, source, &path, version, None);
            }
        }
    }
    for python in dirs(home.join("Library/Python")) {
        package_bins(
            catalog,
            &python.join("lib/python/site-packages"),
            "Python Benutzerpaket",
        );
    }
    if let Some(cargo) = json(&home.join(".cargo/.crates2.json")) {
        if let Some(installs) = cargo.get("installs").and_then(Value::as_object) {
            for item in installs.keys() {
                let mut tokens = item.split_whitespace();
                if let (Some(name), Some(version)) = (tokens.next(), tokens.next()) {
                    catalog.add(
                        name,
                        Kind::Package,
                        "cargo install",
                        &home.join(".cargo/bin"),
                        Some(version.into()),
                        None,
                    );
                }
            }
        }
    }
}

fn history_entry(line: &str, latest: u64) -> Option<(&str, u64)> {
    let (metadata, command) = line.strip_prefix(": ")?.split_once(';')?;
    let (time, duration) = metadata.split_once(':')?;
    duration.parse::<u64>().ok()?;
    let time = time.parse::<u64>().ok()?;
    if !(946_684_800..=latest).contains(&time) {
        return None;
    }
    let mut words = command.split_ascii_whitespace();
    let mut name = words.next()?;
    if matches!(name, "sudo" | "command" | "time") {
        name = words.next()?;
    }
    let name = name.strip_prefix("./").unwrap_or(name).rsplit('/').next()?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return None;
    }
    Some((name, time))
}

fn command_times(home: &Path, names: &HashSet<String>) -> (HashMap<String, u64>, &'static str) {
    const LIMIT: u64 = 8 * 1024 * 1024;
    let Ok(mut file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(home.join(".zsh_history"))
    else {
        return (HashMap::new(), "unavailable");
    };
    let Ok(meta) = file.metadata() else {
        return (HashMap::new(), "unavailable");
    };
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || file
            .seek(SeekFrom::Start(meta.len().saturating_sub(LIMIT)))
            .is_err()
    {
        return (HashMap::new(), "unavailable");
    }
    let latest = now();
    let mut values = HashMap::<String, u64>::new();
    let mut known_timestamps = false;
    let mut reader = BufReader::new(file.take(LIMIT));
    let mut line = Vec::new();
    if meta.len() > LIMIT {
        let _ = reader.read_until(b'\n', &mut line);
    }
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            _ => {}
        }
        let Ok(content) = std::str::from_utf8(&line) else {
            continue;
        };
        let Some((name, at)) = history_entry(content, latest) else {
            continue;
        };
        known_timestamps = true;
        if names.contains(name) {
            values
                .entry(name.to_owned())
                .and_modify(|value| *value = (*value).max(at))
                .or_insert(at);
        }
    }
    (
        values,
        if known_timestamps {
            "checked"
        } else {
            "undated"
        },
    )
}

pub(crate) fn scan<'a>(
    home: &Path,
    apps: impl Iterator<Item = &'a AppBundle>,
    include_history: bool,
) -> ToolReport {
    let mut catalog = Catalog {
        home,
        rows: Vec::new(),
        seen: HashSet::new(),
    };
    for app in apps {
        if app.id.starts_with("com.jetbrains.")
            || app.id.starts_with("com.microsoft.vscode")
            || app.id.starts_with("com.apple.dt.xcode")
            || app.id.starts_with("dev.zed.")
            || app.id.starts_with("com.todesktop.230313mzl4w4u92")
            || matches!(
                app.id.as_str(),
                "com.docker.docker"
                    | "com.sublimetext.4"
                    | "com.panic.nova"
                    | "com.google.android.studio"
                    | "org.rstudio.rstudio"
                    | "io.podman_desktop.podman-desktop"
                    | "com.postmanlabs.mac"
                    | "com.insomnia.app"
                    | "com.sequel-ace.sequel-ace"
            )
        {
            catalog.add(
                &app.name,
                Kind::Environment,
                "macOS-App",
                &app.path,
                app.version.clone(),
                None,
            );
        }
    }
    for prefix in ["/opt/homebrew", "/usr/local"] {
        homebrew(&mut catalog, Path::new(prefix));
    }
    installed_ecosystems(&mut catalog);
    let mut bin_paths = BTreeSet::new();
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path).filter(|path| path.is_absolute()) {
            if bin_paths.insert(dir.clone()) {
                commands(&mut catalog, &dir, "PATH", false);
            }
        }
    }
    for (dir, source, all) in [
        ("/opt/homebrew/bin".into(), "Homebrew", false),
        ("/usr/local/bin".into(), "Lokales bin", true),
        ("/opt/local/bin".into(), "MacPorts", false),
        ("/usr/bin".into(), "macOS", false),
        (
            "/Library/Developer/CommandLineTools/usr/bin".into(),
            "Xcode Command Line Tools",
            false,
        ),
        (home.join(".cargo/bin"), "Cargo / Rustup", true),
        (home.join(".local/bin"), "Benutzer-bin", true),
        (home.join("bin"), "Benutzer-bin", true),
        (home.join(".bun/bin"), "Bun", true),
        (home.join(".volta/bin"), "Volta", true),
        (home.join(".docker/bin"), "Docker Desktop", true),
        (home.join(".local/share/pnpm"), "pnpm", false),
    ] {
        // A known installation path can provide more coverage than PATH.
        commands(&mut catalog, &dir, source, all);
    }
    let installed: Vec<_> = catalog
        .rows
        .iter()
        .filter_map(|tool| tool.removal_plan.clone())
        .collect();
    for tool in &mut catalog.rows {
        if tool.removal_plan.is_some() || !matches!(tool.kind, Kind::Cli | Kind::Runtime) {
            continue;
        }
        let plan = installed
            .iter()
            .filter(|plan| plan.covers(&tool.actual_path))
            .max_by_key(|plan| plan.depth())
            .cloned()
            .or_else(|| tool_removal::standalone(home, &tool.actual_path));
        tool.removal = plan.as_ref().map(Removal::label);
        tool.removal_target = plan.as_ref().map(|plan| plan.target().to_owned());
        tool.removal_plan = plan;
    }
    let history = if include_history {
        let names = catalog
            .rows
            .iter()
            .filter(|row| matches!(row.kind, Kind::Runtime | Kind::Cli))
            .map(|row| row.name.clone())
            .collect();
        let (matches, status) = command_times(home, &names);
        for row in &mut catalog.rows {
            if matches!(row.kind, Kind::Runtime | Kind::Cli) {
                row.last_used = matches.get(&row.name).copied();
            }
        }
        status
    } else {
        "off"
    };
    catalog.rows.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let mut caches: Vec<_> = [
        ("npm", ".npm/_cacache"),
        ("Yarn", "Library/Caches/Yarn"),
        ("pnpm", "Library/pnpm/store"),
        ("Pip", "Library/Caches/pip"),
        ("uv", ".cache/uv"),
        ("Homebrew", "Library/Caches/Homebrew"),
        ("Cargo Registry", ".cargo/registry/cache"),
        ("Go-Module", "go/pkg/mod/cache"),
        ("Gradle", ".gradle/caches"),
        ("Maven", ".m2/repository"),
        ("Bun", ".bun/install/cache"),
        ("SwiftPM", "Library/Caches/org.swift.swiftpm"),
    ]
    .into_par_iter()
    .enumerate()
    .filter_map(|(index, (name, relative))| {
        let path = home.join(relative);
        if !path
            .canonicalize()
            .ok()
            .is_some_and(|real| real == path && real.is_dir())
        {
            return None;
        }
        let detail = measure(&path);
        let removal = (detail.bytes > 0 && detail.skipped == 0)
            .then(|| tool_removal::cache_plan(home, name, &path))
            .flatten();
        Some(DevCache {
            id: index + 1,
            name,
            path: display(&path, home),
            bytes: detail.bytes,
            updated_at: detail.newest,
            incomplete: detail.skipped != 0,
            removal: removal.as_ref().map(Removal::label),
            entry: DevCacheEntry {
                name,
                path,
                removal,
            },
        })
    })
    .collect();
    caches.sort_by_key(|cache| std::cmp::Reverse(cache.bytes));
    ToolReport {
        rows: catalog.rows,
        caches,
        history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_history_extracts_only_a_direct_command_name_and_valid_timestamp() {
        assert_eq!(
            history_entry(
                ": 1700000000:0;/opt/homebrew/bin/node --argument",
                1800000000
            ),
            Some(("node", 1700000000))
        );
        assert_eq!(
            history_entry(": 1700000001:0;sudo docker ps", 1800000000),
            Some(("docker", 1700000001))
        );
        assert_eq!(history_entry("git status", 1800000000), None);
        assert_eq!(history_entry(": 1800000001:0;git status", 1800000000), None);
        assert_eq!(
            history_entry(": 1700000000:0;TOKEN=secret git status", 1800000000),
            None
        );
    }
}
