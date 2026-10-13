# Personal Device Link

[![CI](https://github.com/lucaschasel/personal-device-link/actions/workflows/ci.yml/badge.svg)](https://github.com/lucaschasel/personal-device-link/actions/workflows/ci.yml) · [MIT License](LICENSE) · [构建记录与下载](https://github.com/lucaschasel/personal-device-link/actions)

让两台电脑直接协作：按需共享键盘、鼠标和剪贴板，双向传输文件。使用 Rust 核心与 Tauri 桌面界面，无需账号或云端中转。

当前版本 **0.15.1**，应用协议 **7**。建议两端使用同一版本。连接仅建立通信；控制和剪贴板共享需要对方逐次同意，接收文件需要确认保存位置。

## 支持哪些设备

| 连接方式 | 设备组合 | 说明 |
| --- | --- | --- |
| 局域网 / IP 直连 | Windows ↔ Windows、macOS ↔ macOS、Windows ↔ macOS | 同系统与跨系统均支持；任一端可发起控制或传文件 |
| 蓝牙桥接（实验性） | Windows ↔ Windows、macOS ↔ macOS、Windows ↔ macOS | 两端均可等待或主动连接；先系统配对，再选不同角色。新增组合待实机验收 |

支持 Windows 10/11 与 macOS 11 及以上版本。Linux、手机和平板尚未实现完整桌面支持。控制方向由使用者选择，不绑定操作系统；切换方向前先停止当前控制。

局域网可以是 Wi-Fi、有线网络或可用热点，不要求能访问互联网。完全没有局域网时，可使用上述支持组合的蓝牙桥接。不要假定 Windows 在无上游网络时一定能开启系统移动热点。

## 安装与打开

下载对应操作系统的**应用包**，源码包供开发者构建。Mac 日常使用不需要运行 `.command` 或安装编译工具。

| 系统 | 应用包 | 打开方式 |
| --- | --- | --- |
| Windows | `personal-device-link-v0151-windows.zip` | 完整解压后运行 `Personal Device Link.exe`，保留蓝牙组件和随附 DLL；需要 WebView2 Runtime |
| Mac Apple Silicon | `personal-device-link-0.15.1-macos-arm64-signed.dmg` | 打开 DMG，将应用拖到“应用程序”后打开 |
| Mac Intel | `personal-device-link-0.15.1-macos-x86_64-signed.dmg` | 同上 |

**Mac 表中是工作流生成的正式包文件名，不表示已经提供下载。** 每次推送后的 CI 会构建 Windows ZIP 和未公证的 Mac 测试 DMG，可在成功的 Actions 运行底部下载 Artifacts。正式 Mac 包需维护者运行 `Build macOS packages` 的 signed 模式并完成签名、公证，再将产物附到 GitHub Release。工作流产物可从 Actions 下载；它不会自动发布 Release。带 `test` 的包是未公证测试包，系统仍可能拦截。具体安装、安全提示及当前源码构建方法见 [Mac 安装说明](docs/macos-quickstart.md)。

本地 Windows 便携包尚未进行代码签名。macOS 的签名、公证流程见 [发布指南](docs/releasing.md)。

## 连接与使用

1. 两台设备接入同一局域网并打开应用，在“设备连接”中选择对方。无法发现时可输入对方的 IP 地址。允许应用访问本地网络，无需关闭防火墙。
2. 连接后按需勾选键盘、鼠标，点击“开始所选控制”。对方弹窗显示发起设备、连接方式和具体权限，同意后才开始。连接本身不会接管输入。
3. 开启剪贴板共享需对方确认；随后双向同步新复制的文字、图片和普通文件。旧内容需点击“同步当前内容”才发送。
4. 在“文件传输”中选文件发送，对方选择保存位置。可查看进度、取消传输，完成时校验且不覆盖已有文件。

控制中可修改开关并点击“应用选择”。新增权限需再次同意；减少权限或停止无需等待对方。接收方也可停止控制。

紧急停止按本机系统选择：Windows **Ctrl + Alt + Esc**；macOS **Control + Option + Esc**。macOS 使用键鼠控制时可能需要辅助功能授权；这与应用安装时的 Gatekeeper 检查是两回事。

更多内容：[桌面使用](docs/desktop.md) · [授权机制](docs/consent.md) · [文件传输](docs/files.md) · [蓝牙说明](docs/bluetooth-bridge.md)。测试范围与实机验收见 [测试说明](docs/testing.md)。

## 当前边界

- 仅连接可信设备。应用层控制与文件通道尚未加密，设备名称与系统由对方提供，不作为可信身份认证。
- 剪贴板授权后，新复制的敏感内容也会同步；可随时关闭。普通文件单次最多 16 个、总量 256 MiB，文件夹请先压缩。富文本按纯文本处理，不同步剪切删除动作。
- 不含屏幕画面串流，主要用于看得到对方屏幕的设备协作。
- 蓝牙支持范围以表格为准，不宣称任意两台电脑都能通过蓝牙连接。无线硬件、系统权限及真实双机交互需要实机验收。

## 开发与验证

安装 Rust stable、Node.js LTS（CI 使用 22）。Windows 需要 C++ 构建工具；macOS 需要 Xcode Command Line Tools。

```powershell
# Windows：构建桌面与蓝牙组件并打包
powershell -File scripts/Build-Desktop.ps1
```

```bash
# macOS：本机源码构建，仅开发者需要
bash scripts/build-macos-gui.sh
open "desktop/src-tauri/target/release/bundle/macos/Personal Device Link.app"
```

构建需要联网获取工具和依赖，已构建应用的设备通信不需要互联网。不要把源码目录里的 `.command` 当成面向普通用户的安装器。

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cd desktop
npm ci
npm test
npm run build
```

核心命令行工具可通过 `cargo run -- --help` 查看参数；其控制码流程与桌面弹窗授权不同。不要让 CLI 与 GUI 同时监听同一个端口。

项目结构：`src/` 为通信与系统输入核心，`desktop/src/` 为界面，`desktop/src-tauri/` 为桌面桥接，`tests/` 为自动化测试，`scripts/` 为构建与诊断脚本。GitHub 持续集成覆盖 Windows 和 macOS，实际执行状态以仓库 Actions 为准。

## 许可证

本项目使用 [MIT License](LICENSE)。第三方依赖遵循各自的许可证。
