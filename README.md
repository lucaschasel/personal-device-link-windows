# Personal Device Link — Windows

[![Windows CI](https://github.com/lucaschasel/personal-device-link-windows/actions/workflows/ci.yml/badge.svg)](https://github.com/lucaschasel/personal-device-link-windows/actions/workflows/ci.yml) · [MIT](LICENSE)

这是 **Windows 客户端源码仓库**，用于按需共享键盘、鼠标和剪贴板，以及双向传输文件。使用 Rust 与 Tauri，无需账号或云端中转。

**Mac 用户请前往 [personal-device-link-mac](https://github.com/lucaschasel/personal-device-link-mac)。** 本仓库不包含 macOS 原生实现或 Mac 打包脚本。

当前版本 **0.15.1**，支持 Windows 10/11。Windows CI 在 x64 环境构建；其他架构尚未提供验证包。

## 下载与运行

在 [Actions](https://github.com/lucaschasel/personal-device-link-windows/actions/workflows/ci.yml) 中打开成功的运行，下载 Windows artifact，解压其中的 `personal-device-link-v0151-windows.zip`，再完整解压应用目录。

双击 `Personal Device Link.exe`，保留随附的 `bluetooth_bridge.exe` 和 DLL。需要 Microsoft Edge WebView2 Runtime。当前便携包未进行代码签名，Actions 产物保留 14 天。

紧急停止：**Ctrl + Alt + Esc**。首次连接时允许应用通过防火墙访问可信的本地网络。

## 在 Windows 构建

安装 Rust stable、Visual Studio C++ 构建工具及 Windows SDK、Node.js（CI 使用 22）。在本仓库根目录执行：

```powershell
powershell -File scripts/Build-Desktop.ps1
```

输出位于 `dist/personal-device-link-v0151-windows.zip`。构建时需获取依赖；已构建应用的设备通信不需要互联网。

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cd desktop
npm ci
npm test
npm run build
```

## 使用

1. 两台设备打开各自系统的应用，接入同一局域网，在附近设备中选择对方；也可以手动输入 IP。局域网不需要互联网。
2. 完全没有局域网时，可尝试实验性蓝牙桥接：先在系统设置配对，一端选择等待连接，另一端填写地址与频道主动连接。链路就绪后点击“连接蓝牙设备”。
3. 连接后按需选择键盘、鼠标或共享剪贴板。对方弹窗显示发起设备、连接方式和请求内容，同意后才生效。连接不会自动接管输入。
4. 在文件传输页选择文件，对方确认保存位置后开始；支持进度、取消和 SHA-256 校验，不覆盖已有文件。

控制中可调整功能；增加权限需要对方再次确认。剪贴板支持新复制的文字、图片和普通文件，旧内容需主动同步，可随时关闭。

## 互通与边界

两端保持应用协议 **7**，支持 Windows ↔ Windows、Mac ↔ Mac、Windows ↔ Mac。任一端均可发起控制和传输，切换方向前先停止当前控制。蓝牙两端均可选择等待或主动连接，新增组合仍需实机验收。

- 只连接可信设备：应用层 TCP 尚未加密，设备名称不是可信身份认证。
- 剪贴板授权后新复制的敏感内容也会同步；普通文件单次最多 16 个、总量 256 MiB，文件夹先压缩，不同步剪切删除动作。
- 不包含屏幕画面串流；主要用于能看到对方屏幕的设备协作。
- 蓝牙实际可用性受硬件、驱动和系统配对支持影响；CI 编译与模拟测试不能替代双机验证。

## 源码结构

`src/` 包含通信、文件、剪贴板与本平台输入实现；`desktop/` 是 Tauri 桌面端；`tests/` 是模拟与协议测试；`scripts/` 提供本平台构建工具。

两个仓库均可独立构建，无需克隆另一个仓库。为保持独立性，两边各自包含相同的通信协议和通用界面代码；修改协议时应同步更新两边，并验证跨系统兼容性。

更多说明：[桌面使用](docs/desktop.md) · [授权机制](docs/consent.md) · [蓝牙](docs/bluetooth-bridge.md) · [测试](docs/testing.md) · [发布](docs/releasing.md)。

## 许可证

[MIT License](LICENSE)。第三方依赖遵循各自许可证。
