# Windows 构建与分发

本仓库仅构建 Windows 应用。[Mac 仓库](https://github.com/lucaschasel/personal-device-link-mac) 单独维护 DMG 与签名流程。

执行 `powershell -File scripts/Build-Desktop.ps1`，生成 `dist/personal-device-link-v0151-windows.zip`。打包目录包含桌面程序、蓝牙组件、所需 DLL、使用说明及 MIT 许可证。

推送代码后 Windows CI 执行格式检查、Clippy、测试和 Release 构建。成功后可从 Actions 下载 Windows artifact，保留 14 天；工作流不会自动发布 GitHub Release。经真机验证后可把 ZIP 放入 Release 长期分发。

源码包可运行 `powershell -File scripts/Package-Source.ps1` 生成，仅包含当前 Windows 源码及文档，不包含缓存和本地 Mac 仓库。Windows 便携包目前未签名。
