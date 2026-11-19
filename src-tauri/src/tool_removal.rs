use crate::system_actions::{move_to_trash, wait_until, ScratchDir};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct Removal {
    guard: PathBuf,
    identity: (u64, u64, i64, i64),
    action: Action,
    target: String,
}

#[derive(Clone)]
enum Action {
    Trash,
    Manager {
        label: &'static str,
        binary: PathBuf,
        args: Vec<OsString>,
        env: Vec<(&'static str, PathBuf)>,
    },
}

impl Removal {
    fn identity(path: &Path) -> Option<(u64, u64, i64, i64)> {
        let metadata = path.symlink_metadata().ok()?;
        if metadata.file_type().is_symlink() || path.canonicalize().ok()?.as_path() != path {
            return None;
        }
        Some((
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ))
    }

    fn new(path: &Path, action: Action) -> Option<Self> {
        let guard = path.canonicalize().ok()?;
        Some(Self {
            target: path.file_name()?.to_string_lossy().into_owned(),
            identity: Self::identity(&guard)?,
            guard,
            action,
        })
    }

    pub(crate) fn label(&self) -> &'static str {
        match &self.action {
            Action::Trash => "Papierkorb",
            Action::Manager { label, .. } => label,
        }
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn path(&self) -> &Path {
        &self.guard
    }

    pub(crate) fn covers(&self, binary: &Path) -> bool {
        binary.starts_with(&self.guard)
    }

    pub(crate) fn depth(&self) -> usize {
        self.guard.components().count()
    }

    pub(crate) fn remove(&self) -> Result<(), String> {
        if Self::identity(&self.guard) != Some(self.identity) {
            return Err("Die Installation hat sich geändert. Aktualisiere das Inventar und prüfe sie erneut.".into());
        }
        match &self.action {
            Action::Trash => move_to_trash(&self.guard),
            Action::Manager {
                label,
                binary,
                args,
                env,
            } => {
                let mut process = Command::new(binary);
                process
                    .args(args)
                    .envs(env.iter().map(|(key, value)| (*key, value)))
                    .current_dir("/")
                    .env("HOMEBREW_NO_AUTO_UPDATE", "1")
                    .env("HOMEBREW_NO_ANALYTICS", "1")
                    .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
                    .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
                let _config = if *label == "npm" {
                    let scratch = ScratchDir::new().map_err(|error| error.to_string())?;
                    let user = scratch.path().join("user.npmrc");
                    let global = scratch.path().join("global.npmrc");
                    for path in [&user, &global] {
                        fs::File::create(path).map_err(|error| error.to_string())?;
                    }
                    process
                        .env("NPM_CONFIG_USERCONFIG", user)
                        .env("NPM_CONFIG_GLOBALCONFIG", global)
                        .env("NPM_CONFIG_CACHE", scratch.path().join("cache"));
                    Some(scratch)
                } else if *label == "pnpm" {
                    let scratch = ScratchDir::new().map_err(|error| error.to_string())?;
                    let mut cache_option = OsString::from("--config.cache-dir=");
                    cache_option.push(scratch.path());
                    let mut probe = Command::new(binary)
                        .args(["store", "path", "--store-dir"])
                        .arg(&self.guard)
                        .arg(&cache_option)
                        .envs(env.iter().map(|(key, value)| (*key, value)))
                        .env("COREPACK_ENABLE_NETWORK", "0")
                        .current_dir("/")
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn()
                        .map_err(|error| error.to_string())?;
                    let status = wait_until(&mut probe, Duration::from_secs(8))
                        .map_err(|error| error.to_string())?;
                    let mut output = String::new();
                    if let Some(stream) = probe.stdout.take() {
                        stream
                            .take(4096)
                            .read_to_string(&mut output)
                            .map_err(|error| error.to_string())?;
                    }
                    let active_store = Path::new(output.trim()).canonicalize().ok();
                    if !status.success()
                        || !active_store
                            .is_some_and(|path| path.is_dir() && path.starts_with(&self.guard))
                    {
                        return Err("Die installierte pnpm-Version konnte diesem Store nicht eindeutig zugeordnet werden.".into());
                    }
                    process
                        .arg(cache_option)
                        .env("COREPACK_ENABLE_NETWORK", "0");
                    Some(scratch)
                } else {
                    None
                };
                let mut child = process.spawn().map_err(|error| error.to_string())?;
                let stderr = child.stderr.take().expect("stderr is piped");
                let reader = std::thread::spawn(move || {
                    let mut output = Vec::new();
                    let _ = stderr.take(32 * 1024).read_to_end(&mut output);
                    String::from_utf8_lossy(&output).into_owned()
                });
                let status = wait_until(&mut child, Duration::from_secs(90))
                    .map_err(|error| error.to_string())?;
                let output = reader.join().unwrap_or_default();
                if status.success() {
                    Ok(())
                } else {
                    let short: String = output.trim().chars().take(500).collect();
                    Err(if short.is_empty() {
                        "Der Paketmanager hat das Entfernen abgelehnt.".into()
                    } else {
                        short
                    })
                }
            }
        }
    }
}

fn safe_name(name: &str, scoped: bool) -> bool {
    let allowed = |part: &str| {
        part.as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._+-@".contains(&byte))
    };
    if scoped {
        if let Some((namespace, package)) =
            name.strip_prefix('@').and_then(|name| name.split_once('/'))
        {
            return allowed(namespace) && allowed(package);
        }
    }
    allowed(name)
}

fn binary(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.is_file())
}

fn manager(
    path: &Path,
    label: &'static str,
    binary: PathBuf,
    args: Vec<OsString>,
    env: Vec<(&'static str, PathBuf)>,
) -> Option<Removal> {
    Removal::new(
        path,
        Action::Manager {
            label,
            binary,
            args,
            env,
        },
    )
}

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn npm(home: &Path, name: &str, path: &Path) -> Option<Removal> {
    if name == "npm" || !safe_name(name, true) {
        return None;
    }
    let modules = path
        .ancestors()
        .find(|path| path.file_name().is_some_and(|part| part == "node_modules"))?;
    let prefix = modules.parent()?.parent()?;
    if modules != prefix.join("lib/node_modules") || modules.join(name) != path {
        return None;
    }
    let manifest = path.join("package.json");
    let config: serde_json::Value =
        serde_json::from_reader(fs::File::open(&manifest).ok()?.take(1024 * 1024)).ok()?;
    if config.get("name")?.as_str()? != name {
        return None;
    }
    let cli_and_runtime = [
        prefix.to_owned(),
        "/opt/homebrew".into(),
        "/usr/local".into(),
    ]
    .into_iter()
    .chain(
        fs::read_dir(home.join(".nvm/versions/node"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|item| item.path()),
    )
    .find_map(|installation| {
        let cli = installation.join("lib/node_modules/npm/bin/npm-cli.js");
        let node = installation.join("bin/node");
        (cli.is_file() && node.is_file()).then_some((cli, node))
    })?;
    let mut command = vec![cli_and_runtime.0.into_os_string()];
    command.extend(args(&[
        "uninstall",
        "--global",
        "--offline",
        "--ignore-scripts",
        "--prefix",
    ]));
    command.push(prefix.as_os_str().to_owned());
    command.extend(args(&["--", name]));
    manager(path, "npm", cli_and_runtime.1, command, vec![])
}

fn python(home: &Path, name: &str, path: &Path) -> Option<Removal> {
    let version = path
        .strip_prefix(home.join("Library/Python"))
        .ok()?
        .iter()
        .next()?
        .to_str()?;
    if !version
        .bytes()
        .all(|part| part.is_ascii_digit() || part == b'.')
    {
        return None;
    }
    let command_name = format!("python{version}");
    let executable = binary([
        format!("/Library/Frameworks/Python.framework/Versions/{version}/bin/{command_name}")
            .into(),
        format!("/opt/homebrew/bin/{command_name}").into(),
        format!("/usr/local/bin/{command_name}").into(),
        "/Library/Developer/CommandLineTools/usr/bin/python3".into(),
    ])?;
    const VERIFY_AND_REMOVE: &str = r#"
from importlib.metadata import distribution
from pathlib import Path
import sys
name, expected = sys.argv[1:]
try:
    installed = distribution(name)
except Exception:
    sys.exit("Dieses Paket ist für den erkannten Python-Interpreter nicht erreichbar.")
if Path(installed._path).resolve() != Path(expected).resolve():
    sys.exit("Python verweist auf eine andere Installation des Pakets. Bitte prüfe den Pfad.")
from pip._internal.cli.main import main
sys.exit(main(["uninstall", "--yes", name]))
"#;
    let mut command = args(&["-E", "-c", VERIFY_AND_REMOVE, name]);
    command.push(path.as_os_str().to_owned());
    manager(path, "pip", executable, command, vec![])
}

pub(crate) fn plan(home: &Path, name: &str, source: &str, path: &Path) -> Option<Removal> {
    let mut plan = match source {
        "npm global" => npm(home, name, path),
        "Homebrew" | "Homebrew Cask" if safe_name(name, false) => {
            let (directory, flag) = if source == "Homebrew" {
                ("Cellar", "--formula")
            } else {
                ("Caskroom", "--cask")
            };
            let prefix = path.parent()?.parent()?.parent()?;
            if path.parent()?.file_name()? != name
                || path.parent()?.parent()? != prefix.join(directory)
            {
                return None;
            }
            manager(
                path,
                "Homebrew",
                binary([prefix.join("bin/brew")])?,
                args(&["uninstall", flag, name]),
                vec![],
            )
        }
        "pipx" | "uv tool" if safe_name(name, false) => {
            let label = if source == "pipx" { "pipx" } else { "uv" };
            let executable = binary([
                home.join(format!(".local/bin/{label}")),
                format!("/opt/homebrew/bin/{label}").into(),
                format!("/usr/local/bin/{label}").into(),
            ])?;
            let (command, env) = if label == "pipx" {
                (
                    args(&["uninstall", name]),
                    vec![
                        ("PIPX_HOME", path.parent()?.parent()?.to_owned()),
                        ("PIPX_BIN_DIR", home.join(".local/bin")),
                    ],
                )
            } else {
                (
                    args(&["tool", "uninstall", name]),
                    vec![
                        ("UV_TOOL_DIR", path.parent()?.to_owned()),
                        ("UV_TOOL_BIN_DIR", home.join(".local/bin")),
                    ],
                )
            };
            manager(path, label, executable, command, env)
        }
        "Python Benutzerpaket" if safe_name(name, false) => python(home, name, path),
        "cargo install" if safe_name(name, false) => {
            let root = home.join(".cargo");
            let mut command = args(&["uninstall", "--root"]);
            command.push(root.as_os_str().to_owned());
            command.extend(args(&["--", name]));
            manager(
                &root.join(".crates2.json"),
                "Cargo",
                binary([root.join("bin/cargo")])?,
                command,
                vec![("CARGO_HOME", root.clone())],
            )
        }
        "rustup" => {
            let toolchain = path.file_name()?.to_str()?;
            if !safe_name(toolchain, false) {
                return None;
            }
            let executable = binary([
                home.join(".cargo/bin/rustup"),
                "/opt/homebrew/bin/rustup".into(),
                "/usr/local/bin/rustup".into(),
            ])?;
            manager(
                path,
                "Rustup",
                executable,
                args(&["toolchain", "uninstall", toolchain]),
                vec![("RUSTUP_HOME", home.join(".rustup"))],
            )
        }
        "nvm" | "pyenv" | "rbenv" => {
            let directory = match source {
                "nvm" => ".nvm/versions/node",
                "pyenv" => ".pyenv/versions",
                _ => ".rbenv/versions",
            };
            if path.parent()? != home.join(directory)
                || path.symlink_metadata().ok()?.file_type().is_symlink()
            {
                return None;
            }
            Removal::new(path, Action::Trash)
        }
        "macOS-App" => {
            let real = path.canonicalize().ok()?;
            if !path.starts_with("/Applications") && !path.starts_with(home) {
                return None;
            }
            if path != real
                || path.extension()?.to_str()?.to_lowercase() != "app"
                || matches!(name, "Docker" | "Docker Desktop" | "Xcode")
            {
                return None;
            }
            Removal::new(path, Action::Trash)
        }
        _ => None,
    }?;
    plan.target = if matches!(source, "nvm" | "pyenv" | "rbenv" | "rustup") {
        format!("{name} {}", path.file_name()?.to_string_lossy())
    } else {
        name.to_owned()
    };
    Some(plan)
}

pub(crate) fn cache_plan(home: &Path, name: &str, path: &Path) -> Option<Removal> {
    if !path.starts_with(home)
        || path.canonicalize().ok()?.as_path() != path
        || !path
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_dir() && meta.uid() == unsafe { libc::geteuid() })
    {
        return None;
    }
    let mut plan = match name {
        "Maven" => return None,
        "pnpm" => {
            let nvm = fs::read_dir(home.join(".nvm/versions/node"))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("bin/pnpm"));
            let pnpm = binary(
                [
                    home.join(".local/share/pnpm/pnpm"),
                    home.join(".local/bin/pnpm"),
                    "/opt/homebrew/bin/pnpm".into(),
                    "/usr/local/bin/pnpm".into(),
                ]
                .into_iter()
                .chain(nvm),
            )?;
            let search = std::env::join_paths([
                pnpm.parent()?,
                Path::new("/opt/homebrew/bin"),
                Path::new("/usr/local/bin"),
                Path::new("/usr/bin"),
                Path::new("/bin"),
            ])
            .ok()?;
            let mut command = args(&["store", "prune", "--store-dir"]);
            command.push(path.as_os_str().to_owned());
            manager(path, "pnpm", pnpm, command, vec![("PATH", search.into())])
        }
        _ => Removal::new(path, Action::Trash),
    }?;
    plan.target = name.to_owned();
    Some(plan)
}

pub(crate) fn standalone(home: &Path, path: &Path) -> Option<Removal> {
    if path.parent()? != home.join("bin") && path.parent()? != home.join(".local/bin") {
        return None;
    }
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
        return None;
    }
    Removal::new(path, Action::Trash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_cannot_be_confused_for_options_or_paths() {
        assert!(safe_name("@angular/cli", true));
        assert!(safe_name("python@3.13", false));
        assert!(!safe_name("@angular/../cli", true));
        assert!(!safe_name("--force", true));
        assert!(!safe_name("http://host/package", true));
    }

    #[test]
    fn only_known_standalone_bins_can_be_trashed() {
        let home = Path::new("/Users/fixture");
        assert!(standalone(home, Path::new("/usr/bin/git")).is_none());
        assert!(plan(
            home,
            "Xcode",
            "macOS-App",
            Path::new("/System/Applications/Xcode.app")
        )
        .is_none());
    }
}
