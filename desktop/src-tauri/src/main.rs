#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use personal_device_link::{
    desktop::{CaptureDriver, Desktop, Snapshot},
    input::options::{CaptureConfig, ControlOptions},
    input::queue::InputQueue,
    network::protocol::Device,
};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
mod bluetooth;

struct NativeCapture {
    app: tauri::AppHandle,
    #[cfg(windows)]
    windows:
        std::sync::Mutex<Option<personal_device_link::platform::windows::input_capture::Capture>>,
}

impl CaptureDriver for NativeCapture {
    fn permission(&self) -> io::Result<()> {
        #[cfg(windows)]
        {
            let _ = &self.app;
            Ok(())
        }
        #[cfg(not(windows))]
        {
            Err(io::Error::other("当前系统尚未实现输入捕获"))
        }
    }
    fn start(
        &self,
        queue: Arc<InputQueue>,
        running: Arc<AtomicBool>,
        config: CaptureConfig,
    ) -> io::Result<()> {
        #[cfg(windows)]
        {
            let mut slot = self.windows.lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_some() {
                return Err(io::Error::other("已有捕获会话"));
            }
            *slot = Some(
                personal_device_link::platform::windows::input_capture::Capture::start(
                    queue, running, config,
                )?,
            );
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (queue, running, config);
            Err(io::Error::other("当前系统尚未实现输入捕获"))
        }
    }
    fn stop(&self) -> io::Result<()> {
        #[cfg(windows)]
        {
            self.windows
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .map_or(Ok(()), |capture| capture.finish())
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }
}

struct AppState {
    bluetooth: Arc<bluetooth::Bluetooth>,
    desktop: Arc<Desktop>,
    exiting: AtomicBool,
    done: AtomicBool,
}
#[tauri::command]
fn show_request_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
#[tauri::command]
fn decide_consent(
    state: tauri::State<'_, AppState>,
    id: String,
    allow: bool,
) -> Result<(), String> {
    state
        .desktop
        .consent
        .decide(&id, allow)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn snapshot(state: tauri::State<'_, AppState>) -> AppSnapshot {
    AppSnapshot {
        bluetooth: state.bluetooth.snapshot(),
        desktop: state.desktop.snapshot(),
    }
}
#[derive(serde::Serialize)]
struct AppSnapshot {
    bluetooth: bluetooth::View,
    #[serde(flatten)]
    desktop: Snapshot,
}
#[tauri::command]
async fn start_bluetooth(
    state: tauri::State<'_, AppState>,
    role: personal_device_link::bluetooth::Role,
    address: Option<String>,
    channel: Option<u16>,
) -> Result<(), String> {
    let bluetooth = state.bluetooth.clone();
    tauri::async_runtime::spawn_blocking(move || {
        bluetooth.start(role, address.unwrap_or_default(), channel.unwrap_or(0))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn stop_bluetooth(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if state.desktop.snapshot().connection.address == "127.0.0.1:45833" {
        state.desktop.disconnect();
    }
    let bluetooth = state.bluetooth.clone();
    tauri::async_runtime::spawn_blocking(move || bluetooth.stop())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn connect_bluetooth(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if state.bluetooth.snapshot().phase != "ready" {
        return Err("请先建立蓝牙链路".into());
    }
    state
        .desktop
        .connect(None, Some("127.0.0.1:45833".into()))
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn sync_current_clipboard(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .desktop
        .clipboard
        .send_current()
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn disable_clipboard(state: tauri::State<'_, AppState>) {
    state.desktop.clipboard.disable();
}
#[tauri::command]
fn connect_clipboard(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .desktop
        .connect_clipboard(String::new())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn connect_device(
    state: tauri::State<'_, AppState>,
    id: Option<String>,
    address: Option<String>,
) -> Result<(), String> {
    state
        .desktop
        .connect(id, address)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn disconnect_device(state: tauri::State<'_, AppState>) {
    state.desktop.disconnect();
}
#[tauri::command]
fn start_control(state: tauri::State<'_, AppState>, options: ControlOptions) -> Result<(), String> {
    state
        .desktop
        .start_control_with_options(String::new(), options)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn update_control(
    state: tauri::State<'_, AppState>,
    options: ControlOptions,
) -> Result<(), String> {
    state
        .desktop
        .update_control(options)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn stop_control(state: tauri::State<'_, AppState>) {
    state.desktop.stop_control();
}
#[tauri::command]
fn retry_network(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .desktop
        .start_network("0.0.0.0:45832".parse().unwrap())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn start_receiver(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .desktop
        .start_receiver("0.0.0.0:45832".parse().unwrap())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn stop_receiver(state: tauri::State<'_, AppState>) {
    state.desktop.stop_receiver();
}

#[tauri::command]
async fn choose_and_send(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let desktop = Arc::clone(&state.desktop);
    let initial = desktop.snapshot().connection;
    if !matches!(initial.phase.as_str(), "connected" | "controlling") {
        return Err("请先连接目标设备".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title("选择要发送的单个文件")
            .blocking_pick_file()
        else {
            return Ok(());
        };
        let current = desktop.snapshot().connection;
        if current.address != initial.address
            || current.name != initial.name
            || current.os != initial.os
        {
            return Err("目标连接已改变，请重新选择文件".into());
        }
        desktop
            .send_file(path.into_path().map_err(|e| e.to_string())?)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn accept_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let transfers = Arc::clone(&state.desktop.transfers);
    let name = transfers.pending_name(&id).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title("接收文件：请选择新文件名，不覆盖已有文件")
            .set_file_name(name)
            .blocking_save_file()
        else {
            return Ok(());
        };
        transfers
            .decide(&id, Some(path.into_path().map_err(|e| e.to_string())?))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn reject_file(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .desktop
        .transfers
        .decide(&id, None)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn cancel_file(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .desktop
        .transfers
        .cancel(&id)
        .map_err(|e| e.to_string())
}

fn shutdown(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if state.exiting.swap(true, Ordering::AcqRel) {
        return;
    }
    state.desktop.request_shutdown();
    let bluetooth = Arc::clone(&state.bluetooth);
    let desktop = Arc::clone(&state.desktop);
    let app = app.clone();
    std::thread::spawn(move || {
        bluetooth.stop();
        desktop.join();
        app.state::<AppState>().done.store(true, Ordering::Release);
        app.exit(0);
    });
}
fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let name = std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "Windows-PC".into());
            let desktop = Arc::new(Desktop::new(
                Device::local(name)?,
                Arc::new(NativeCapture {
                    app: app.handle().clone(),
                    #[cfg(windows)]
                    windows: std::sync::Mutex::new(None),
                }),
            )?);
            desktop.start_discovery()?;
            // A bind error is visible in Snapshot and can be retried in the window.
            let _ = desktop.start_network("0.0.0.0:45832".parse().unwrap());
            app.manage(AppState {
                bluetooth: bluetooth::Bluetooth::new(),
                desktop,
                exiting: AtomicBool::new(false),
                done: AtomicBool::new(false),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            decide_consent,
            show_request_window,
            start_bluetooth,
            stop_bluetooth,
            connect_bluetooth,
            snapshot,
            sync_current_clipboard,
            disable_clipboard,
            connect_clipboard,
            connect_device,
            disconnect_device,
            start_control,
            update_control,
            stop_control,
            start_receiver,
            retry_network,
            stop_receiver,
            choose_and_send,
            accept_file,
            reject_file,
            cancel_file
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                shutdown(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("无法启动桌面界面");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !app.state::<AppState>().done.load(Ordering::Acquire) {
                api.prevent_exit();
                shutdown(app);
            }
        }
    });
}
