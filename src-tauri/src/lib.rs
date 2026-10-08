mod health;
mod jobs;
mod index;
mod quarantine;
mod optimize;
mod scan;
mod search;

use health::DiskHealth;
use jobs::ScanJobs;
use index::{IndexStats, IndexedSearch};
use quarantine::{QuarantineGate, QuarantineItem, QuarantinePreview};
use optimize::OptimizationResult;
use scan::{ScanProgress, ScanReport, ScanRequest};
use search::{SearchReport, SearchRequest};
use std::sync::Arc;
use tauri::{ipc::Channel, AppHandle, Manager, State};

#[tauri::command]
async fn scan_path(
    request: ScanRequest,
    job_id: String,
    on_progress: Channel<ScanProgress>,
    jobs: State<'_, Arc<ScanJobs>>,
) -> Result<ScanReport, String> {
    let jobs = Arc::clone(jobs.inner());
    let token = jobs.start(job_id.clone())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        scan::scan_with_control(request, token.as_ref(), |event| {
            let _ = on_progress.send(event);
        })
    })
    .await
    .map_err(|err| format!("Falha interna da tarefa de varredura: {err}"))
    .and_then(|result| result);
    jobs.finish(&job_id);
    result
}

#[tauri::command]
async fn search_path(
    request: SearchRequest,
    job_id: String,
    on_progress: Channel<ScanProgress>,
    jobs: State<'_, Arc<ScanJobs>>,
) -> Result<SearchReport, String> {
    let jobs = Arc::clone(jobs.inner());
    let token = jobs.start(job_id.clone())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        search::search_with_control(request, token.as_ref(), |event| {
            let _ = on_progress.send(event);
        })
    })
    .await
    .map_err(|err| format!("Falha interna da busca por metadados: {err}"))
    .and_then(|result| result);
    jobs.finish(&job_id);
    result
}

#[tauri::command]
fn cancel_scan(job_id: String, jobs: State<'_, Arc<ScanJobs>>) -> Result<bool, String> {
    jobs.cancel(&job_id)
}

#[tauri::command]
async fn refresh_index(
    root: String,
    max_files: Option<usize>,
    job_id: String,
    on_progress: Channel<ScanProgress>,
    app: AppHandle,
    jobs: State<'_, Arc<ScanJobs>>,
) -> Result<IndexStats, String> {
    let app_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let jobs = Arc::clone(jobs.inner());
    let token = jobs.start(job_id.clone())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        index::refresh(
            &app_data.join("storage-index.sqlite"), &root, max_files,
            token.as_ref(), |event| { let _ = on_progress.send(event); }
        )
    }).await.map_err(|e| e.to_string()).and_then(|result| result);
    jobs.finish(&job_id);
    result
}

#[tauri::command]
async fn search_index(request: SearchRequest, app: AppHandle)
    -> Result<IndexedSearch, String> {
    let app_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        index::search_index(&app_data.join("storage-index.sqlite"), request)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn preview_quarantine(
    path: String, app: AppHandle, gate: State<'_, Arc<QuarantineGate>>,
) -> Result<QuarantinePreview, String> {
    let app_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let gate = Arc::clone(gate.inner());
    tauri::async_runtime::spawn_blocking(move || {
        quarantine::preview(path, &app_data, &gate)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn quarantine_file(
    preview_id: String, confirmation: String, app: AppHandle,
    gate: State<'_, Arc<QuarantineGate>>,
) -> Result<QuarantineItem, String> {
    let app_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let gate = Arc::clone(gate.inner());
    tauri::async_runtime::spawn_blocking(move || {
        quarantine::quarantine(preview_id, confirmation, &app_data, &gate)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_quarantine(app: AppHandle) -> Result<Vec<QuarantineItem>, String> {
    let app_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || quarantine::list(&app_data))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn restore_quarantine(
    id: String, confirmation: String, app: AppHandle,
) -> Result<QuarantineItem, String> {
    let app_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        quarantine::restore(id, confirmation, &app_data)
    }).await.map_err(|e| e.to_string())?
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
        .manage(Arc::new(ScanJobs::default()))
        .manage(Arc::new(QuarantineGate::default()))
        .invoke_handler(tauri::generate_handler![scan_path, search_path, cancel_scan, refresh_index, search_index,
            preview_quarantine, quarantine_file, list_quarantine, restore_quarantine,
            optimize_volume, disk_health])
        .run(tauri::generate_context!())
        .expect("error while running Thorn Intelligence");
}
