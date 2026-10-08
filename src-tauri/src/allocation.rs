//! Read-only, on-demand logical vs allocated size measurement for the largest
//! files already shown in the explorer. Not a volume-wide allocation or
//! recoverable-space estimate. Never opens content or hashes cloud files.
use crate::scan::{content_status, ScanProgress};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const MAX_TARGETS: usize = 300;
const MAX_PATH_UNITS: usize = 32_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationTarget {
    pub path: String,
    pub expected_size_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationRequest {
    pub root: String,
    pub targets: Vec<AllocationTarget>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationItem {
    pub path: String,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationReport {
    pub items: Vec<AllocationItem>,
    pub measured: usize,
    pub skipped: usize,
    pub failed: usize,
    pub elapsed_ms: u128,
}

fn ensure_active(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Medição de espaço alocado cancelada.".into())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn allocated_bytes(path: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    use windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW;

    // Canonical paths on Windows normally use the verbatim \\?\ prefix,
    // permitting NTFS paths beyond MAX_PATH (up to the Win32 wide limit).
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.len() >= MAX_PATH_UNITS {
        return Err("Caminho excede o limite da API Win32.".into());
    }
    wide.push(0);
    let mut high = 0u32;
    unsafe {
        // INVALID_FILE_SIZE (0xffffffff) can also be a valid low DWORD.
        // SetLastError(0) makes that case distinguishable from failure.
        SetLastError(0);
        let low = GetCompressedFileSizeW(wide.as_ptr(), &mut high);
        if low == u32::MAX && GetLastError() != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok((u64::from(high) << 32) | u64::from(low))
    }
}

#[cfg(not(windows))]
fn allocated_bytes(_: &Path) -> Result<u64, String> {
    Err("A medição de alocação requer a API nativa do Windows.".into())
}

pub fn measure(
    request: AllocationRequest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> Result<AllocationReport, String> {
    let started = Instant::now();
    if request.targets.len() > MAX_TARGETS {
        return Err(format!("Selecione até {MAX_TARGETS} arquivos por medição."));
    }
    if request.targets.is_empty() {
        return Err("Selecione arquivos para medir.".into());
    }
    let root_path = Path::new(&request.root);
    let root_meta = std::fs::symlink_metadata(root_path)
        .map_err(|e| format!("Raiz indisponível: {e}"))?;
    if !root_meta.is_dir() || root_meta.file_type().is_symlink()
        || content_status(&root_meta) != "local"
    {
        return Err("Selecione uma raiz local normal.".into());
    }
    let root = std::fs::canonicalize(root_path)
        .map_err(|e| format!("Raiz inacessível: {e}"))?;

    let mut items = Vec::with_capacity(request.targets.len());
    let mut measured = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    progress(ScanProgress {
        phase: "allocation".into(), files_scanned: 0, hash_bytes_read: 0,
    });
    for (index, target) in request.targets.into_iter().enumerate() {
        ensure_active(cancel)?;
        let path = PathBuf::from(&target.path);
        let mut item = AllocationItem {
            path: target.path,
            logical_bytes: target.expected_size_bytes,
            allocated_bytes: None,
            status: "unsupported".into(),
        };
        if !path.is_absolute() {
            item.status = "outside".into();
            skipped += 1;
        } else {
            match std::fs::symlink_metadata(&path) {
                Err(_) => { item.status = "unavailable".into(); failed += 1; }
                Ok(meta) if !meta.file_type().is_file()
                    || meta.file_type().is_symlink()
                    || content_status(&meta) != "local" => {
                    item.status = "excluded".into();
                    skipped += 1;
                }
                Ok(meta) if meta.len() != item.logical_bytes => {
                    item.status = "changed".into();
                    skipped += 1;
                }
                Ok(_) => match std::fs::canonicalize(&path) {
                    Err(_) => { item.status = "unavailable".into(); failed += 1; }
                    Ok(canonical) if !canonical.starts_with(&root) => {
                        item.status = "outside".into();
                        skipped += 1;
                    }
                    Ok(canonical) => {
                        // Recheck the canonical target immediately before the
                        // Win32 metadata query. No file contents are read.
                        match std::fs::symlink_metadata(&canonical) {
                            Ok(now) if now.file_type().is_file()
                                && content_status(&now) == "local"
                                && now.len() == item.logical_bytes => {
                                match allocated_bytes(&canonical) {
                                    Ok(bytes) => {
                                        item.allocated_bytes = Some(bytes);
                                        item.status = "measured".into();
                                        measured += 1;
                                    }
                                    Err(_) => {
                                        item.status = "unavailable".into();
                                        failed += 1;
                                    }
                                }
                            }
                            _ => { item.status = "changed".into(); skipped += 1; }
                        }
                    }
                },
            }
        }
        items.push(item);
        if index % 10 == 0 || index + 1 == items.capacity() {
            progress(ScanProgress {
                phase: "allocation".into(),
                files_scanned: index + 1,
                hash_bytes_read: 0,
            });
        }
    }
    ensure_active(cancel)?;
    progress(ScanProgress {
        phase: "complete".into(), files_scanned: items.len(), hash_bytes_read: 0,
    });
    Ok(AllocationReport {
        items, measured, skipped, failed, elapsed_ms: started.elapsed().as_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.bin");
        fs::write(&file, vec![1u8; 8192]).unwrap();
        (dir, file)
    }

    #[test]
    fn does_not_open_content_and_reports_measured_size_on_windows() {
        let (dir, file) = fixture();
        let report = measure(AllocationRequest {
            root: dir.path().display().to_string(),
            targets: vec![AllocationTarget {
                path: file.display().to_string(), expected_size_bytes: 8192,
            }],
        }, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(report.items.len(), 1);
        #[cfg(windows)]
        {
            assert_eq!(report.measured, 1);
            assert_eq!(report.items[0].status, "measured");
            assert!(report.items[0].allocated_bytes.is_some());
        }
        #[cfg(not(windows))]
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn rejects_oversized_input_and_cancellation() {
        let (dir, file) = fixture();
        let make = || AllocationTarget {
            path: file.display().to_string(), expected_size_bytes: 8192,
        };
        assert!(measure(AllocationRequest {
            root: dir.path().display().to_string(),
            targets: (0..301).map(|_| make()).collect(),
        }, &AtomicBool::new(false), |_| {}).is_err());
        let cancel = AtomicBool::new(true);
        assert!(measure(AllocationRequest {
            root: dir.path().display().to_string(),
            targets: vec![make()],
        }, &cancel, |_| {}).unwrap_err().contains("cancelada"));
    }

    #[test]
    fn never_measures_outside_scope_or_stale_size() {
        let (dir, file) = fixture();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("outside.bin");
        fs::write(&outside, "secret").unwrap();
        let report = measure(AllocationRequest {
            root: dir.path().display().to_string(),
            targets: vec![
                AllocationTarget {
                    path: file.display().to_string(),
                    expected_size_bytes: 9999,
                },
                AllocationTarget {
                    path: outside.display().to_string(),
                    expected_size_bytes: 6,
                },
            ],
        }, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(report.measured, 0);
        assert_eq!(report.skipped, 2);
        assert!(report.items.iter().all(|i| i.allocated_bytes.is_none()));
    }
}
