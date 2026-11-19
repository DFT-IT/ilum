#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod dev;
mod dev_impact;
mod files;
mod growth;
mod inventory;
mod macos_integration;
mod monitor;
mod residues;
mod system_actions;
mod tool_removal;
mod toolchain;
mod updates;

use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    ActivationPolicy, AppHandle, Emitter, LogicalPosition, Manager, Rect, Runtime, WebviewWindow,
    WindowEvent,
};

struct TrayMenuItems {
    dashboard: MenuItem<tauri::Wry>,
    growth: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

/// Applies the app language to its native menu and synchronizes both webviews.
#[tauri::command]
fn set_language(app: AppHandle, language: String) -> Result<(), String> {
    let (dashboard, growth, quit) = match language.as_str() {
        "de" => ("Dashboard öffnen", "Was ist gewachsen?", "Ilum beenden"),
        "en" => ("Open Dashboard", "What grew?", "Quit Ilum"),
        _ => return Err("Unsupported language.".into()),
    };
    let items = app.state::<TrayMenuItems>();
    items
        .dashboard
        .set_text(dashboard)
        .map_err(|error| error.to_string())?;
    items
        .growth
        .set_text(growth)
        .map_err(|error| error.to_string())?;
    items
        .quit
        .set_text(quit)
        .map_err(|error| error.to_string())?;
    app.emit("language-changed", language)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_appearance(app: AppHandle, mode: String) -> Result<(), String> {
    let theme = match mode.as_str() {
        "system" => None,
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => return Err("Unbekanntes Erscheinungsbild.".into()),
    };
    app.set_theme(theme);
    app.emit("appearance-changed", mode)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_dashboard(app: AppHandle, growth: Option<bool>) -> Result<(), String> {
    if let Some(popover) = app.get_webview_window("popover") {
        let _ = popover.hide();
    }
    let main = app.get_webview_window("main").ok_or("Dashboard fehlt.")?;
    app.set_activation_policy(ActivationPolicy::Regular)
        .map_err(|error| error.to_string())?;
    main.show()
        .and_then(|_| main.unminimize())
        .and_then(|_| main.set_focus())
        .map_err(|error| error.to_string())?;
    let _ = main.emit("surface-shown", ());
    if growth == Some(true) {
        let _ = main.emit("show-storage-growth", ());
    }
    Ok(())
}

#[tauri::command]
fn close_popover(window: WebviewWindow) -> Result<(), String> {
    if window.label() == "popover" {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn toggle_popover<R: Runtime>(app: &AppHandle<R>, rect: Rect) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("popover") else {
        return Ok(());
    };
    if window.is_visible()? {
        return window.hide();
    }
    let monitors = window.available_monitors()?;
    let monitor = monitors.iter().min_by(|a, b| {
        let distance = |monitor: &tauri::Monitor| {
            let scale = monitor.scale_factor();
            let position = rect.position.to_physical::<i32>(scale);
            let left = monitor.position().x;
            let right = left + monitor.size().width as i32;
            let x = (left - position.x).max(position.x - right).max(0);
            let y = position.y - monitor.position().y;
            (x as f64).hypot(y as f64) / scale
        };
        distance(a).total_cmp(&distance(b))
    });
    let scale = monitor
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(window.scale_factor()?);
    let position = rect.position.to_logical::<f64>(scale);
    let size = rect.size.to_logical::<f64>(scale);
    let width = window
        .inner_size()?
        .to_logical::<f64>(window.scale_factor()?)
        .width;
    let mut x = position.x + (size.width - width) / 2.0;
    let mut y = position.y + size.height + 5.0;
    if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let origin = area.position.to_logical::<f64>(scale);
        let size = area.size.to_logical::<f64>(scale);
        let left = origin.x + 8.0;
        x = x.clamp(left, (origin.x + size.width - width - 8.0).max(left));
        y = y.max(origin.y);
    }
    window.set_position(LogicalPosition::new(x, y))?;
    window.show()?;
    window.set_focus()?;
    let _ = window.emit("surface-shown", ());
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(monitor::Monitor::default()))
        .manage(files::ScanState::default())
        .manage(growth::GrowthState::default())
        .manage(updates::UpdateState::default())
        .manage(dev::DevState::default())
        .manage(inventory::InventoryState::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if updates::updates_configured(app.handle()) {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
            }
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            if let Some(popover) = app.get_webview_window("popover") {
                macos_integration::round_popover(&popover)?;
            }

            let dashboard =
                MenuItem::with_id(app, "dashboard", "Dashboard öffnen", true, None::<&str>)?;
            let growth =
                MenuItem::with_id(app, "growth", "Was ist gewachsen?", true, None::<&str>)?;
            let name = &app.package_info().name;
            let quit =
                MenuItem::with_id(app, "quit", format!("{name} beenden"), true, Some("Cmd+Q"))?;
            let menu = Menu::with_items(app, &[&dashboard, &growth, &quit])?;
            app.manage(TrayMenuItems {
                dashboard,
                growth,
                quit,
            });
            TrayIconBuilder::new()
                .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .tooltip(name.as_str())
                .show_menu_on_left_click(false)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "dashboard" => {
                        let _ = open_dashboard(app.clone(), None);
                    }
                    "growth" => {
                        let _ = open_dashboard(app.clone(), Some(true));
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        if let Err(error) = toggle_popover(tray.app_handle(), rect) {
                            eprintln!("Menüleistenfenster: {error}");
                        }
                    }
                })
                .build(app)?;
            if std::env::args().any(|arg| arg == "--dashboard") {
                open_dashboard(app.handle().clone(), None)?;
            }
            growth::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
                if window.label() == "main" {
                    let _ = window
                        .app_handle()
                        .set_activation_policy(ActivationPolicy::Accessory);
                }
            }
            WindowEvent::Focused(false) if window.label() == "popover" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            open_dashboard,
            set_appearance,
            set_language,
            close_popover,
            macos_integration::account_picture,
            macos_integration::check_folder_access,
            macos_integration::open_privacy_settings,
            updates::release_info,
            updates::check_for_update,
            updates::install_update,
            updates::restart_after_update,
            growth::storage_growth,
            growth::reveal_storage_growth,
            monitor::telemetry,
            monitor::terminate_process,
            files::scan_files,
            files::trash_files,
            files::inspect_file,
            files::file_thumbnail,
            dev::pick_dev_folder,
            dev::scan_dev,
            dev::act_on_dev_entry,
            dev::scan_docker,
            dev::open_docker,
            inventory::scan_inventory,
            inventory::reveal_inventory_item,
            inventory::remove_inventory_tool,
            inventory::check_dev_impact,
            inventory::clean_dev_cache,
            inventory::reveal_dev_cache,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri konnte nicht gestartet werden")
        .run(|app, event| {
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                let _ = open_dashboard(app.clone(), None);
            }
        });
}
