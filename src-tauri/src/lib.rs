mod health;
mod optimize;
mod scan;
mod search;

use health::DiskHealth;
use optimize::OptimizationResult;
use scan::{ScanReport, ScanRequest};
use search::{SearchReport, SearchRequest};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
async fn scan_path(request: ScanRequest) -> Result<ScanReport, String> {
    tauri::async_runtime::spawn_blocking(move || scan::scan(request))
        .await
        .map_err(|err| format!("Falha interna da tarefa de varredura: {err}"))?
}

#[tauri::command]
async fn search_path(request: SearchRequest) -> Result<SearchReport, String> {
    tauri::async_runtime::spawn_blocking(move || search::search(request))
        .await
        .map_err(|err| format!("Falha interna da busca por metadados: {err}"))?
}

#[tauri::command]
async fn optimize_volume(
    drive: String,
    execute: bool,
    gate: State<'_, Arc<optimize::OptimizationGate>>,
) -> Result<OptimizationResult, String> {
    let gate = Arc::clone(gate.inner());
    tauri::async_runtime::spawn_blocking(move || optimize::run(drive, execute, &gate))
        .await
        .map_err(|err| format!("Falha interna da tarefa de otimização: {err}"))?
}

#[tauri::command]
async fn disk_health(drive: String) -> Result<DiskHealth, String> {
    tauri::async_runtime::spawn_blocking(move || health::read(drive))
        .await
        .map_err(|err| format!("Falha interna ao consultar o disco: {err}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(optimize::OptimizationGate::default()))
        .invoke_handler(tauri::generate_handler![scan_path, search_path, optimize_volume, disk_health])
        .run(tauri::generate_context!())
        .expect("error while running Thorn Intelligence");
}
