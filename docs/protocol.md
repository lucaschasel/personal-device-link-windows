# 控制通道协议（0.13.0 / version 7）

0.13.0 使用协议 7，剪贴板子协议 2，文件子协议仍为 1。桌面启动即接受普通连接与授权请求，但所有控制与剪贴板会话都必须由接收端逐次确认。HelloAck 只表示能力，不代表授权。停止控制请求撤销活动控制及待处理授权。CLI 诊断模式保留令牌验证，桌面不会把有效令牌当作跳过弹窗的依据。

传输层使用 TCP。端口默认 `45832`，可由 CLI 参数覆盖。一个客户端对应一个长期会话；当前服务器最多同时处理 16 个连接，每个连接一个工作线程。TCP 设置 `TCP_NODELAY`，避免小消息受到 Nagle 合并延迟。实现基于 [Rust TcpStream](https://doc.rust-lang.org/std/net/struct.TcpStream.html)。

## 帧边界

桌面剪贴板使用独立 TCP 连接：首帧 `clipboard_hello` 为 `{version:2, token:"", device:<设备信息>}`。接收端显示双向文字/图片/文件共享请求，60 秒内同意且会话仍有效时才开启本机共享、创建剪贴板后端并回复 `clipboard_ready`。请求端等待最多 70 秒；握手后恢复 10 秒读写超时。拒绝、过期、断开、关闭共享、退出均不可获得授权。桌面拒绝旧子协议 1；模拟测试保留无弹窗实例的旧令牌路径。

握手后不再使用控制消息帧，而是请求端发送一份剪贴板记录、接收端回复一份记录，空记录表示本轮无变化。元数据为 u32 大端长度及最多 16384 字节的 JSON，`kind` 为 `Empty`、`Text`（`size`）、`Image`（`width,height`）或 `Files`（`files:[{name,size}]`）。每份非空正文后附 32 字节 SHA-256；文件列表中每个文件各有正文与校验值。正文按 64 KiB 流式处理，长度、名称和总量先验证。文字 1 MiB、图片 RGBA 64 MiB、每批最多 16 个文件且共 256 MiB；文件校验全部完成后才发布系统剪贴板。连接使用 10 秒单次读写超时，关闭共享直接关闭套接字，不自动重连。完整行为见 [剪贴板说明](desktop.md#剪贴板与文件)。

```text
+----------------------+--------------------------------+
| 长度: u32 big-endian  | UTF-8 JSON，长度为前缀指定字节数 |
| 4 bytes              | 1 .. 65536 bytes               |
+----------------------+--------------------------------+
```

所有消息使用 `{"type": "...", "payload": {...}}`。前缀表示 JSON 的字节数，不是字符数，也不包含前缀本身。读取时先验证长度上限，再分配缓冲区。连续消息可共用一次 TCP 包，一条消息也可以跨多个 TCP 包。

帧边界上的 EOF 视为正常关闭。头部/正文截断、非法长度、非法 UTF-8/JSON、未知消息类型、缺失字段或超时导致会话终止。发生读取错误后不能重试解析剩余字节，必须重新建立连接。

## 握手与状态

客户端首先发送：

```json
{"type":"hello","payload":{"version":7,"device":{"name":"MacBook","os":"macos"},"text":"Hello"}}
```

服务器校验版本及设备信息，然后记录收到的 `Hello` 并回复：

```json
{"type":"hello_ack","payload":{"version":7,"device":{"name":"Windows-PC","os":"windows"},"text":"Hello","capabilities":{"keyboard":true,"pointer":true,"files":true}}}
```

设备名称为非空 UTF-8 字符串，最长 128 字节；OS 最长 32 字节。两者均不允许控制字符。OS 使用 Rust 的 `std::env::consts::OS`，如 `macos`、`windows`。此设备信息是对端声明的数据，不代表经过认证的身份。

握手完成后，客户端每隔 5 秒在同一连接发送心跳：

```json
{"type":"ping","payload":{"sequence":1}}
```

服务器用原序号回复：

```json
{"type":"pong","payload":{"sequence":1}}
```

客户端只允许一个未完成的心跳，并验证响应序号。计时显示应用层往返耗时，不代表未来输入处理延迟。

正常单次测试结束时客户端发送：

```json
{"type":"goodbye","payload":{"reason":"client finished"}}
```

服务器关闭连接，不再回复。Stage 2 中客户端 `Ctrl+C` 请求停止心跳并发送 goodbye；接收端 `Ctrl+C` 停止监听、注销 mDNS 广播后退出进程。若退出时正在阻塞读写，需要等待当前操作返回或超时。强制结束进程时，对端通过 EOF、连接错误或超时识别断开。

版本不匹配或状态不允许的消息会收到 `error`，然后断开：

```json
{"type":"error","payload":{"message":"unsupported protocol version"}}
```

普通连接读写超时为每次阻塞 I/O 的 15 秒。键盘控制会话使用每帧 2 秒总读取期限，防止部分帧无限延长按键保持时间；客户端控制发送超时 500 ms。每条控制消息最多 64 KiB，连接数有上限；协议仅适用于受信任局域网。

## 键鼠控制状态

完成 Hello 后发送桌面控制请求（空 token 表示等待接收端确认）：

```json
{"type":"control_request","payload":{"token":"","options":{"keyboard":true,"pointer":false,"shortcut_mapping":true}}}
```

桌面接收端必须允许控制请求、用户在 60 秒内同意且没有其他控制会话，才创建输入后端并回复 `{"type":"control_started","payload":{}}`。否则回复 error 并关闭此连接。

GUI 默认允许请求，但实际输入始终需要弹窗确认。Windows 与 macOS 均可接收或发起，角色不由 OS 决定。HelloAck 的能力以当前接收服务为准；普通 CLI 不支持文件传输，files 为 false。控制请求中的 keyboard/pointer 至少一个为 true，未选择类别的输入将被接收端拒绝并关闭会话、释放已有输入。shortcut_mapping 由主动方在原生键码转换阶段应用；网络键位使用统一 Key 枚举。

授权后键盘事件示例：

```json
{"type":"keyboard","payload":{"key":"a","pressed":true,"repeat":false}}
{"type":"keyboard","payload":{"key":"a","pressed":false,"repeat":false}}
```

键名为明确的枚举，不能传任意扫描码。重复事件必须对应已按下的普通键，修饰键不能重复；释放未由本会话按下的键不会调用输入 API。

控制期间每 500 ms 发送 ping，pong 格式不变。停止时发送 `{"type":"stop_control","payload":{}}`，接收端释放所有本会话鼠标按钮和按键并回复 `{"type":"control_stopped","payload":{}}`，TCP 仍可继续普通心跳。Goodbye、EOF、协议错误、超时和接收端退出也会触发鼠标按钮和按键清理。

Mac 紧急退出先停止本地捕获，再清理网络会话；不会将紧急组合键中的 Escape 发到 Windows。接收端仅允许一个控制租约，先释放输入状态再交还租约。

文件传输使用独立 TCP 连接和 1 MiB 流式分块，文件子协议仍为 1，详见 [文件说明](files.md)。

## 控制中更新选项

```json
{"type":"update_control","payload":{"options":{"keyboard":false,"pointer":true,"shortcut_mapping":false}}}
{"type":"control_updated","payload":{"options":{"keyboard":false,"pointer":true,"shortcut_mapping":false}}}
```

更新必须已有有效控制租约。接收端先释放本会话全部按键和鼠标按钮，成功后修改选项并确认；未授权或已撤销会话不能靠更新消息取得控制。客户端停止旧捕获、弃用旧队列，验证确认中的选项与请求一致，再启动新配置。协议输入仍按最新选择严格过滤。

UpdateControl 允许键盘与鼠标同时为 false，表示暂停输入；心跳和独占租约保持，再次更新可恢复，停止控制才释放租约。初次 ControlRequest 仍要求至少启用一项。

## 鼠标事件

与键盘事件共用控制租约和 TCP 顺序，仅授权会话可发送：

```json
{"type":"pointer","payload":{"event":{"kind":"move","dx":12,"dy":-4}}}
{"type":"pointer","payload":{"event":{"kind":"button","button":"left","pressed":true}}}
{"type":"pointer","payload":{"event":{"kind":"button","button":"left","pressed":false}}}
{"type":"pointer","payload":{"event":{"kind":"scroll","horizontal":-30,"vertical":120}}}
```

`dx/dy` 为相对位移，右/下为正。滚动使用 Windows wheel units，120 为一格，上/右为正。每个轴范围 -32767..32767；按钮仅 `left`、`right`、`middle`。重复按下被忽略，未持有按钮的松开不调用 OS API。两次 down/up 由 Windows 判定双击，不新增 double-click 注入指令。双轴滚动在接收端生成独立的 WHEEL 与 HWHEEL 输入。

本地队列最多 512 个事件；合并只发生在相邻同向移动/滚动之间，不跨越键盘/按钮消息。溢出停止控制；停止后不继续发送旧队列。关闭会话先释放按钮再释放键盘状态。用户体验及平台限制见 [桌面端使用说明](desktop.md)。

## 0.13.0 授权状态约束

每项授权请求绑定当前 TCP 连接与 UUID，最多 8 项等待，决定仅使用一次；收到授权前输入、连接结束或请求失效时拒绝。接收端关闭/重启授权后，旧请求不可恢复。设备名称和 OS 为对方声明；地址取 TCP peer_addr，回环显示“蓝牙 / 本机桥接”，其他显示“局域网 / IP 直连”，不宣称回环地址证明蓝牙身份。

UpdateControl 先释放当前按键及鼠标按钮；新增键盘/鼠标或在启用键盘时改变快捷键适配，需要新的确认。减少功能或暂停不弹窗。等待期间主动方停止捕获；拒绝/超时结束该控制连接。请求端在等待中取消会关闭对应 socket。活动输入每帧看门狗仍为 2 秒。

剪贴板批准只授权本次双向会话；失败、关闭、断开后退回关闭状态，不自动重连。原有逐文件保存确认继续保留，并补齐 peer_os 与 transport 元数据。
