//! Bridges local account pictures, folder access checks, and AppKit integration.

use std::{fs, io, path::PathBuf, process::Command};

use base64::{engine::general_purpose::STANDARD, Engine};
use objc2::{
    class, msg_send,
    rc::autoreleasepool,
    runtime::{AnyObject, Bool},
};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use crate::system_actions::ScratchDir;

type Obj = *mut AnyObject;

#[link(name = "OpenDirectory", kind = "framework")]
extern "C" {
    static kODRecordTypeUsers: Obj;
    static kODAttributeTypeJPEGPhoto: Obj;
    static kODAttributeTypePicture: Obj;
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    fn NSUserName() -> Obj;
}

pub fn round_popover<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    let ns_window = window.ns_window()?.cast::<AnyObject>();
    // setup calls this on the main thread. Clip the webview with the public AppKit layer.
    unsafe {
        let clear: Obj = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![ns_window, setOpaque: Bool::NO];
        let _: () = msg_send![ns_window, setBackgroundColor: clear];
        let _: () = msg_send![ns_window, setHasShadow: Bool::YES];
        let view: Obj = msg_send![ns_window, contentView];
        if !view.is_null() {
            let _: () = msg_send![view, setWantsLayer: Bool::YES];
            let layer: Obj = msg_send![view, layer];
            if !layer.is_null() {
                let _: () = msg_send![layer, setCornerRadius: 15.0_f64];
                let _: () = msg_send![layer, setMasksToBounds: Bool::YES];
            }
        }
    }
    Ok(())
}

enum AccountPicture {
    Image(Vec<u8>),
    File(PathBuf),
}

fn local_account_picture() -> Option<AccountPicture> {
    autoreleasepool(|_| unsafe {
        let no_error = std::ptr::null_mut::<Obj>();
        let no_attributes = std::ptr::null_mut::<AnyObject>();
        let session: Obj = msg_send![class!(ODSession), defaultSession];
        let node: Obj =
            msg_send![class!(ODNode), nodeWithSession: session, type: 0x2201_u32, error: no_error];
        let username = NSUserName();
        if node.is_null() || username.is_null() {
            return None;
        }
        let record: Obj = msg_send![node, recordWithRecordType: kODRecordTypeUsers, name: username, attributes: no_attributes, error: no_error];
        if record.is_null() {
            return None;
        }
        let pictures: Obj =
            msg_send![record, valuesForAttribute: kODAttributeTypeJPEGPhoto, error: no_error];
        let photo: Obj = msg_send![pictures, firstObject];
        let is_data: Bool = msg_send![photo, isKindOfClass: class!(NSData)];
        if is_data.as_bool() {
            let len: usize = msg_send![photo, length];
            if (1..=8 * 1024 * 1024).contains(&len) {
                let bytes: *const u8 = msg_send![photo, bytes];
                if !bytes.is_null() {
                    return Some(AccountPicture::Image(
                        std::slice::from_raw_parts(bytes, len).to_vec(),
                    ));
                }
            }
        }
        let paths: Obj =
            msg_send![record, valuesForAttribute: kODAttributeTypePicture, error: no_error];
        let path: Obj = msg_send![paths, firstObject];
        let is_string: Bool = msg_send![path, isKindOfClass: class!(NSString)];
        if !is_string.as_bool() {
            return None;
        }
        let utf8: *const std::ffi::c_char = msg_send![path, UTF8String];
        if utf8.is_null() {
            return None;
        }
        let path = PathBuf::from(std::ffi::CStr::from_ptr(utf8).to_str().ok()?);
        (path.is_absolute()
            && path
                .metadata()
                .ok()
                .is_some_and(|file| file.is_file() && file.len() <= 8 * 1024 * 1024))
        .then_some(AccountPicture::File(path))
    })
}

fn render_account_picture() -> Option<String> {
    let scratch = ScratchDir::new().ok()?;
    let input = match local_account_picture()? {
        AccountPicture::Image(bytes) => {
            let path = scratch.path().join("profile.jpg");
            fs::write(&path, bytes).ok()?;
            path
        }
        AccountPicture::File(path) => path,
    };
    let output = scratch.path().join("avatar.jpg");
    let converted = Command::new("/usr/bin/sips")
        .args(["-Z", "96", "-s", "format", "jpeg"])
        .arg(input)
        .arg("--out")
        .arg(&output)
        .output()
        .ok()?;
    if !converted.status.success() {
        return None;
    }
    Some(STANDARD.encode(fs::read(output).ok()?))
}

#[tauri::command]
pub async fn account_picture() -> Option<String> {
    tauri::async_runtime::spawn_blocking(render_account_picture)
        .await
        .ok()
        .flatten()
}

#[derive(Serialize)]
pub struct FolderAccess {
    name: &'static str,
    status: &'static str,
}

/// Tests whether the three standard personal folders are readable by this process.
#[tauri::command]
pub async fn check_folder_access(app: AppHandle) -> Result<Vec<FolderAccess>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = app.path().home_dir().map_err(|error| error.to_string())?;
        Ok([
            ("Desktop", "Schreibtisch"),
            ("Documents", "Dokumente"),
            ("Downloads", "Downloads"),
        ]
        .into_iter()
        .map(|(path, name)| {
            let checked =
                fs::read_dir(home.join(path)).and_then(|mut entries| entries.next().transpose());
            let status = match checked {
                Ok(_) => "readable",
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => "denied",
                Err(error) if error.kind() == io::ErrorKind::NotFound => "missing",
                Err(_) => "unknown",
            };
            FolderAccess { name, status }
        })
        .collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn open_privacy_settings() -> Result<(), String> {
    Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders")
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
