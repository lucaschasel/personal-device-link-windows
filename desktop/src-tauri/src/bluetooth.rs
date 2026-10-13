use personal_device_link::{bluetooth::Role, clipboard::lock};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[derive(Clone, Serialize)]
pub struct View {
    pub phase: String,
    pub role: Role,
    pub address: String,
    pub channel: u16,
    pub detail: String,
}
struct Inner {
    generation: u64,
    child: Option<Child>,
    view: View,
}
pub struct Bluetooth {
    inner: Mutex<Inner>,
}
#[derive(Deserialize)]
struct Event {
    event: String,
    address: Option<String>,
    channel: Option<u16>,
}
fn apply(view: &mut View, event: Event) {
    match event.event.as_str() {
        "listening" => {
            view.phase = "waiting".into();
            view.address = event.address.unwrap_or_default();
            view.channel = event.channel.unwrap_or_default();
            view.detail = "等待已配对的对方设备连接".into();
        }
        "negotiating" => {
            view.phase = "starting".into();
            view.detail = "无线链路已建立，正在确认对方应用".into();
        }
        "ready" => {
            view.phase = "ready".into();
            view.detail = "蓝牙链路已连接，可点击连接设备；不会自动接管键鼠".into();
        }
        _ => (),
    }
}
impl Bluetooth {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                generation: 0,
                child: None,
                view: View {
                    phase: "off".into(),
                    role: Role::Listen,
                    address: String::new(),
                    channel: 0,
                    detail: String::new(),
                },
            }),
        })
    }
    pub fn snapshot(&self) -> View {
        lock(&self.inner).view.clone()
    }
    pub fn stop(&self) {
        let mut state = lock(&self.inner);
        state.generation += 1;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.view = View {
            phase: "off".into(),
            role: Role::Listen,
            address: String::new(),
            channel: 0,
            detail: String::new(),
        };
    }
    pub fn start(
        self: &Arc<Self>,
        role: Role,
        address: String,
        channel: u16,
    ) -> Result<(), String> {
        let arguments = role.arguments(&address, channel)?;
        let mut state = lock(&self.inner);
        if state.child.is_some() {
            return Err("蓝牙已启动，请先停止后重试".into());
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let helper = executable.with_file_name("bluetooth_bridge.exe");
        if !helper.is_file() {
            return Err(
                "缺少蓝牙组件，请重新安装完整应用包；源码构建请按构建说明包含蓝牙组件".into(),
            );
        }
        let mut command = Command::new(helper);
        command.args(arguments);
        command
            .arg("--managed")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("蓝牙组件启动失败：{e}"))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        state.generation += 1;
        let generation = state.generation;
        state.view = View {
            phase: "starting".into(),
            role,
            address: String::new(),
            channel: 0,
            detail: "正在启动蓝牙，请确保两机已在系统中配对".into(),
        };
        state.child = Some(child);
        drop(state);
        let owner = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(event) = serde_json::from_str::<Event>(&line) {
                    let mut state = lock(&owner.inner);
                    if state.generation != generation || state.child.is_none() {
                        return;
                    }
                    apply(&mut state.view, event);
                }
            }
        });
        let owner = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut state = lock(&owner.inner);
                if state.generation != generation {
                    return;
                }
                if !line.contains("蓝牙桥接已连接") {
                    state.view.detail = line.chars().take(1200).collect();
                }
            }
        });
        let owner = self.clone();
        let mut phase_since = Instant::now();
        let mut last_phase = "starting".to_string();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(150));
            let mut state = lock(&owner.inner);
            if state.generation != generation {
                return;
            }
            if last_phase != state.view.phase {
                last_phase = state.view.phase.clone();
                phase_since = Instant::now();
            }
            if state.view.phase == "starting" && phase_since.elapsed() > Duration::from_secs(30) {
                if let Some(mut child) = state.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                // Invalidate pipe readers so late output cannot overwrite the timeout.
                state.generation += 1;
                state.view.phase = "error".into();
                state.view.detail =
                    "蓝牙连接超时。请确认已配对、对方选择了等待连接，并核对地址与频道。".into();
                return;
            }
            let status = state
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok())
                .flatten();
            if let Some(status) = status {
                state.child.take();
                state.view.phase = "error".into();
                state.view.detail = format!("蓝牙已断开（{status}）。{}", state.view.detail);
                return;
            }
        });
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_input_is_not_a_shell_command() {
        assert_eq!(
            personal_device_link::bluetooth::address("aa:bb:cc:dd:ee:ff", 4).unwrap(),
            "AABBCCDDEEFF"
        );
        assert!(personal_device_link::bluetooth::address("$(whoami)", 4).is_err());
        assert!(personal_device_link::bluetooth::address("AABBCCDDEEFF", 31).is_err());
    }
    #[test]
    fn waiting_and_radio_ready_are_distinct() {
        let b = Bluetooth::new();
        let mut v = b.snapshot();
        apply(
            &mut v,
            serde_json::from_str(r#"{"event":"listening","address":"AABBCCDDEEFF","channel":7}"#)
                .unwrap(),
        );
        assert_eq!(v.phase, "waiting");
        assert_eq!(v.channel, 7);
        apply(
            &mut v,
            serde_json::from_str(r#"{"event":"ready"}"#).unwrap(),
        );
        assert_eq!(v.phase, "ready");
    }
}
