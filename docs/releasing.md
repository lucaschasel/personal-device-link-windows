# 构建与分发应用包

公开仓库的普通用户应下载应用包，不应依赖源码启动脚本。此仓库的工作流只构建并上传 Actions artifacts，不会自动创建或发布 GitHub Release。

## GitHub 生成 Mac DMG

将代码上传到你管理的 GitHub 仓库后，打开 Actions → **Build macOS packages** → Run workflow。

- `test`：无需证书，生成临时签名的测试 DMG，macOS 可能拦截，不可宣称已经公证。
- `signed`：必须配置下面的 Secrets；任何签名、公证、票据校验失败都会停止，不会退回测试包。

两个任务分别在 `macos-14`（Apple Silicon）和 `macos-15-intel`（Intel）上原生构建。界面、Rust 蓝牙桥接与原生蓝牙辅助程序均装入同一个 `.app`，架构与 runner 一致。完成后下载对应 artifact 并解压得到 DMG 和 `.sha256` 校验文件。Artifacts 保留 14 天；长期分发需维护者将通过验证的产物附到 Release。

正式模式需要以下仓库 Actions Secrets，勿提交证书或密钥到源码：

| 名称 | 内容 |
| --- | --- |
| `PDL_CERTIFICATE_BASE64` | Developer ID Application 证书及私钥导出的 `.p12` 文件的 Base64 内容 |
| `PDL_CERTIFICATE_PASSWORD` | 导出 `.p12` 时设置的密码（必须非空） |
| `PDL_SIGNING_IDENTITY` | 完整的 `Developer ID Application: 名称 (TEAMID)` 身份 |
| `PDL_APPLE_ID` | 用于公证的 Apple ID |
| `PDL_APPLE_PASSWORD` | 该 Apple ID 的 App 专用密码 |
| `PDL_APPLE_TEAM_ID` | Apple Developer Team ID |

脚本先完成所有应用内容的装配，再逐个为可执行程序及整个 `.app` 签名，启用 hardened runtime 与安全时间戳。它提交应用公证并装订票据、验证 Gatekeeper 接受，随后制作 DMG、签名并公证 DMG。签名密钥临时导入独立钥匙串，脚本退出时清理。签名后不再向 `.app` 添加文件。

本地 Mac 也可在配置相同环境变量后执行 `bash scripts/package-macos.sh signed`；不要把包含密码的命令或终端日志发到 issue。只做开发测试可执行 `bash scripts/package-macos.sh test`。

Developer ID 证书需要相应的 Apple 开发者资格。临时签名不解决公开下载时的 Gatekeeper 提示，详见 [Apple 公证要求](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) 和 [Tauri macOS 签名说明](https://v2.tauri.app/distribute/sign/macos/)。首次运行、蓝牙、本地网络和辅助功能的正常权限提示仍可能出现。

## Windows 与源码包

运行 `powershell -File scripts/Build-Desktop.ps1` 生成 Windows 便携包。运行 `powershell -File scripts/Package-Source.ps1` 生成精简源码包，包含工作流、构建脚本和文档，不包含开发工具链、编译缓存或签名凭据。Windows 代码签名未接入此工作流。

## 发布前验证

核对两端版本与协议，完成 CI 和两个 Mac 架构的实际构建。对下载后的应用包做打开、蓝牙组件加载、本地网络、辅助功能、双设备控制及文件传输验收。同系统与跨系统组合都应覆盖；蓝牙限制仍须在 Release 中说明。只有真实执行后的结果才能写作“已通过”。

推送代码会触发 CI 测试与测试包构建；正式签名、公证需要维护者配置 Apple 证书与凭据。通过真机验证后，可将对应产物附到 GitHub Release 长期分发。
