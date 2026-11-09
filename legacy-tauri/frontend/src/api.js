// Brücke zwischen Svelte-UI und dem Rust-Backend (Tauri).
// Die Funktionsnamen entsprechen der alten Wails-API, damit die UI-Logik gleich bleibt.
import { invoke, Channel } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';

const err = (e) => 'FEHLER:' + (e?.message ?? e);

function toBytes(m) {
  if (m instanceof ArrayBuffer) return new Uint8Array(m);
  if (ArrayBuffer.isView(m)) return new Uint8Array(m.buffer, m.byteOffset, m.byteLength);
  if (Array.isArray(m)) return new Uint8Array(m);
  return m; // String
}

function channels(onData, onClose) {
  const data = new Channel();
  data.onmessage = (m) => onData(toBytes(m));
  const close = new Channel();
  close.onmessage = onClose;
  return { onData: data, onClose: close };
}

export async function Connect(id, ip, user, pass, privateKey, cols, rows, onData, onClose) {
  try {
    return await invoke('connect', { id, ip, user, pass, privateKey, cols, rows, ...channels(onData, onClose) });
  } catch (e) { return String(e); }
}

export async function ConnectSerial(id, portName, baudRate, onData, onClose) {
  try {
    return await invoke('connect_serial', { id, portName, baudRate, ...channels(onData, onClose) });
  } catch (e) { return String(e); }
}

export const GetSerialPorts = () => invoke('get_serial_ports').catch(() => []);
export const Disconnect = (id) => invoke('disconnect', { id }).catch(() => {});
export const ForgetHostKey = (hostId) => invoke('forget_host_key', { hostId });

// Eingaben strikt in Reihenfolge senden und bündeln, solange ein Aufruf unterwegs ist.
const outQueues = new Map();
export function SendData(id, data) {
  let q = outQueues.get(id);
  if (!q) { q = { buf: '', busy: false }; outQueues.set(id, q); }
  q.buf += data;
  if (!q.busy) flush(id, q);
}
async function flush(id, q) {
  q.busy = true;
  while (q.buf) {
    const d = q.buf; q.buf = '';
    try { await invoke('send_data', { id, data: d }); } catch (e) { /* Sitzung weg */ }
  }
  q.busy = false;
  outQueues.delete(id);
}

export const ResizePTY = (id, cols, rows) => invoke('resize_pty', { id, cols, rows }).catch(() => {});

export const LoadHosts = () => invoke('load_hosts');
export const SaveHosts = (json) => invoke('save_hosts', { json });
export const LoadSSHKeys = () => invoke('load_ssh_keys');
export const SaveSSHKeys = (json) => invoke('save_ssh_keys', { json });
export const LoadSettings = () => invoke('load_settings').then(JSON.parse).catch(() => ({}));
export const SaveSettings = (obj) => invoke('save_settings', { json: JSON.stringify(obj, null, 2) }).catch(() => {});

export async function ImportSSHKeyFromFile() {
  const path = await open({ title: 'SSH Private Key auswählen', multiple: false, directory: false });
  if (!path) return '';
  try { return await invoke('read_local_text', { path }); } catch (e) { return err(e); }
}

export async function ListDirectory(id, path) {
  try { return await invoke('list_directory', { id, path }); } catch (e) { return '[]'; }
}
export async function ReadFile(id, path) {
  try { return await invoke('read_file', { id, path }); } catch (e) { return err(e); }
}
export async function WriteFile(id, path, content) {
  try { await invoke('write_file', { id, path, content }); return 'OK'; } catch (e) { return err(e); }
}
export async function DeleteFile(id, path) {
  try { await invoke('delete_file', { id, path }); return 'OK'; } catch (e) { return err(e); }
}
export async function RenameFile(id, oldPath, newPath) {
  try { await invoke('rename_file', { id, oldPath, newPath }); return 'OK'; } catch (e) { return err(e); }
}
export async function MakeDirectory(id, path) {
  try { await invoke('make_directory', { id, path }); return 'OK'; } catch (e) { return err(e); }
}

export async function UploadFile(id, remoteDir) {
  const localPath = await open({ title: 'Datei für Upload auswählen', multiple: false, directory: false });
  if (!localPath) return 'Abgebrochen';
  try { await invoke('upload_file', { id, remoteDir, localPath }); return 'OK'; } catch (e) { return err(e); }
}

export async function DownloadFile(id, remotePath) {
  const name = remotePath.split('/').pop();
  const localPath = await save({ title: 'Speicherort auswählen', defaultPath: name });
  if (!localPath) return 'Abgebrochen';
  try { await invoke('download_file', { id, remotePath, localPath }); return 'OK'; } catch (e) { return err(e); }
}
