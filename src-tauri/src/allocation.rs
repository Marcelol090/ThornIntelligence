//! Read-only, on-demand logical vs allocated size measurement for the largest
//! files already shown in the explorer. Not a volume-wide allocation or
//! recoverable-space estimate. Never opens content or hashes cloud files.
use crate::scan::{content_status, ScanProgress};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const MAX_TARGETS: usize = 300;

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
    /// Number of paths pointing to the same file on this volume (may include outside-root aliases).
    pub hardlink_count: Option<u32>,
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
fn allocation_metadata(path: &Path, expected_size: u64) -> Result<Option<(u64, u32)>, String> {
    use std::mem::{size_of, MaybeUninit};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandleEx, FileStandardInfo, FILE_STANDARD_INFO,
        FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    // Query metadata via a stable handle. No read permission, content I/O,
    // icon extraction, or fallback that could hydrate an offline placeholder.
    // NO_RECALL is a hint to providers, not a guarantee of their behavior.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL)
        .open(path)
        .map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || opened.file_type().is_symlink()
        || content_status(&opened) != "local" || opened.len() != expected_size
    {
        return Ok(None);
    }

    let mut raw = MaybeUninit::<FILE_STANDARD_INFO>::zeroed();
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileStandardInfo,
            raw.as_mut_ptr().cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let info = unsafe { raw.assume_init() };
    if info.Directory != 0 || info.DeletePending != 0
        || info.EndOfFile < 0 || info.EndOfFile as u64 != expected_size
        || info.AllocationSize < 0 || info.NumberOfLinks == 0
    {
        return Ok(None);
    }

    // Both fields come from the same file handle and cannot be mistaken
    // for independently recoverable capacity when hardlinks are present.
    Ok(Some((info.AllocationSize as u64, info.NumberOfLinks)))
}

#[cfg(not(windows))]
fn allocation_metadata(_: &Path, _: u64) -> Result<Option<(u64, u32)>, String> {
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
            hardlink_count: None,
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
                                match allocation_metadata(&canonical, item.logical_bytes) {
                                    Ok(Some((bytes, links))) => {
                                        item.allocated_bytes = Some(bytes);
                                        item.hardlink_count = Some(links);
                                        item.status = "measured".into();
                                        measured += 1;
                                    }
                                    Ok(None) => {
                                        item.status = "changed".into();
                                        skipped += 1;
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
            assert_eq!(report.items[0].hardlink_count, Some(1));
        }
        #[cfg(not(windows))]
        assert_eq!(report.failed, 1);
    }

    #[cfg(windows)]
    #[test]
    fn hardlinked_paths_share_allocation_and_expose_link_count() {
        let (dir, file) = fixture();
        let alias = dir.path().join("alias.bin");
        fs::hard_link(&file, &alias).unwrap();
        let report = measure(AllocationRequest {
            root: dir.path().display().to_string(),
            targets: vec![
                AllocationTarget {
                    path: file.display().to_string(),
                    expected_size_bytes: 8192,
                },
                AllocationTarget {
                    path: alias.display().to_string(),
                    expected_size_bytes: 8192,
                },
            ],
        }, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(report.measured, 2);
        assert_eq!(report.items[0].allocated_bytes, report.items[1].allocated_bytes);
        assert!(report.items.iter().all(|item| item.hardlink_count.unwrap_or(0) >= 2));
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
