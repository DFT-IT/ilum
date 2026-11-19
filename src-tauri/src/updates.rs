//! Checks and installs verified release artifacts and restarts after installation.

use serde::Serialize;
use tauri::{async_runtime::Mutex, AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct UpdateState(Mutex<bool>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    version: String,
    updates_configured: bool,
}

#[derive(Serialize)]
pub struct AvailableUpdate {
    version: String,
    notes: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
    installing: bool,
}

pub(crate) fn updates_configured(app: &AppHandle) -> bool {
    app.config().plugins.0.get("updater").is_some_and(|config| {
        config
            .get("pubkey")
            .and_then(|value| value.as_str())
            .is_some_and(|key| !key.is_empty())
            && config
                .get("endpoints")
                .and_then(|value| value.as_array())
                .is_some_and(|urls| !urls.is_empty())
    })
}

#[tauri::command]
pub fn release_info(app: AppHandle) -> ReleaseInfo {
    ReleaseInfo {
        version: app.package_info().version.to_string(),
        updates_configured: updates_configured(&app),
    }
}

async fn available(app: &AppHandle) -> Result<Option<Update>, String> {
    if !updates_configured(app) {
        return Err("Dieser Build hat keinen Update-Kanal.".into());
    }
    let updater = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    updater.check().await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<AvailableUpdate>, String> {
    Ok(available(&app).await?.map(|update| AvailableUpdate {
        version: update.version,
        notes: update.body,
    }))
}

/// Rechecks the selected version and lets the updater verify the archive before installation.
#[tauri::command]
pub async fn install_update(app: AppHandle, version: String) -> Result<(), String> {
    let state = app.state::<UpdateState>();
    let mut installed = state
        .0
        .try_lock()
        .map_err(|_| "Ein Update wird bereits installiert.")?;
    if *installed {
        return Err("Starte Ilum neu, um das installierte Update zu verwenden.".into());
    }
    let update = available(&app)
        .await?
        .filter(|update| update.version == version)
        .ok_or("Das ausgewählte Update ist nicht mehr verfügbar. Prüfe erneut auf Updates.")?;
    let progress_window = app.get_webview_window("main");
    let mut downloaded = 0_u64;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                if let Some(window) = &progress_window {
                    let _ = window.emit(
                        "update-progress",
                        UpdateProgress {
                            downloaded,
                            total,
                            installing: false,
                        },
                    );
                }
            },
            || {
                if let Some(window) = &progress_window {
                    let _ = window.emit(
                        "update-progress",
                        UpdateProgress {
                            downloaded: 0,
                            total: None,
                            installing: true,
                        },
                    );
                }
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    *installed = true;
    Ok(())
}

#[tauri::command]
pub async fn restart_after_update(app: AppHandle) -> Result<(), String> {
    let state = app.state::<UpdateState>();
    if !*state.0.lock().await {
        return Err("Es ist noch kein Update installiert.".into());
    }
    app.request_restart();
    Ok(())
}
