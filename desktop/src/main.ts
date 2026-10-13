import { invoke } from './bridge';
import './style.css';

import type { Snapshot } from './types';
import { view, icon } from './view';
import { escape, filterTransfers, addressInput, bluetoothInputError, transferEmptyMessage } from './ui';

document.querySelector('#app')!.innerHTML = view;
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const show = (id: string, visible: boolean) => { $(id).hidden = !visible; };
const text = (id: string, value: string) => { if ($(id).textContent !== value) $(id).textContent = value; };
let current: Snapshot | undefined;
let bluetoothRole: 'listen' | 'connect' = 'listen';
let busy = false, lastPeerHtml = '';
let page = 'overview', transferFilter = 'all';
let lastSnapshot = 0, snapshotRequest = 0;
let previousPhase = '';
let lastTransfersHtml = '', requestId = '';
const prompted = new Set<string>();
const transferPhases: Record<string, string> = { pending: '等待你确认', waiting: '等待对方确认', preparing: '准备接收', transferring: '传输中', verifying: '正在校验与保存', complete: '已完成 · SHA-256 一致', failed: '传输失败', cancelled: '已取消', rejected: '已拒绝' };
const finished = (phase: string) => ['complete', 'failed', 'cancelled', 'rejected'].includes(phase);
function bytes(n: number) { const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; let unit = 0; while (n >= 1024 && unit < units.length - 1) { n /= 1024; unit++; } return `${n.toFixed(unit ? 1 : 0)} ${units[unit]}`; }
function renderTransfers(s: Snapshot) {
  const connected = ['connected', 'controlling', 'updating'].includes(s.connection.phase);
  $<HTMLButtonElement>('send-file').disabled = !connected || !s.connection.capabilities.files || busy || s.closing;
  text('file-target', connected ? `发送到 ${s.connection.name}；接收请求会在本窗口确认。` : '连接设备后，即可双向发送文件。');
  const visible = filterTransfers(s.transfers, transferFilter);
  text('transfer-count', String(visible.length));
  const [emptyTitle, emptyDetail] = transferEmptyMessage(transferFilter);
  for (const id of prompted) if (!s.transfers.some(t => t.id === id && t.phase === 'pending')) prompted.delete(id);
  const html = visible.length ? visible.map(t => {
    const pct = t.size ? Math.min(100, t.transferred / t.size * 100) : t.phase === 'complete' ? 100 : 0;
    return `<article class="transfer"><div class="transfer-top"><div><h3>${escape(t.name)}</h3><small>${t.direction === 'incoming' ? '来自' : '发送到'} ${escape(t.peer)} · ${escape(t.address)} · ${bytes(t.size)}</small></div><span class="pill">${transferPhases[t.phase] || escape(t.phase)}</span></div><progress max="100" value="${pct}" aria-label="${escape(t.name)} 传输进度"></progress><div class="transfer-meta"><span>${pct.toFixed(1)}% · ${bytes(t.transferred)} / ${bytes(t.size)}${['transferring', 'verifying'].includes(t.phase) ? ` · 平均 ${bytes(t.bytes_per_second)}/s` : ''}</span><div>${t.phase === 'pending' ? `<button class="secondary" data-file="accept_file" data-id="${t.id}" ${busy ? 'disabled' : ''}>选择位置并接收</button> <button class="text-button" data-file="reject_file" data-id="${t.id}" ${busy ? 'disabled' : ''}>拒绝</button>` : !finished(t.phase) ? `<button class="text-button" data-file="cancel_file" data-id="${t.id}">取消传输</button>` : ''}</div></div>${t.error ? `<p class="error-text">${escape(t.error)}</p>` : ''}${t.phase === 'complete' && t.path ? `<p class="file-path">已保存：${escape(t.path)}</p>` : ''}${t.sha256 ? `<details class="checksum"><summary>SHA-256 校验值</summary><code>${escape(t.sha256)}</code></details>` : ''}</article>`;
  }).join('') : `<div class="transfer-empty"><h3>${emptyTitle}</h3><p>${emptyDetail}</p></div>`;
  if (html !== lastTransfersHtml) { $('transfers').innerHTML = html; lastTransfersHtml = html; }
  const dialog = $<HTMLDialogElement>('file-request');
  if (requestId && !s.transfers.some(t => t.id === requestId && t.phase === 'pending')) { dialog.close(); requestId = ''; }
  const pending = s.transfers.find(t => t.phase === 'pending' && !prompted.has(t.id));
  if (pending && !dialog.open && !$<HTMLDialogElement>('consent-request').open && !busy && !s.closing) {
    requestId = pending.id; prompted.add(pending.id);
    text('request-name', pending.name); text('request-peer', `${pending.peer}（${pending.peer_os}）通过 ${pending.transport}（${pending.address}）向本机发送 ${bytes(pending.size)} 文件`);
    dialog.showModal();
    void invoke('show_request_window').catch(() => {});
  }
}
const phases: Record<string, string> = { idle: '未连接', connecting: '连接中', connected: '已连接', preparing: '等待授权…', controlling: '控制中', updating: '切换中…', stopping: '停止中…', disconnecting: '断开中…' };
let consentId = '', decidingConsent = false;
function renderConsent(s: Snapshot) {
  const dialog = $<HTMLDialogElement>('consent-request');
  if (consentId && !s.requests.some(r => r.id === consentId)) { dialog.close(); consentId = ''; }
  const request = s.requests[0];
  if (!request || dialog.open || $<HTMLDialogElement>('file-request').open || busy || s.closing) return;
  consentId = request.id;
  text('consent-title', request.kind === 'clipboard' ? '请求双向共享剪贴板' : request.kind === 'control_update' ? '请求变更控制权限' : '请求控制本机');
  text('consent-peer', `${request.device.name}（${request.device.os}） · ${request.transport} · ${request.address}`);
  const o = request.options;
  text('consent-details', o ? `本次请求：${[o.keyboard ? '键盘控制' : '', o.pointer ? '鼠标 / 触摸板控制' : ''].filter(Boolean).join('、')}。快捷键适配${o.shortcut_mapping ? '开启' : '关闭'}。同意后持续到任一端停止控制或连接断开。` : '同意后，双方新复制的文字、图片和普通文件会自动同步，包括密码等敏感内容。旧内容不自动发送；任一端可关闭共享。');
  dialog.showModal();
  void invoke('show_request_window').catch(() => {});
}
function render(s: Snapshot) {
  current = s;
  const runningTransfers = s.transfers.filter(t => !finished(t.phase)).length;
  text('nav-transfer-count', String(runningTransfers)); show('nav-transfer-count', runningTransfers > 0);
  text('header-status', s.connection.phase === 'idle' ? (s.receiver.active ? '正在被控制' : '未连接') : phases[s.connection.phase] || s.connection.phase);
  const controlling = ['preparing', 'controlling', 'updating', 'stopping'].includes(s.connection.phase);
  show('global-session', controlling || s.receiver.active);
  text('global-session-text', s.receiver.active ? '另一台设备正在控制本机' : `${s.connection.name} · ${phases[s.connection.phase] || ''}`);
  text('global-stop', s.connection.phase === 'preparing' ? '撤回请求' : '停止控制');
  $<HTMLButtonElement>('global-stop').disabled = s.closing || s.connection.phase === 'stopping';
  text('service-status', s.network.phase === 'listening' ? '本机服务已就绪' : '本机服务未就绪');
  $('service-dot').classList.toggle('gray', s.network.phase !== 'listening');
  $<HTMLButtonElement>('go-files').disabled = !['connected', 'controlling', 'updating'].includes(s.connection.phase);
  renderConsent(s);
  renderTransfers(s);
  const bt = s.bluetooth, btActive = ['starting', 'waiting', 'ready'].includes(bt.phase);
  if (btActive) bluetoothRole = bt.role;
  renderBluetoothRole(btActive || busy || s.closing);
  text('bt-status', ({ off: '未开启', starting: '正在启动…', waiting: '等待对方连接', ready: '蓝牙链路已就绪', error: '已停止 / 失败' } as Record<string,string>)[bt.phase] || bt.phase);
  text('bt-start', bt.phase === 'starting' ? '正在建立…' : bt.phase === 'waiting' ? '等待对方加入…' : bluetoothRole === 'listen' ? '等待蓝牙连接' : '建立蓝牙连接');
  const btError = bluetoothRole === 'connect' ? bluetoothInputError($<HTMLInputElement>('bt-address').value, Number($<HTMLInputElement>('bt-channel').value)) : '';
  text('bt-input-hint', btError || '地址和频道格式正确，可发起连接。');
  show('bt-input-hint', bluetoothRole === 'connect' && !btActive);
  $<HTMLButtonElement>('bt-start').disabled = busy || s.closing || btActive || !!btError;
  show('bt-start', bt.phase !== 'ready');
  show('bt-stop', btActive); show('bt-connect', bt.phase === 'ready');
  $<HTMLButtonElement>('bt-stop').disabled = s.closing;
  const applicationOnBluetooth = s.connection.address === '127.0.0.1:45833' && s.connection.phase !== 'idle';
  text('bt-connect', applicationOnBluetooth ? '已连接此设备' : '连接蓝牙设备');
  $<HTMLButtonElement>('bt-connect').disabled = busy || s.closing || !['idle', 'error'].includes(s.connection.phase);
  show('bt-local', !!bt.address && btActive); text('bt-local', `本机蓝牙地址：${bt.address} · 频道：${bt.channel}（在对方设备输入）`);
  text('bt-detail', bt.detail);
  const cb = s.clipboard;
  text('clipboard-status', ({ off: '关闭', ready: '尚未共享', connecting: '等待对方确认…', syncing: '同步中', stopping: '正在停止…' } as Record<string, string>)[cb.phase] || cb.phase);
  $<HTMLInputElement>('clipboard-toggle').checked = cb.enabled;
  show('clipboard-options', cb.enabled || cb.phase === 'stopping' || cb.connections > 0 || ['connected', 'controlling', 'updating', 'preparing'].includes(s.connection.phase));
  $<HTMLInputElement>('clipboard-toggle').disabled = s.closing || cb.phase === 'stopping' || (busy && !cb.enabled) || (!cb.enabled && !['connected', 'controlling', 'updating'].includes(s.connection.phase));
  show('clipboard-connect', cb.enabled && cb.phase === 'ready');
  show('clipboard-send-current', cb.phase === 'syncing');
  $<HTMLButtonElement>('clipboard-send-current').disabled = busy || s.closing;
  $<HTMLButtonElement>('clipboard-connect').disabled = busy || s.closing || !['connected', 'controlling', 'updating'].includes(s.connection.phase) || cb.phase !== 'ready';
  text('clipboard-detail', cb.phase === 'syncing' ? `与 ${cb.peer} 双向同步 · 已发送 ${cb.sent} 次 / 接收 ${cb.received} 次${cb.activity ? ' · 最近同步：' + cb.activity : ' · 请重新复制内容'}` : cb.enabled ? cb.phase === 'connecting' ? '已发送共享请求，等待对方同意；取消勾选可撤回。' : '当前没有同步会话，可重新请求共享。' : '当前不会读取或发送你的剪贴板。');
  text('clipboard-error', cb.error || ''); show('clipboard-error', !!cb.error);
  text('clipboard-cache', cb.cache_path);
  const mac = s.device.os === 'macos', win = s.device.os === 'windows';
  const c = s.connection, r = s.receiver, idle = c.phase === 'idle';
  if ((idle && previousPhase !== 'idle') || (c.phase === 'connected' && ['connecting', 'controlling', 'preparing', 'updating', 'stopping'].includes(previousPhase))) {
    $<HTMLInputElement>('enable-keyboard').checked = false;
    $<HTMLInputElement>('enable-pointer').checked = false;
  }
  if (c.phase === 'controlling' && previousPhase !== 'controlling') {
    $<HTMLInputElement>('enable-keyboard').checked = c.controls.keyboard;
    $<HTMLInputElement>('enable-pointer').checked = c.controls.pointer;
    $<HTMLInputElement>('shortcut-mapping').checked = c.controls.shortcut_mapping;
  }
  previousPhase = c.phase;
  const editing = ['connected', 'controlling'].includes(c.phase) && !busy && !s.closing;
  $<HTMLInputElement>('shortcut-mapping').disabled = !editing;
  $<HTMLInputElement>('enable-keyboard').disabled = !c.capabilities.keyboard || !editing;
  $<HTMLInputElement>('enable-pointer').disabled = !c.capabilities.pointer || !editing;
  text('local-name', s.device.name); text('local-os', mac ? 'macOS' : win ? 'Windows' : s.device.os);
  text('local-detail', s.network.phase === 'listening' ? '本机接收服务已开启 · 键鼠控制需单独授权' : s.network.phase === 'starting' ? '正在启动局域网服务…' : '局域网服务未启动，请查看下方错误并重试');
  text('network-error', s.network.error || ''); show('network-warning', !!s.network.error);
  show('retry-network', s.network.phase === 'off');
  $<HTMLButtonElement>('retry-network').disabled = busy || s.closing;
  text('discovery-status', s.network.phase !== 'listening' ? '等待本机服务' : s.discovery_error ? '发现暂不可用' : '自动发现中');
  text('peer-count', String(s.peers.filter(p => p.online).length));
  text('discovery-error', s.discovery_error ? `自动发现暂不可用：${s.discovery_error}。可尝试 IP 连接。` : ''); show('discovery-error', !!s.discovery_error);
  const html = s.peers.length ? s.peers.map(p => `<article class="peer-card ${p.online ? '' : 'offline'}"><div class="peer-icon">${icon('screen')}</div><div class="peer-info"><h3>${escape(p.name)}</h3><p>${escape(p.os === 'macos' ? 'macOS' : p.os === 'windows' ? 'Windows' : p.os)} · ${escape(p.addresses[0] || '')}</p><small><span class="dot ${p.online ? '' : 'gray'}"></span> ${p.online ? '在线' : '离线'}</small></div><button class="secondary" data-connect="${escape(p.id)}" ${!p.online || !idle || busy || s.closing ? 'disabled' : ''}> ${p.addresses.includes(c.address) && !idle ? '已连接' : '连接'} ${icon('arrow')}</button></article>`).join('') : `<div class="empty-state"><div class="radar">${icon('screen')}</div><h3>等待另一台设备加入</h3><p>在另一台设备打开本应用，<br>附近的设备会自动出现在这里。</p><span class="scanning">正在搜索局域网</span></div>`;
  if (html !== lastPeerHtml) { $('peers').innerHTML = html; lastPeerHtml = html; }
  show('receiver-panel', win || mac);
  text('receiver-state', r.phase === 'listening' ? (r.active ? '正在被控制' : '已开启 · 每次请求均需确认') : r.phase === 'starting' ? '正在启动接收…' : r.phase === 'stopping' ? '正在断开并释放输入…' : '已关闭控制请求 · 不影响发现与文件传输');
  text('receiver-toggle', r.phase === 'off' ? '允许控制请求' : r.phase === 'listening' ? '停止控制并关闭请求' : '处理中…');
  $<HTMLButtonElement>('receiver-toggle').disabled = busy || s.closing || !['off', 'listening'].includes(r.phase);
  text('receiver-error', r.error || ''); show('receiver-error', !!r.error);
  text('connection-status', c.phase === 'controlling' && !c.controls.keyboard && !c.controls.pointer ? '键鼠已暂停' : phases[c.phase] || c.phase); $('connection-status').classList.toggle('connected', ['connected', 'controlling', 'updating'].includes(c.phase));
  text('target-name', idle ? (cb.connections > 0 ? '已有设备连接本机' : '选择一台设备') : c.name || '正在连接对方设备…');
  show('target-transport', !idle && !!c.address);
  text('target-transport', c.address === '127.0.0.1:45833' ? '蓝牙连接' : '局域网 / IP 连接');
  text('target-detail', c.phase === 'updating' ? '正在切换功能；新增权限需对方确认。请松开修饰键与鼠标按钮。' : c.phase === 'preparing' ? '正在准备并等待对方确认；授权请求最长等待 60 秒，可随时取消。' : idle ? (cb.connections > 0 ? '等待对方发送控制、文件或剪贴板请求；收到后在弹窗中确认。' : '连接后即可查看状态。') : c.address || '正在建立设备连接…');
  show('latency', c.latency_ms !== null); text('latency', `${c.latency_ms?.toFixed(1) || '—'} ms · 心跳往返`);
  show('control-form', (mac || win) && (c.capabilities.keyboard || c.capabilities.pointer) && ['connected', 'controlling', 'updating'].includes(c.phase)); show('active-help', c.phase === 'controlling');
  show('start-fields', c.phase === 'connected');
  show('apply-control', ['controlling', 'updating'].includes(c.phase));
  show('switch-help', ['controlling', 'updating'].includes(c.phase));
  text('apply-control', c.phase === 'updating' ? '正在应用…' : '应用选择');
  const unchanged = $<HTMLInputElement>('enable-keyboard').checked === c.controls.keyboard
    && $<HTMLInputElement>('enable-pointer').checked === c.controls.pointer
    && $<HTMLInputElement>('shortcut-mapping').checked === c.controls.shortcut_mapping;
  $<HTMLButtonElement>('apply-control').disabled = c.phase !== 'controlling' || !editing || unchanged;
  text('active-description', `正在控制 ${c.name}：${[c.controls.keyboard ? '键盘' : '', c.controls.pointer ? '鼠标 / 触摸板' : ''].filter(Boolean).join(' + ') || '键鼠已暂停，可重新选择后应用'}`);
  text('local-pointer-help', c.controls.pointer ? `按住 ${mac ? 'Control + Option' : 'Ctrl + Alt'} 可操作本机鼠标，再点击停止控制。` : '鼠标保留在本机，可随时点击停止控制。');
  text('emergency-chord', mac ? 'Control + Option + Esc' : 'Ctrl + Alt + Esc');
  show('stop-control', ['preparing', 'controlling', 'updating', 'stopping'].includes(c.phase));
  text('stop-control', c.phase === 'preparing' ? '取消开始' : c.phase === 'stopping' ? '正在停止…' : '停止控制');
  $<HTMLButtonElement>('stop-control').disabled = c.phase === 'stopping' || s.closing;
  show('disconnect', !idle); text('disconnect', c.phase === 'connecting' ? '取消连接' : '断开连接');
  $<HTMLButtonElement>('disconnect').disabled = c.phase === 'disconnecting' || s.closing;
  $<HTMLButtonElement>('manual-connect').disabled = !idle || busy || s.closing;
  $<HTMLButtonElement>('start-control').disabled = busy || s.closing || (!$<HTMLInputElement>('enable-keyboard').checked && !$<HTMLInputElement>('enable-pointer').checked);
  text('connection-error', c.error || ''); show('connection-error', !!c.error);
  if (s.closing) { text('notice-text', '正在停止控制并释放输入，完成后关闭窗口…'); show('notice', true); }
}
async function refresh() {
  const request = ++snapshotRequest;
  const snapshot = await invoke<Snapshot>('snapshot');
  if (request > lastSnapshot) { lastSnapshot = request; render(snapshot); }
}
async function action(command: string, args?: Record<string, unknown>) {
  const interrupt = ['cancel_file', 'stop_control', 'disconnect_device', 'stop_bluetooth', 'disable_clipboard', 'decide_consent', 'stop_receiver', 'reject_file'].includes(command);
  if (busy && !interrupt) return;
  if (!interrupt) busy = true;
  show('notice', false); if (current) render(current);
  try { await invoke(command, args); await refresh(); }
  catch (error) { text('notice-text', String(error)); show('notice', true); }
  finally { if (!interrupt) busy = false; if (current) render(current); }
}
$('peers').addEventListener('click', event => { const button = (event.target as Element).closest<HTMLButtonElement>('[data-connect]'); if (button && !button.disabled) void action('connect_device', { id: button.dataset.connect, address: null }); });
$('manual-form').addEventListener('submit', event => { event.preventDefault(); void action('connect_device', { id: null, address: addressInput($<HTMLInputElement>('address').value) }); });
$('control-form').addEventListener('submit', event => { event.preventDefault(); if (current?.connection.phase !== 'connected') return; void action('start_control', { options: { keyboard: $<HTMLInputElement>('enable-keyboard').checked, pointer: $<HTMLInputElement>('enable-pointer').checked, shortcut_mapping: $<HTMLInputElement>('shortcut-mapping').checked } }); });
for (const id of ['enable-keyboard', 'enable-pointer', 'shortcut-mapping']) $(id).addEventListener('change', () => { if (current) render(current); });
$('stop-control').addEventListener('click', () => void action('stop_control'));
function renderBluetoothRole(disabled: boolean) {
  show('bt-client', bluetoothRole === 'connect');
  for (const role of ['listen', 'connect'] as const) {
    const button = $<HTMLButtonElement>(`bt-role-${role}`);
    button.disabled = disabled;
    button.classList.toggle('selected', bluetoothRole === role);
    button.setAttribute('aria-pressed', String(bluetoothRole === role));
  }
  $<HTMLInputElement>('bt-address').disabled = disabled;
  $<HTMLInputElement>('bt-channel').disabled = disabled;
}
for (const role of ['listen', 'connect'] as const) $('bt-role-' + role).addEventListener('click', () => {
  bluetoothRole = role;
  if (current) render(current);
});
for (const id of ['bt-address', 'bt-channel']) $(id).addEventListener('input', () => { if (current) render(current); });
$('bt-start').addEventListener('click', () => void action('start_bluetooth', { role: bluetoothRole, address: $<HTMLInputElement>('bt-address').value.trim(), channel: bluetoothRole === 'connect' ? Number($<HTMLInputElement>('bt-channel').value) : 0 }));
$('bt-stop').addEventListener('click', () => void action('stop_bluetooth'));
$('bt-connect').addEventListener('click', () => void action('connect_bluetooth'));
$('clipboard-toggle').addEventListener('change', () => void action(current?.clipboard.enabled ? 'disable_clipboard' : 'connect_clipboard'));
$('clipboard-send-current').addEventListener('click', () => void action('sync_current_clipboard'));
$('clipboard-connect').addEventListener('click', () => void action('connect_clipboard'));
$('apply-control').addEventListener('click', () => void action('update_control', { options: { keyboard: $<HTMLInputElement>('enable-keyboard').checked, pointer: $<HTMLInputElement>('enable-pointer').checked, shortcut_mapping: $<HTMLInputElement>('shortcut-mapping').checked } }));
$('retry-network').addEventListener('click', () => void action('retry_network'));
$('disconnect').addEventListener('click', () => void action('disconnect_device'));
$('send-file').addEventListener('click', () => void action('choose_and_send'));
$('transfers').addEventListener('click', event => { const button = (event.target as Element).closest<HTMLButtonElement>('[data-file]'); if (button && !button.disabled) void action(button.dataset.file!, { id: button.dataset.id }); });
async function decideConsent(allow: boolean) {
  const id = consentId;
  if (!id || decidingConsent) return;
  decidingConsent = true;
  $<HTMLButtonElement>('consent-accept').disabled = true;
  $<HTMLButtonElement>('consent-reject').disabled = true;
  try { await action('decide_consent', { id, allow }); }
  finally { decidingConsent = false; $<HTMLButtonElement>('consent-accept').disabled = false; $<HTMLButtonElement>('consent-reject').disabled = false; }
}
$('consent-accept').addEventListener('click', () => void decideConsent(true));
$('consent-reject').addEventListener('click', () => void decideConsent(false));
$('consent-request').addEventListener('cancel', event => { event.preventDefault(); void decideConsent(false); });
$('request-later').addEventListener('click', () => $<HTMLDialogElement>('file-request').close());
for (const [button, command] of [['request-accept', 'accept_file'], ['request-reject', 'reject_file']]) $(button).addEventListener('click', () => { const id = requestId; $<HTMLDialogElement>('file-request').close(); void action(command, { id }); });
$('receiver-toggle').addEventListener('click', () => void action(current?.receiver.phase === 'off' ? 'start_receiver' : 'stop_receiver'));
function navigate(next: string) {
  page = next;
  for (const item of ['overview', 'files', 'help']) {
    show(item, item === page);
    $(item + '-nav').classList.toggle('active', item === page);
    if (item === page) $(item + '-nav').setAttribute('aria-current', 'page');
    else $(item + '-nav').removeAttribute('aria-current');
  }
  text('page-title', ({ overview: '设备连接', files: '文件传输', help: '使用帮助' } as Record<string,string>)[page]);
  text('page-description', ({ overview: '选择连接方式，让你的设备一起工作。', files: '每一次发送，都由对方确认接收。', help: '连接、授权与日常操作，一次了解。' } as Record<string,string>)[page]);
}
for (const item of ['overview', 'files', 'help']) $(item + '-nav').addEventListener('click', () => navigate(item));
$('go-files').addEventListener('click', () => navigate('files'));
$('view-session').addEventListener('click', () => navigate('overview'));
$('global-stop').addEventListener('click', () => void action(current?.receiver.active ? 'stop_receiver' : 'stop_control'));
$('dismiss-notice').addEventListener('click', () => show('notice', false));
for (const method of ['lan', 'bluetooth']) $('method-' + method).addEventListener('click', () => {
  for (const item of ['lan', 'bluetooth']) { show(item + '-method', item === method); $('method-' + item).classList.toggle('selected', item === method); $('method-' + item).setAttribute('aria-pressed', String(item === method)); }
});
document.querySelectorAll<HTMLButtonElement>('[data-filter]').forEach(button => button.addEventListener('click', () => {
  transferFilter = button.dataset.filter!;
  document.querySelectorAll<HTMLButtonElement>('[data-filter]').forEach(b => { b.classList.toggle('selected', b === button); b.setAttribute('aria-pressed', String(b === button)); });
  if (current) renderTransfers(current);
}));
async function poll() {
  try { await refresh(); }
  catch { text('notice-text', '无法连接桌面服务。请从 Personal Device Link 桌面应用打开此界面。'); show('notice', true); }
  setTimeout(() => void poll(), 500);
}
void poll();
