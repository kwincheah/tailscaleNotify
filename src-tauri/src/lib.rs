use base64::Engine;
use notify::event::{AccessKind, AccessMode, ModifyKind};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

/// A file must be quiet (no new events, unchanged size/mtime) for this long before we notify.
/// Taildrop moves finished files into place whole, so a short window is enough.
const QUIET_PERIOD: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FilePayload {
    path: String,
    name: String,
    preview_data_url: Option<String>,
}

/// The file most recently announced to the UI. "Open File" may only open this path,
/// so the webview cannot be used to open arbitrary paths.
#[derive(Default)]
struct LatestFile(Mutex<Option<PathBuf>>);

#[cfg(target_os = "linux")]
#[derive(Default)]
struct ReceiverProcess(Mutex<Option<std::process::Child>>);

/// Only react to content changes. Metadata/access events fire whenever a file is opened,
/// tagged or indexed, and would re-trigger alerts for old files.
fn is_supported_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Name(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Access(AccessKind::Close(AccessMode::Write))
    )
}

fn is_ignored_file(path: &Path) -> bool {
    let Some(filename) = path.file_name().map(|f| f.to_string_lossy()) else {
        return true;
    };

    filename.starts_with('.')
        || filename.starts_with("~$")
        || filename.ends_with(".tmp")
        || filename.ends_with(".part")
        || filename.ends_with(".partial")
        || filename.ends_with(".crdownload")
        || filename.ends_with(".download")
        || filename.eq_ignore_ascii_case("desktop.ini")
}

fn ensure_directory_exists(path: &Path) -> bool {
    if path.exists() {
        return true;
    }

    if let Err(error) = std::fs::create_dir_all(path) {
        eprintln!("failed to create directory {:?}: {}", path, error);
        return false;
    }

    true
}

/// Size + mtime, used both to detect when a write has settled and to avoid
/// re-announcing a file whose content has not changed.
type Fingerprint = (u64, Option<SystemTime>);

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some((metadata.len(), metadata.modified().ok()))
}

/// macOS: Taildrop files have no quarantine tag (or one from Tailscale). Anything tagged by
/// another app (Chrome, Safari, WhatsApp, Mail, Slack, AirDrop, ...) is not from Taildrop.
#[cfg(target_os = "macos")]
fn is_other_app_download(path: &Path) -> bool {
    let output = std::process::Command::new("xattr")
        .arg("-p")
        .arg("com.apple.quarantine")
        .arg(path)
        .output();

    match output {
        Ok(result) if result.status.success() => {
            // Format: flags;timestamp;agent-name;uuid
            let tag = String::from_utf8_lossy(&result.stdout);
            let agent = tag.split(';').nth(2).unwrap_or("").trim().to_lowercase();
            !agent.contains("tailscale")
        }
        _ => false,
    }
}

/// Windows: browsers and most downloaders write a Mark-of-the-Web stream with a source URL.
#[cfg(target_os = "windows")]
fn is_other_app_download(path: &Path) -> bool {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    match std::fs::read_to_string(&stream) {
        Ok(contents) => contents.contains("HostUrl=") || contents.contains("ReferrerUrl="),
        Err(_) => false,
    }
}

/// Linux: the watched directory is filled only by `tailscale file get`, so everything is Taildrop.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn is_other_app_download(_path: &Path) -> bool {
    false
}

fn image_mime_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

fn build_preview_data_url(path: &Path) -> Option<String> {
    const MAX_PREVIEW_SIZE_BYTES: u64 = 12 * 1024 * 1024;

    let mime = image_mime_type(path)?;
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_PREVIEW_SIZE_BYTES {
        eprintln!("skipping preview for large file: {:?}", path);
        return None;
    }

    let bytes = std::fs::read(path).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{};base64,{}", mime, encoded))
}

/// Lets the alert appear over another app's full-screen Space. macOS only puts a background app's
/// window there if it is a non-activating NSPanel that can join all Spaces as a full-screen
/// auxiliary; Tauri's `visibleOnAllWorkspaces` alone leaves it on the desktop until the user
/// leaves full screen. Status level is set here rather than via `set_always_on_top`, which would
/// reset it to the lower floating level. Must run on the main thread.
#[cfg(target_os = "macos")]
fn float_over_full_screen_apps(window: &tauri::WebviewWindow) {
    use objc2::ClassType;
    use objc2_app_kit::{
        NSPanel, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    };

    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    // SAFETY: Tauri returns a valid NSWindow pointer that lives as long as `window`.
    let ns_window: &NSWindow = unsafe { &*ns_window.cast::<NSWindow>() };

    if !ns_window.styleMask().contains(NSWindowStyleMask::NonactivatingPanel) {
        // SAFETY: NSPanel is an NSWindow subclass with no extra instance state, so the existing
        // object is valid as one (the same conversion the tauri-nspanel plugin performs).
        unsafe {
            objc2::ffi::object_setClass(
                (ns_window as *const NSWindow).cast_mut().cast(),
                NSPanel::class(),
            );
        }
        ns_window.setStyleMask(ns_window.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    }
    ns_window.setCollectionBehavior(
        ns_window.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    ns_window.setLevel(NSStatusWindowLevel);
}

/// `take_focus` is false for automatic alerts so an incoming file never steals keyboard focus.
fn show_main_window(app_handle: &AppHandle, take_focus: bool) {
    let app_handle_for_closure = app_handle.clone();

    if let Err(error) = app_handle.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        if take_focus {
            let _ = app_handle_for_closure.show();
        }

        let Some(main_window) = app_handle_for_closure.get_webview_window("main") else {
            eprintln!("main window not found");
            return;
        };
        // Before show(), so the window opens on the Space the user is on, full-screen ones included.
        #[cfg(target_os = "macos")]
        float_over_full_screen_apps(&main_window);
        let _ = main_window.unminimize();
        let _ = main_window.show();
        if take_focus {
            let _ = main_window.set_focus();
        }
    }) {
        eprintln!("failed to schedule main-window show on main thread: {}", error);
    }
}

fn announce_file(app_handle: &AppHandle, path: &Path) {
    let Some(name) = path.file_name().map(|f| f.to_string_lossy().to_string()) else {
        return;
    };
    println!("[+] Taildrop file received: {}", name);

    *app_handle.state::<LatestFile>().0.lock().unwrap() = Some(path.to_path_buf());

    let payload = FilePayload {
        path: path.to_string_lossy().to_string(),
        name,
        preview_data_url: build_preview_data_url(path),
    };

    // Emit before showing so the window never flashes the previous file.
    if let Err(error) = app_handle.emit("new-file-received", payload) {
        eprintln!("failed to emit event to frontend: {}", error);
    }
    show_main_window(app_handle, false);
}

/// Directory where Taildrop delivers files on this platform.
/// macOS/Windows: the Tailscale GUI saves straight into Downloads.
/// Linux: tailscaled holds files until fetched; we fetch them into ~/TaildropInbox.
fn taildrop_directory() -> Option<PathBuf> {
    let home = dirs_next::home_dir()?;
    if cfg!(target_os = "linux") {
        Some(home.join("TaildropInbox"))
    } else {
        Some(dirs_next::download_dir().unwrap_or_else(|| home.join("Downloads")))
    }
}

/// Linux only: keep `tailscale file get` running so received files land in `inbox`.
/// Requires the user to be the tailscaled operator (`sudo tailscale set --operator=$USER`).
#[cfg(target_os = "linux")]
fn start_linux_receiver(app_handle: AppHandle, inbox: PathBuf) {
    std::thread::spawn(move || loop {
        let spawned = std::process::Command::new("tailscale")
            .args(["file", "get", "--wait", "--loop", "--conflict=rename"])
            .arg(&inbox)
            .spawn();

        match spawned {
            Ok(child) => {
                let state = app_handle.state::<ReceiverProcess>();
                *state.0.lock().unwrap() = Some(child);
                // Poll instead of wait() so the exit handler can take and kill the child.
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let mut guard = state.0.lock().unwrap();
                    match guard.as_mut().map(|c| c.try_wait()) {
                        Some(Ok(None)) => continue,
                        Some(Ok(Some(status))) => {
                            eprintln!("`tailscale file get` exited ({}); restarting", status);
                            *guard = None;
                            break;
                        }
                        Some(Err(error)) => {
                            eprintln!("failed to poll `tailscale file get`: {}", error);
                            *guard = None;
                            break;
                        }
                        None => return, // taken by the exit handler
                    }
                }
            }
            Err(error) => eprintln!("failed to start `tailscale file get`: {}", error),
        }
        std::thread::sleep(Duration::from_secs(15));
    });
}

fn start_taildrop_watcher(app_handle: AppHandle, watch_path: PathBuf) {
    std::thread::spawn(move || {
        if !ensure_directory_exists(&watch_path) {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("failed to create file watcher: {}", error);
                return;
            }
        };
        if let Err(error) = watcher.watch(&watch_path, RecursiveMode::NonRecursive) {
            eprintln!("failed to watch {:?}: {}", watch_path, error);
            return;
        }

        println!("[*] Watching for Taildrop files in {:?}", watch_path);

        // Files with recent activity, waiting for writes to settle: path -> (last event, fingerprint).
        let mut pending: HashMap<PathBuf, (Instant, Option<Fingerprint>)> = HashMap::new();
        // Files already announced, so later events with unchanged content are ignored.
        let mut announced: HashMap<PathBuf, Fingerprint> = HashMap::new();

        loop {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(Ok(event)) if is_supported_event(&event.kind) => {
                    for path in event.paths {
                        if !is_ignored_file(&path) {
                            let print = fingerprint(&path);
                            pending.insert(path, (Instant::now(), print));
                        }
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => eprintln!("watch error: {:?}", error),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let now = Instant::now();
            let mut ready = Vec::new();
            pending.retain(|path, (last_event, last_print)| {
                let Some(current) = fingerprint(path) else {
                    return false; // removed, renamed away, or a directory
                };
                if Some(current) != *last_print {
                    // Still being written: restart the quiet period.
                    *last_event = now;
                    *last_print = Some(current);
                    return true;
                }
                if now.duration_since(*last_event) < QUIET_PERIOD {
                    return true;
                }
                // On Windows this fails while the writer still holds the file open.
                if File::open(path).is_err() {
                    return true;
                }
                ready.push((path.clone(), current));
                false
            });

            for (path, print) in ready {
                if announced.get(&path) == Some(&print) || is_other_app_download(&path) {
                    continue;
                }
                announced.insert(path.clone(), print);
                announce_file(&app_handle, &path);
            }
        }
    });
}

#[tauri::command]
fn open_received_file(app_handle: AppHandle) -> Result<(), String> {
    let Some(path) = app_handle.state::<LatestFile>().0.lock().unwrap().clone() else {
        return Err("no file has been received yet".into());
    };
    app_handle
        .opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| error.to_string())
}

/// Flips launch-at-login, then syncs the checkmark to the real state in case the change failed.
fn toggle_autostart(app_handle: &AppHandle, item: &CheckMenuItem<tauri::Wry>) {
    let autolaunch = app_handle.autolaunch();
    let result = if autolaunch.is_enabled().unwrap_or(false) {
        autolaunch.disable()
    } else {
        autolaunch.enable()
    };
    if let Err(error) = result {
        eprintln!("failed to change launch-at-login: {}", error);
    }
    let _ = item.set_checked(autolaunch.is_enabled().unwrap_or(false));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .manage(LatestFile::default());
    #[cfg(target_os = "linux")]
    let builder = builder.manage(ReceiverProcess::default());

    builder
        .invoke_handler(tauri::generate_handler![open_received_file])
        // Hide instead of destroying the window when its close button is used.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let show_item = MenuItem::with_id(app, "show", "Show Latest Alert", true, None::<&str>)?;
            let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
            let autostart_item = CheckMenuItem::with_id(
                app,
                "autostart",
                "Launch at Login",
                true,
                autostart_enabled,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &autostart_item, &quit_item])?;

            // Without an icon the tray item is invisible, leaving no way to reach Quit.
            let mut tray = TrayIconBuilder::new()
                .tooltip("Taildrop Alert")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_main_window(app, true),
                    "autostart" => toggle_autostart(app, &autostart_item),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let Some(watch_path) = taildrop_directory() else {
                eprintln!("could not resolve the Taildrop directory");
                return Ok(());
            };

            #[cfg(target_os = "linux")]
            if ensure_directory_exists(&watch_path) {
                start_linux_receiver(app.handle().clone(), watch_path.clone());
            }

            start_taildrop_watcher(app.handle().clone(), watch_path);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, _event| match _event {
            // Clicking the Dock icon reopens the hidden window (macOS only).
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_main_window(_app_handle, true),
            #[cfg(target_os = "linux")]
            tauri::RunEvent::Exit => {
                if let Some(mut child) = _app_handle.state::<ReceiverProcess>().0.lock().unwrap().take() {
                    let _ = child.kill();
                }
            }
            _ => {}
        });
}
