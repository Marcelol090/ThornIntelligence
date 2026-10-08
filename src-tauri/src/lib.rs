mod compare;
mod health;
mod jobs;
mod optimize;
mod scan;
mod search;

use compare::{CompareReport, CompareRequest};
use health::DiskHealth;
use jobs::ScanJobs;
use optimize::OptimizationResult;
use scan::{ScanProgress, ScanReport, ScanRequest};
use search::{SearchReport, SearchRequest};
use std::sync::Arc;
use tauri::{ipc::Channel, State};

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
async fn compare_folders(
    request: CompareRequest,
    job_id: String,
    on_progress: Channel<ScanProgress>,
    jobs: State<'_, Arc<ScanJobs>>,
) -> Result<CompareReport, String> {
    let jobs = Arc::clone(jobs.inner());
    let token = jobs.start(job_id.clone())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        compare::compare_with_control(request, token.as_ref(), |event| {
            let _ = on_progress.send(event);
        })
    })
    .await
    .map_err(|err| format!("Falha interna ao comparar as pastas: {err}"))
    .and_then(|result| result);
    jobs.finish(&job_id);
    result
}

#[tauri::command]
fn cancel_scan(job_id: String, jobs: State<'_, Arc<ScanJobs>>) -> Result<bool, String> {
    jobs.cancel(&job_id)
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
        .invoke_handler(tauri::generate_handler![scan_path, search_path, compare_folders, cancel_scan, optimize_volume, disk_health])
        .run(tauri::generate_context!())
        .expect("error while running Thorn Intelligence");
}
