# Stage 2 设备发现

基于 [mdns-sd](https://docs.rs/mdns-sd/latest/mdns_sd/)，不依赖系统 Bonjour 服务或云服务器。服务类型为 `_pdl._tcp.local.`，IPv4 组播使用 UDP 5353；数据连接仍使用 Stage 1 TCP 协议。

## 服务记录

- SRV：实际已绑定的 TCP 端口，支持 `--bind 0.0.0.0:0` 自动分配端口。
- A：可监听的 IPv4 地址。绑定所有网卡时自动维护地址；绑定指定 IP 时仅通过对应接口广播。
- TXT：`id`（随机 UUID）、`name`、`os`、`version`（控制协议版本，Stage 4+5 为 3）。
- DNS 实例名称和主机名由 UUID 构成，显示名保存在 TXT，支持中文。

每次启动接收端产生新的实例 ID。设备表以完整 DNS 服务实例为键，多网卡上的同一实例不会显示为多台设备；两台同名设备仍保留为不同项。同一接收进程排除自己的广播，但另一独立进程的 `discover` 仍能看到本机接收端，便于诊断。

拒绝不兼容版本、缺失/不合法的字段、端口 0、无有效 IPv4 地址、控制字符名称及过多地址。每台最多 32 个地址，设备表最多 256 项。优先尝试私有 IPv4 地址；连接失败时尝试其他地址，每个地址的连接/握手单次 I/O 超时 3 秒。

发现记录和 Hello 中的名称、OS 必须一致，否则不使用该连接。发现信息不代表可信身份；键盘和鼠标控制另需接收端显式开启和随机控制码，控制码从不进入 mDNS。文件操作的接收确认在后续阶段实现。

## 在线状态与退出

`ServiceResolved` 新增或更新 `Online`，重复记录不重复输出。`ServiceRemoved` 标记为 `Offline`。非法更新也会撤下旧的在线记录。

每 15 秒对在线实例重新确认，等待 10 秒，期间不重复延长确认窗口。强制退出/断网一般在约 25 秒内反映为离线；拥塞、丢包或休眠可能影响时间，不作实时保证。正常 `Ctrl+C` 停止 TCP 接收循环、注销广播（goodbye）并关闭发现线程。离线记录 60 秒后从内部表清理；终端历史行不会被擦除。

`Online` 是服务发现层状态。防火墙可能允许 mDNS 却禁止 TCP，此时列表可见，但连接会报错。

## CLI 与兼容范围

`listen` 同时监听 TCP、发布广播、观察其他接收端。`discover` 默认扫描 5 秒后输出快照；`discover --watch` 持续输出变化。`connect --device 名称或完整ID` 先扫描（默认 5 秒，可用 `--seconds 1..60` 修改），拒绝重名歧义，再建立原有 Hello/心跳连接。

有限扫描可能漏掉启动较慢或被网络阻断的设备，可延长到 10 秒或使用 watch。未发现时给出排查提示；按不存在/离线名称连接返回非零退出码。

自动发现仅支持 IPv4；IPv6 仍可通过数字地址直连。回环地址、IPv6 监听或 `--no-discovery` 不发布 mDNS。发现模块失败时 TCP 接收端继续运行，并输出错误以便使用 IP 直连。

## 排查顺序

1. 两端使用同一阶段的程序。Stage 1 不广播；Stage 4+5 协议版本为 3，与 Stage 1/2/3 不兼容。
2. 接收端显示 `mDNS enabled`，未绑定到 `127.0.0.1`。
3. 同一 Wi-Fi / 热点，未开启客户端隔离。
4. Windows 允许程序的 UDP 5353 与 TCP 端口；Mac 允许运行程序访问本地网络。
5. 多网卡/VPN 环境可使用 `listen --bind 当前Wi-Fi的IPv4:45832` 限制广播接口。
6. 使用 `connect IP:45832 --once` 区分 TCP 问题和组播发现问题。

API 参考：[ServiceDaemon](https://docs.rs/mdns-sd/latest/mdns_sd/struct.ServiceDaemon.html)、[ServiceInfo](https://docs.rs/mdns-sd/latest/mdns_sd/struct.ServiceInfo.html)。
