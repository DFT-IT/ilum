use serde::Serialize;
use std::{
    path::Path,
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};
use sysinfo::{Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};
use tauri::{State, WebviewWindow};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    cpu: f32,
    cores: usize,
    ram_used: u64,
    ram_total: u64,
    disk_used: Option<u64>,
    disk_total: Option<u64>,
    uptime: u64,
    computer_name: Option<String>,
    memory_pressure: Option<&'static str>,
    processes: Vec<ProcessRow>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessRow {
    pid: u32,
    name: String,
    memory: u64,
    cpu: f32,
    started: u64,
    can_terminate: bool,
}

#[derive(Default)]
pub struct Monitor {
    system: System,
    last: Option<(Instant, Snapshot)>,
    computer_name: Option<(Instant, Option<String>)>,
}

fn owned_by_user(process: &sysinfo::Process) -> bool {
    let uid = unsafe { libc::geteuid() };
    uid != 0
        && process.pid().as_u32() > 1
        && process.pid().as_u32() != std::process::id()
        && process.user_id().is_some_and(|value| **value == uid)
        && process.exe().is_some_and(|path| {
            path.is_absolute()
                && !["/System", "/usr/libexec", "/Library/Apple"]
                    .iter()
                    .any(|root| path.starts_with(root))
        })
        && !matches!(
            process.name().to_str(),
            Some("Finder" | "Dock" | "loginwindow" | "SystemUIServer" | "WindowServer")
        )
}

fn process_details() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet)
}

fn computer_name() -> Option<String> {
    Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(System::host_name)
}

fn memory_pressure() -> Option<&'static str> {
    let mut pressure: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&pressure);
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&mut pressure as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    match pressure {
        1 => Some("normal"),
        2 => Some("warning"),
        4 => Some("critical"),
        _ => None,
    }
}

pub(crate) fn startup_disk(disks: &Disks) -> Option<&sysinfo::Disk> {
    disks
        .iter()
        .find(|disk| disk.mount_point() == Path::new("/System/Volumes/Data"))
        .or_else(|| {
            disks
                .iter()
                .find(|disk| disk.mount_point() == Path::new("/"))
        })
}

impl Monitor {
    fn snapshot(&mut self) -> Snapshot {
        if let Some((at, snapshot)) = &self.last {
            if at.elapsed() < Duration::from_millis(1200) {
                return snapshot.clone();
            }
        } else {
            self.system.refresh_cpu_usage();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, process_details());

        let mut processes: Vec<_> = self
            .system
            .processes()
            .values()
            .filter(|p| p.memory() > 0)
            .map(|process| ProcessRow {
                pid: process.pid().as_u32(),
                name: process.name().to_string_lossy().into_owned(),
                memory: process.memory(),
                cpu: process.cpu_usage(),
                started: process.start_time(),
                can_terminate: owned_by_user(process),
            })
            .collect();
        processes.sort_unstable_by(|a, b| b.cpu.total_cmp(&a.cpu));
        let cpu_leaders: std::collections::HashSet<_> = processes
            .iter()
            .take(30)
            .map(|process| process.pid)
            .collect();
        processes.sort_unstable_by_key(|process| std::cmp::Reverse(process.memory));
        let mut rank = 0;
        processes.retain(|process| {
            rank += 1;
            rank <= 30 || cpu_leaders.contains(&process.pid)
        });
        let disks = Disks::new_with_refreshed_list();
        let disk = startup_disk(&disks);
        if self
            .computer_name
            .as_ref()
            .is_none_or(|(checked, _)| checked.elapsed() > Duration::from_secs(60))
        {
            self.computer_name = Some((Instant::now(), computer_name()));
        }
        let snapshot = Snapshot {
            cpu: self.system.global_cpu_usage(),
            cores: self.system.cpus().len(),
            ram_used: self.system.used_memory(),
            ram_total: self.system.total_memory(),
            disk_used: disk.map(|disk| disk.total_space().saturating_sub(disk.available_space())),
            disk_total: disk.map(|disk| disk.total_space()),
            uptime: System::uptime(),
            computer_name: self
                .computer_name
                .as_ref()
                .and_then(|(_, name)| name.clone()),
            memory_pressure: memory_pressure(),
            processes,
        };
        self.last = Some((Instant::now(), snapshot.clone()));
        snapshot
    }
}

#[tauri::command]
pub async fn telemetry(window: WebviewWindow) -> Result<Option<Snapshot>, String> {
    if !window.is_visible().unwrap_or(false) {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager;
        let state = window.state::<Mutex<Monitor>>();
        let mut monitor = state
            .lock()
            .map_err(|_| "Systemdaten sind vorübergehend nicht verfügbar.")?;
        Ok(Some(monitor.snapshot()))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn terminate_process(
    state: State<'_, Mutex<Monitor>>,
    pid: u32,
    started: u64,
) -> Result<(), String> {
    let mut monitor = state
        .lock()
        .map_err(|_| "Prozessliste ist nicht verfügbar.")?;
    let pid = Pid::from_u32(pid);
    let mut current = System::new();
    current.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, process_details());
    let process = current
        .process(pid)
        .filter(|process| started != 0 && process.start_time() == started && owned_by_user(process))
        .ok_or("Der Prozess ist nicht mehr verfügbar oder kann hier nicht beendet werden.")?;
    if process.kill_with(Signal::Term) != Some(true) {
        return Err("macOS hat das Signal zum Beenden abgelehnt.".into());
    }
    monitor.last = None;
    Ok(())
}
