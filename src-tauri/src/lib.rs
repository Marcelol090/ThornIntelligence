mod health;
mod optimize;
mod scan;

use health::DiskHealth;
use optimize::OptimizationResult;
use scan::{ScanReport, ScanRequest};

#[tauri::command]
async fn scan_path(request: ScanRequest) -> Result<ScanReport, String> {
    tauri::async_runtime::spawn_blocking(move || scan::scan(request))
        .await
        .map_err(|err| format!("Falha interna da tarefa de varredura: {err}"))?
}

#[tauri::command]
async fn optimize_volume(drive: String, execute: bool) -> Result<OptimizationResult, String> {
    tauri::async_runtime::spawn_blocking(move || optimize::run(drive, execute))
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
        .invoke_handler(tauri::generate_handler![scan_path, optimize_volume, disk_health])
        .run(tauri::generate_context!())
        .expect("error while running Thorn Intelligence");
}
