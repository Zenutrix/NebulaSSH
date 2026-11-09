//! NebulaSSH – Tauri-Backend (Rust).

mod serial;
mod session;
mod ssh;
mod storage;

use session::{close_entry, Cmd, Entry, Sessions};
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

struct AppState {
    sessions: Sessions,
}

fn ssh_session(state: &AppState, id: &str) -> Result<Arc<ssh::SshSession>, String> {
    match state.sessions.lock().unwrap().get(id) {
        Some(Entry::Ssh(s)) => Ok(s.clone()),
        _ => Err("Keine aktive SFTP Sitzung".into()),
    }
}

// --- Verbindungen ----------------------------------------------------------

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn connect(
    state: State<'_, AppState>,
    id: String,
    ip: String,
    user: String,
    pass: String,
    private_key: String,
    cols: u32,
    rows: u32,
    on_data: Channel<InvokeResponseBody>,
    on_close: Channel<String>,
) -> Result<String, String> {
    ssh::connect(
        state.sessions.clone(),
        id,
        ip,
        user,
        pass,
        private_key,
        cols,
        rows,
        on_data,
        on_close,
    )
    .await
}

#[tauri::command]
async fn connect_serial(
    state: State<'_, AppState>,
    id: String,
    port_name: String,
    baud_rate: u32,
    on_data: Channel<InvokeResponseBody>,
    on_close: Channel<String>,
) -> Result<String, String> {
    let sessions = state.sessions.clone();
    tokio::task::spawn_blocking(move || {
        serial::connect(sessions, id, port_name, baud_rate, on_data, on_close)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_serial_ports() -> Vec<String> {
    tokio::task::spawn_blocking(serial::list_ports)
        .await
        .unwrap_or_default()
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>, id: String) {
    let entry = state.sessions.lock().unwrap().remove(&id);
    if let Some(e) = entry {
        close_entry(e);
    }
}

fn push(state: &AppState, id: &str, cmd: Cmd) {
    match state.sessions.lock().unwrap().get(id) {
        Some(Entry::Ssh(s)) => {
            let _ = s.tx.send(cmd);
        }
        Some(Entry::Serial(s)) => {
            let _ = s.tx.send(cmd);
        }
        _ => {}
    }
}

/// Synchroner Command: läuft in Aufrufreihenfolge, damit Tastendrücke nie vertauscht werden.
#[tauri::command]
fn send_data(state: State<'_, AppState>, id: String, data: String) {
    push(&state, &id, Cmd::Data(data.into_bytes()));
}

#[tauri::command]
fn resize_pty(state: State<'_, AppState>, id: String, cols: u32, rows: u32) {
    if cols > 0 && rows > 0 {
        push(&state, &id, Cmd::Resize(cols, rows));
    }
}

#[tauri::command]
fn forget_host_key(host_id: String) {
    ssh::forget_host_key(&host_id);
}

// --- Gespeicherte Daten ------------------------------------------------------

#[tauri::command]
async fn load_hosts() -> String {
    tokio::task::spawn_blocking(|| storage::load_json("hosts.json"))
        .await
        .unwrap_or_else(|_| "[]".into())
}

#[tauri::command]
async fn save_hosts(json: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || storage::save_json("hosts.json", &json))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn load_ssh_keys() -> String {
    tokio::task::spawn_blocking(|| storage::load_json("keys.json"))
        .await
        .unwrap_or_else(|_| "[]".into())
}

#[tauri::command]
async fn save_ssh_keys(json: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || storage::save_json("keys.json", &json))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Einstellungen (Makros, Baudraten) – nicht geheim, daher Klartext.
#[tauri::command]
fn load_settings() -> String {
    std::fs::read_to_string(storage::data_dir().join("settings.json"))
        .unwrap_or_else(|_| "{}".into())
}

#[tauri::command]
fn save_settings(json: String) -> Result<(), String> {
    std::fs::write(storage::data_dir().join("settings.json"), json).map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_local_text(path: String) -> Result<String, String> {
    let meta = tokio::fs::metadata(&path).await.map_err(|e| e.to_string())?;
    if meta.len() > 1024 * 1024 {
        return Err("Datei ist zu groß für einen SSH-Key".into());
    }
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| e.to_string())
}

// --- SFTP --------------------------------------------------------------------

#[tauri::command]
async fn list_directory(state: State<'_, AppState>, id: String, path: String) -> Result<String, String> {
    let s = ssh_session(&state, &id)?;
    ssh::list_directory(&s, &path).await
}

#[tauri::command]
async fn read_file(state: State<'_, AppState>, id: String, path: String) -> Result<String, String> {
    let s = ssh_session(&state, &id)?;
    ssh::read_file(&s, &path).await
}

#[tauri::command]
async fn write_file(
    state: State<'_, AppState>,
    id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let s = ssh_session(&state, &id)?;
    ssh::write_file(&s, &path, &content).await
}

#[tauri::command]
async fn delete_file(state: State<'_, AppState>, id: String, path: String) -> Result<(), String> {
    let s = ssh_session(&state, &id)?;
    ssh::delete(&s, &path).await
}

#[tauri::command]
async fn rename_file(
    state: State<'_, AppState>,
    id: String,
    old_path: String,
    new_path: String,
) -> Result<(), String> {
    let s = ssh_session(&state, &id)?;
    ssh::rename(&s, &old_path, &new_path).await
}

#[tauri::command]
async fn make_directory(state: State<'_, AppState>, id: String, path: String) -> Result<(), String> {
    let s = ssh_session(&state, &id)?;
    ssh::mkdir(&s, &path).await
}

#[tauri::command]
async fn upload_file(
    state: State<'_, AppState>,
    id: String,
    remote_dir: String,
    local_path: String,
) -> Result<(), String> {
    let s = ssh_session(&state, &id)?;
    ssh::upload(&s, &remote_dir, &local_path).await
}

#[tauri::command]
async fn download_file(
    state: State<'_, AppState>,
    id: String,
    remote_path: String,
    local_path: String,
) -> Result<(), String> {
    let s = ssh_session(&state, &id)?;
    ssh::download(&s, &remote_path, &local_path).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            sessions: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            connect_serial,
            get_serial_ports,
            disconnect,
            send_data,
            resize_pty,
            forget_host_key,
            load_hosts,
            save_hosts,
            load_ssh_keys,
            save_ssh_keys,
            load_settings,
            save_settings,
            read_local_text,
            list_directory,
            read_file,
            write_file,
            delete_file,
            rename_file,
            make_directory,
            upload_file,
            download_file,
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten von NebulaSSH");
}
