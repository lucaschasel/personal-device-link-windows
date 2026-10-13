# 测试与验证

## 本地自动化

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --manifest-path desktop/src-tauri/Cargo.toml --bin personal-device-link-desktop
```

桌面目录执行 `npm ci`、`npm test`、`npm run build`。开发预览使用 `npm run dev` 后打开 `http://127.0.0.1:1420/?preview=empty`（或 connected、files、request）；追加 `&os=macos` 可查看 Mac 文案。预览仅使用模拟数据，生产构建不包含此模块。

测试覆盖通信帧、授权请求、输入释放、同系统控制流程、文件确认与校验、剪贴板边界、蓝牙双角色多路复用和地址校验。输入和剪贴板测试使用模拟后端，不替代真实系统权限和设备操作。

## CI 与安装包

GitHub Actions 对 Windows、macOS 执行 Rust 检查和测试，以及桌面构建。成功的桌面任务上传 Windows 便携 ZIP 或 macOS 未公证测试 DMG。单独的 Mac 打包工作流提供 Intel 与 Apple Silicon 两种架构，以及正式签名、公证模式。

CI 的 macOS 构建会编译 Objective-C / IOBluetooth 辅助程序。跨目标 Rust 类型检查不等于原生 Mac 构建成功。Mac runner 没有双机射频测试条件，构建成功也不表示蓝牙兼容性已通过实机验收。

## 实机验收

- Windows↔Windows、macOS↔macOS、Windows↔macOS 分别通过局域网连接；交换控制与文件发送方向。
- 蓝牙先在系统设置配对，一端等待、另一端主动，再互换角色；断开 Wi-Fi / 网线，排除误走局域网。
- 普通连接不接管输入、不读取剪贴板；控制、增加权限和剪贴板共享均需接收方确认，拒绝和超时不获得权限。
- 控制中停止、断线、关闭窗口，确认本地输入恢复及对方按键释放。
- 文件请求拒绝、取消、已有同名文件、校验失败、重试；确认不覆盖文件。包括零字节、中文文件名和大文件。
- 剪贴板纯文本、图片和普通文件双向共享；确认关闭立即停止、旧内容不自动发送。

新增蓝牙组合目前仍需实机验收，不能把模拟通过描述为无线连接已验收。
