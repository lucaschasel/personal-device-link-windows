$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$releaseDirectory = Join-Path $projectRoot 'desktop/src-tauri/target/release'
$outputDirectory = Join-Path $projectRoot 'dist/personal-device-link-v0151-windows'
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $releaseDirectory 'personal-device-link-desktop.exe') -Destination (Join-Path $outputDirectory 'Personal Device Link.exe')
Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/bluetooth_bridge.exe') -Destination (Join-Path $outputDirectory 'bluetooth_bridge.exe')
$loader = Join-Path $releaseDirectory 'WebView2Loader.dll'
if (Test-Path -LiteralPath $loader) {
    Copy-Item -LiteralPath $loader -Destination (Join-Path $outputDirectory 'WebView2Loader.dll')
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination $outputDirectory
@'
Personal Device Link — 0.15.1 / 桌面交互收尾与开源发布

建议两端一起更新到 0.15.1；控制协议仍为 7，可与 0.13.0 互通。更早版本不兼容。
设备连接页切换局域网/蓝牙，连接后选择键鼠与剪贴板；文件传输页查看任务，可筛选进行中/已结束。控制或等待授权时，任意页面上方都可停止/撤回。
解压整个文件夹，双击 Personal Device Link.exe。保留 bluetooth_bridge.exe 和 WebView2Loader.dll。
先退出旧程序。打开应用自动发现并接受普通连接，连接成功不会接管键鼠或读取剪贴板。

控制：主动方连接目标，勾选键盘和/或鼠标，点击开始。接收端弹窗显示发起设备、系统、连接方式、地址和所需权限。同意本次请求后即可控制，无需控制码。
控制中仍可修改开关并点击“应用选择”。新增输入功能或改变键盘快捷键适配需再次确认；减少功能或暂停无须确认。等待期间释放远端按键并暂停捕获。拒绝或超时会结束控制并断开该控制连接，重新连接后可再次请求。
控制请求 60 秒过期。等待期间点“取消开始”、停止控制或断开会撤回请求；取消正在等待授权的控制也会断开该连接。接收端“停止控制并关闭请求”会撤销当前控制及待处理请求；可再次开启。
紧急停止：Mac Control + Option + Esc；Windows Ctrl + Alt + Esc。切换方向前先停止当前控制。

剪贴板：连接后只需主动方勾选“共享剪贴板”；接收端弹窗同意后，双向同步新复制的文字、图片和文件，不输入配对码。关闭、断开或失败后不会自动恢复，重新共享需再次确认。
旧内容不自动发送，可点击“同步当前内容”。后续复制的敏感内容也会同步。单次 1–16 个普通文件，总量 256 MiB；文件夹先压缩。文字最大 1 MiB，图片像素最多 64 MiB。不传播剪切删除动作。
文件缓存目录显示在界面中；文件传完后到另一端文件夹粘贴。每次运行最多缓存 1 GiB / 64 批；粘贴完成前不要清理缓存。

文件传输：点击“选择文件并发送”，接收端弹窗显示发起者、系统、连接方式、文件名和大小，选择新保存位置后开始；可以拒绝。支持进度、取消和 SHA-256 校验，不覆盖已有文件。文件请求 2 分钟过期。

蓝牙：首次仍须在系统设置完成蓝牙配对。等待连接的一端显示地址与频道，发起端填写后建立链路。Windows 和 macOS 均可选择“等待对方连接”或“主动连接对方”，支持同系统和跨系统组合；两端选择不同角色。链路就绪后任一端点击“连接蓝牙设备”。蓝牙与局域网使用同样的应用授权流程。
连接方式显示“蓝牙 / 本机桥接”表示回环桥接地址；其他地址显示“局域网 / IP 直连”。设备名称和系统是对方提供的信息。

本机 Windows 构建未签名，需要 WebView2 Runtime。Mac 客户端及安装说明：https://github.com/lucaschasel/personal-device-link-mac。
Windows 自动化测试使用模拟输入/剪贴板；自动化回环模拟已覆盖双向传输和角色互换；本版新增蓝牙组合仍需实机验收。仅用于受信任设备与网络；应用层 TCP 尚未加密。

'@ | Set-Content -Encoding utf8 (Join-Path $outputDirectory '使用说明.txt')
Compress-Archive -LiteralPath $outputDirectory -DestinationPath (Join-Path $projectRoot 'dist/personal-device-link-v0151-windows.zip') -Force
Get-Item -LiteralPath (Join-Path $outputDirectory 'Personal Device Link.exe'), (Join-Path $projectRoot 'dist/personal-device-link-v0151-windows.zip') | Select-Object FullName, Length
