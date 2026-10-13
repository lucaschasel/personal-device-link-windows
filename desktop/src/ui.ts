import type { Transfer } from './types';
export const escape = (s: string) => s.replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]!));
export const isFinished = (phase: string) => ['complete', 'failed', 'cancelled', 'rejected'].includes(phase);
export function filterTransfers(transfers: Transfer[], filter: string) {
  return transfers.filter(t => filter === 'all' || (filter === 'finished' ? isFinished(t.phase) : !isFinished(t.phase)));
}
export function addressInput(value: string) {
  const address = value.trim();
  return /^\d{1,3}(?:\.\d{1,3}){3}$/.test(address) ? address + ':45832' : address;
}
export function bluetoothInputError(address: string, channel: number): string {
  if (!/^[0-9a-f]{12}$/i.test(address.replace(/[:\- ]/g, ''))) return '请输入对方显示的 12 位蓝牙地址，可包含冒号或短横线。';
  if (!Number.isInteger(channel) || channel < 1 || channel > 30) return '频道必须是对方当前显示的 1–30 之间的整数。';
  return '';
}
export function transferEmptyMessage(filter: string): [string, string] {
  if (filter === 'active') return ['没有进行中的传输', '待确认、传输中和校验中的任务会显示在这里。'];
  if (filter === 'finished') return ['没有已结束的传输', '已完成、取消、拒绝或失败的任务会显示在这里。'];
  return ['还没有传输记录', '连接设备后，选择文件发送；收到的文件也会显示在这里。'];
}
