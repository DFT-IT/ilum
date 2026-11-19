use std::{
    ffi::{CString, OsString},
    fs, io,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use trash::macos::{DeleteMethod, TrashContextExtMacos};

pub fn move_to_trash(path: &Path) -> Result<(), String> {
    let mut trash = trash::TrashContext::new();
    trash.set_delete_method(DeleteMethod::NsFileManager);
    trash.delete(path).map_err(|error| error.to_string())
}

pub fn play_completion() {
    std::thread::spawn(|| {
        let _ = Command::new("/usr/bin/afplay")
            .args(["-v", "0.45", "/System/Library/Sounds/Glass.aiff"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

pub fn wait_until(child: &mut Child, limit: Duration) -> io::Result<ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Der Vorgang dauert zu lange.",
            ));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new() -> io::Result<Self> {
        let template = std::env::temp_dir().join("bad-batch-preview.XXXXXX");
        let mut bytes =
            CString::new(template.as_os_str().as_encoded_bytes())?.into_bytes_with_nul();
        // mkdtemp replaces the final six Xs and creates a private 0700 directory.
        if unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        bytes.pop();
        Ok(Self(PathBuf::from(OsString::from_vec(bytes))))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
