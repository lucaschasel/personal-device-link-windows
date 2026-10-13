// Development-only fixtures. This module has no access to native commands.
import type { Snapshot } from './types';
const connected = { phase: 'connected', name: 'Studio MacBook', os: 'macos', address: '192.168.1.23:45832', error: null, latency_ms: 4.2, capabilities: { keyboard: true, pointer: true, files: true }, controls: { keyboard: false, pointer: false, shortcut_mapping: true } };
const state: Snapshot = {
  requests: [], bluetooth: { role: 'listen', phase: 'off', address: '', channel: 0, detail: '' },
  clipboard: { connections: 0, enabled: false, code: null, phase: 'off', peer: '', error: null, sent: 0, received: 0, activity: '', cache_path: '/mock/clipboard' },
  network: { phase: 'listening', address: '0.0.0.0:45832', error: null },
  device: { name: '我的工作电脑', os: 'windows' }, peers: [{ id: 'mock-mac', name: 'Studio MacBook', os: 'macos', addresses: ['192.168.1.23:45832'], online: true }], discovery_error: null, closing: false,
  receiver: { phase: 'listening', address: '0.0.0.0:45832', code: null, active: false, error: null },
  connection: { ...connected, phase: 'idle', name: '', address: '', latency_ms: null }, transfers: []
};
const params = new URLSearchParams(location.search);
const scenario = params.get('preview');
if (params.get('os') === 'macos') state.device.os = 'macos';
if (scenario !== 'empty') state.connection = { ...connected };
if (scenario === 'empty') state.peers = [];
if (scenario === 'request') state.requests = [{ id: 'request-1', device: { name: 'Studio MacBook', os: 'macos' }, address: '192.168.1.23:45832', transport: '局域网 / IP 直连', kind: 'control', options: { keyboard: true, pointer: false, shortcut_mapping: true } }];
if (scenario === 'files') state.transfers = [
  { id: 'file-1', direction: 'outgoing', peer: 'Studio MacBook', peer_os: 'macos', transport: '局域网 / IP 直连', address: '192.168.1.23:45832', name: '项目资料.zip', size: 52428800, transferred: 24117248, bytes_per_second: 5242880, phase: 'transferring', error: null, path: null, sha256: null },
  { id: 'file-2', direction: 'incoming', peer: 'Studio MacBook', peer_os: 'macos', transport: '局域网 / IP 直连', address: '192.168.1.23:45832', name: '会议记录.pdf', size: 1048576, transferred: 1048576, bytes_per_second: 0, phase: 'complete', error: null, path: 'D:/文件/会议记录.pdf', sha256: '模拟校验值' }
];
export async function invokePreview(command: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case 'snapshot': return structuredClone(state);
    case 'connect_device': state.connection = { ...connected }; break;
    case 'connect_bluetooth': state.connection = { ...connected, address: '127.0.0.1:45833' }; break;
    case 'disconnect_device': state.connection.phase = 'idle'; state.clipboard.enabled = false; state.clipboard.phase = 'off'; break;
    case 'start_control': state.connection.phase = 'preparing'; break;
    case 'stop_control': state.connection.phase = 'connected'; break;
    case 'update_control': state.connection.controls = args?.options as Snapshot['connection']['controls']; break;
    case 'start_receiver': state.receiver.phase = 'listening'; break;
    case 'stop_receiver': state.receiver.phase = 'off'; state.receiver.active = false; break;
    case 'start_bluetooth': state.bluetooth = { role: args?.role as 'listen' | 'connect', phase: args?.role === 'listen' ? 'waiting' : 'ready', address: args?.role === 'listen' ? 'AABBCCDDEEFF' : '', channel: args?.role === 'listen' ? 8 : 0, detail: args?.role === 'listen' ? '预览：等待对方连接' : '预览：蓝牙链路已就绪' }; break;
    case 'stop_bluetooth': state.bluetooth.phase = 'off'; state.bluetooth.detail = ''; break;
    case 'connect_clipboard': state.clipboard.enabled = true; state.clipboard.phase = 'connecting'; break;
    case 'disable_clipboard': state.clipboard.enabled = false; state.clipboard.phase = 'off'; break;
    case 'decide_consent': state.requests = state.requests.filter(r => r.id !== args?.id); break;
    case 'show_request_window': break;
    case 'choose_and_send': throw new Error('预览模式不会打开本机文件或发送文件。');
    default: break;
  }
}
