import { invoke as nativeInvoke } from '@tauri-apps/api/core';
const preview = import.meta.env.DEV && new URLSearchParams(location.search).has('preview');
export async function invoke<T = void>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (preview) return (await import('./preview')).invokePreview(command, args) as T;
  return nativeInvoke<T>(command, args);
}
