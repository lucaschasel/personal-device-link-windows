export type Peer = { id: string; name: string; os: string; addresses: string[]; online: boolean };
export type Transfer = { id: string; direction: string; peer: string; peer_os: string; transport: string; address: string; name: string; size: number; transferred: number; bytes_per_second: number; phase: string; error: string | null; path: string | null; sha256: string | null };
export type Approval = { id: string; device: { name: string; os: string }; address: string; transport: string; kind: string; options: { keyboard: boolean; pointer: boolean; shortcut_mapping: boolean } | null };
export type Snapshot = {
  requests: Approval[];
  bluetooth: { role: 'listen' | 'connect'; phase: string; address: string; channel: number; detail: string };
  clipboard: { connections: number; enabled: boolean; code: string | null; phase: string; peer: string; error: string | null; sent: number; received: number; activity: string; cache_path: string };
  network: { phase: string; address: string; error: string | null };
  transfers: Transfer[];
  device: { name: string; os: string }; peers: Peer[]; discovery_error: string | null; closing: boolean;
  receiver: { phase: string; address: string; code: string | null; active: boolean; error: string | null };
  connection: { phase: string; name: string; os: string; address: string; error: string | null; latency_ms: number | null; capabilities: { keyboard: boolean; pointer: boolean; files: boolean }; controls: { keyboard: boolean; pointer: boolean; shortcut_mapping: boolean } };
};
