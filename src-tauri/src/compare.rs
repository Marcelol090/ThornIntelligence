//! Read-only comparison: files under a candidate folder that have an exact
//! BLAKE3 match under a separate, explicitly selected reference folder.
//! A match is never an authorization to delete or move either file.
use crate::scan::{avoid_content_read, content_hash_with_progress, ScanProgress};
use same_file::Handle;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

const MAX_FILES_PER_SIDE: usize = 250_000;
const MAX_READ_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_DISPLAY_MATCHES: usize = 300;
const MAX_ERROR_SAMPLES: usize = 10;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareRequest {
    pub reference_root: String,
    pub candidate_root: String,
    pub max_files: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareMatch {
    pub candidate_path: String,
    pub reference_path: String,
    pub size_bytes: u64,
    pub hash: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareReport {
    pub reference_root: String,
    pub candidate_root: String,
    pub reference_files: usize,
    pub candidate_files: usize,
    pub matched_candidates: usize,
    pub matches: Vec<CompareMatch>,
    pub potential_logical_savings_bytes: u64,
    pub hash_bytes_read: u64,
    pub hash_stage_elapsed_ms: u128,
    pub skipped_cloud_files: usize,
    pub hardlink_aliases: usize,
    pub errors: usize,
    pub error_samples: Vec<String>,
    pub truncated: bool,
    pub complete: bool,
    pub elapsed_ms: u128,
}

struct FileRecord {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

#[derive(Default)]
struct Collection {
    files: Vec<FileRecord>,
    skipped_cloud_files: usize,
    errors: usize,
    error_samples: Vec<String>,
    truncated: bool,
}

fn ensure_active(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Comparação cancelada pelo usuário.".into())
    } else {
        Ok(())
    }
}

fn record_error(collection: &mut Collection, message: String) {
    collection.errors += 1;
    if collection.error_samples.len() < MAX_ERROR_SAMPLES {
        collection.error_samples.push(message);
    }
}

fn trusted_root(user_path: &str) -> Result<PathBuf, String> {
    // Refuse root symlinks and junctions *before* canonicalization.
    let requested = Path::new(user_path);
    let source = std::fs::symlink_metadata(requested)
        .map_err(|e| format!("Pasta indisponível: {e}"))?;
    if !source.is_dir() || source.file_type().is_symlink() || avoid_content_read(&source) {
        return Err("Selecione uma pasta local normal, não um link ou pasta virtual.".into());
    }
    std::fs::canonicalize(requested)
        .map_err(|e| format!("Não foi possível acessar a pasta: {e}"))
}

fn collect(
    root: &Path,
    max_files: usize,
    cancel: &AtomicBool,
    scanned_so_far: usize,
    mut on_progress: impl FnMut(usize),
) -> Result<Collection, String> {
    let mut result = Collection::default();
    let mut last_event = Instant::now();
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        ensure_active(cancel)?;
        let entry = match entry {
            Ok(v) => v,
            Err(e) => {
                record_error(&mut result, format!("Diretório inacessível: {e}"));
                continue;
            }
        };
        if entry.path() == root { continue; }
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(meta) => meta,
            Err(e) => {
                record_error(&mut result, format!("Metadados indisponíveis: {e}"));
                continue;
            }
        };
        if entry.file_type().is_symlink() || avoid_content_read(&meta) {
            if entry.file_type().is_dir() { walker.skip_current_dir(); }
            result.skipped_cloud_files += 1;
            continue;
        }
        if entry.file_type().is_dir() { continue; }
        if !entry.file_type().is_file() { continue; }
        if result.files.len() >= max_files {
            result.truncated = true;
            break;
        }
        if meta.len() > 0 {
            result.files.push(FileRecord {
                path: entry.path().to_path_buf(),
                size: meta.len(),
                modified: meta.modified().ok(),
            });
        }
        if last_event.elapsed() >= Duration::from_millis(250) {
            on_progress(scanned_so_far + result.files.len());
            last_event = Instant::now();
        }
    }
    Ok(result)
}

fn stable_file(file: &FileRecord) -> bool {
    std::fs::symlink_metadata(&file.path).is_ok_and(|meta| {
        meta.is_file()
            && !meta.file_type().is_symlink()
            && !avoid_content_read(&meta)
            && meta.len() == file.size
            && meta.modified().ok() == file.modified
    })
}

pub fn compare_with_control(
    request: CompareRequest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> Result<CompareReport, String> {
    let start = Instant::now();
    let reference = trusted_root(&request.reference_root)?;
    let candidate = trusted_root(&request.candidate_root)?;
    // Overlapping trees would make the same file appear on both sides.
    if reference == candidate || reference.starts_with(&candidate) || candidate.starts_with(&reference) {
        return Err("As pastas mestre e candidata não podem ser iguais nem sobrepostas.".into());
    }

    let max_files = request.max_files.unwrap_or(MAX_FILES_PER_SIDE)
        .clamp(1, MAX_FILES_PER_SIDE);
    progress(ScanProgress { phase: "comparing".into(), files_scanned: 0, hash_bytes_read: 0 });
    let master = collect(&reference, max_files, cancel, 0, |files_scanned| {
        progress(ScanProgress { phase: "comparing".into(), files_scanned, hash_bytes_read: 0 });
    })?;
    let candidate_side = collect(&candidate, max_files, cancel, master.files.len(), |files_scanned| {
        progress(ScanProgress { phase: "comparing".into(), files_scanned, hash_bytes_read: 0 });
    })?;

    let mut errors = master.errors + candidate_side.errors;
    let mut error_samples = master.error_samples;
    let sample_slots = MAX_ERROR_SAMPLES.saturating_sub(error_samples.len());
    error_samples.extend(candidate_side.error_samples.into_iter().take(sample_slots));
    let mut complete = !master.truncated && !candidate_side.truncated
        && errors == 0 && master.skipped_cloud_files == 0
        && candidate_side.skipped_cloud_files == 0;
    let skipped_cloud_files = master.skipped_cloud_files + candidate_side.skipped_cloud_files;
    let mut hash_bytes_read = 0u64;
    let mut hardlink_aliases = 0usize;
    let mut matched_candidates = 0usize;
    let mut savings = 0u64;
    let mut results = BTreeMap::<(u64,String), CompareMatch>::new();
    let total_files = master.files.len() + candidate_side.files.len();
    let mut last_event = Instant::now();

    let hash_started = Instant::now();
    // A candidate must have the same length as at least one reference.
    let candidate_sizes: HashSet<u64> = candidate_side.files.iter().map(|f| f.size).collect();
    let mut master_files: Vec<&FileRecord> = master.files.iter()
        .filter(|f| candidate_sizes.contains(&f.size)).collect();
    master_files.sort_unstable_by(|a, b| a.size.cmp(&b.size).then(a.path.cmp(&b.path)));

    // The key is both size and BLAKE3 hash. Retain references, never just
    // paths of arbitrary identical-size candidates.
    let mut known = HashMap::<(u64, String), Vec<&FileRecord>>::new();
    progress(ScanProgress {
        phase: "hashing".into(), files_scanned: total_files, hash_bytes_read,
    });
    for file in master_files {
        ensure_active(cancel)?;
        if !stable_file(file) {
            complete = false;
            errors += 1;
            if error_samples.len() < MAX_ERROR_SAMPLES {
                error_samples.push(format!("Referência alterada/inacessível: {}", file.path.display()));
            }
            continue;
        }
        if hash_bytes_read.saturating_add(file.size) > MAX_READ_BYTES {
            complete = false;
            continue;
        }
        let previous_bytes = hash_bytes_read;
        // During large files, emit progress from inside the sequential reader.
        let (digest, consumed) = content_hash_with_progress(
            &file.path, file.size, cancel, |current| {
                if last_event.elapsed() >= Duration::from_millis(250) {
                    progress(ScanProgress {
                        phase: "hashing".into(), files_scanned: total_files,
                        hash_bytes_read: previous_bytes.saturating_add(current),
                    });
                    last_event = Instant::now();
                }
            },
        );
        hash_bytes_read = hash_bytes_read.saturating_add(consumed);
        ensure_active(cancel)?;
        match digest {
            Ok(Some(hash)) => { known.entry((file.size, hash)).or_default().push(file); }
            _ => {
                errors += 1;
                complete = false;
                if error_samples.len() < MAX_ERROR_SAMPLES {
                    error_samples.push(format!("Não foi possível conferir: {}", file.path.display()));
                }
            }
        }
        if last_event.elapsed() >= Duration::from_millis(250) {
            progress(ScanProgress {
                phase: "hashing".into(), files_scanned: total_files, hash_bytes_read,
            });
            last_event = Instant::now();
        }
    }

    let interesting_sizes: HashSet<u64> = known.keys().map(|(size, _)| *size).collect();
    let mut candidates: Vec<&FileRecord> = candidate_side.files.iter()
        .filter(|f| interesting_sizes.contains(&f.size)).collect();
    candidates.sort_unstable_by(|a, b| a.size.cmp(&b.size).then(a.path.cmp(&b.path)));
    let mut counted_handles = HashSet::<Handle>::new();

    for file in candidates {
        ensure_active(cancel)?;
        if !stable_file(file) {
            complete = false;
            errors += 1;
            if error_samples.len() < MAX_ERROR_SAMPLES {
                error_samples.push(format!("Candidato alterado/inacessível: {}", file.path.display()));
            }
            continue;
        }
        if hash_bytes_read.saturating_add(file.size) > MAX_READ_BYTES {
            complete = false;
            continue;
        }
        let previous_bytes = hash_bytes_read;
        // During large files, emit progress from inside the sequential reader.
        let (digest, consumed) = content_hash_with_progress(
            &file.path, file.size, cancel, |current| {
                if last_event.elapsed() >= Duration::from_millis(250) {
                    progress(ScanProgress {
                        phase: "hashing".into(), files_scanned: total_files,
                        hash_bytes_read: previous_bytes.saturating_add(current),
                    });
                    last_event = Instant::now();
                }
            },
        );
        hash_bytes_read = hash_bytes_read.saturating_add(consumed);
        ensure_active(cancel)?;
        let digest = match digest {
            Ok(Some(hash)) => hash,
            _ => {
                errors += 1;
                complete = false;
                if error_samples.len() < MAX_ERROR_SAMPLES {
                    error_samples.push(format!("Não foi possível conferir: {}", file.path.display()));
                }
                continue;
            }
        };
        let Some(refs) = known.get(&(file.size, digest.clone())) else { continue; };
        // Handles must be checked AFTER hash and metadata verification.
        // An error here is not evidence of independent physical allocation.
        if !stable_file(file) {
            complete = false;
            errors += 1;
            continue;
        }
        let candidate_handle = match Handle::from_path(&file.path) {
            Ok(h) => h,
            Err(_) => { complete = false; errors += 1; continue; }
        };
        if counted_handles.contains(&candidate_handle) {
            hardlink_aliases += 1;
            continue;
        }
        let mut independent_reference = None;
        let mut identity_error = false;
        let mut same_physical_reference = false;
        for reference_file in refs {
            if !stable_file(reference_file) { identity_error = true; break; }
            match Handle::from_path(&reference_file.path) {
                Ok(h) if h == candidate_handle => {
                    // A hardlink to ANY master entry must never be counted,
                    // even if another independent master copy exists.
                    same_physical_reference = true;
                    break;
                }
                Ok(_) => {
                    if independent_reference.is_none() {
                        independent_reference = Some(reference_file.path.display().to_string());
                    }
                }
                Err(_) => { identity_error = true; break; }
            }
        }
        if identity_error {
            complete = false;
            errors += 1;
            continue;
        }
        counted_handles.insert(candidate_handle);
        if same_physical_reference {
            hardlink_aliases += 1;
            continue;
        }
        if let Some(reference_path) = independent_reference {
            matched_candidates += 1;
            savings = savings.saturating_add(file.size);
            let candidate_path = file.path.display().to_string();
            results.insert((file.size, candidate_path.clone()), CompareMatch {
                candidate_path, reference_path, size_bytes: file.size, hash: digest,
            });
            if results.len() > MAX_DISPLAY_MATCHES { results.pop_first(); }
        } else { hardlink_aliases += 1; }
        if last_event.elapsed() >= Duration::from_millis(250) {
            progress(ScanProgress {
                phase: "hashing".into(), files_scanned: total_files, hash_bytes_read,
            });
            last_event = Instant::now();
        }
    }
    ensure_active(cancel)?;
    let hash_stage_elapsed_ms = hash_started.elapsed().as_millis();
    let results = results.into_iter().rev().map(|(_, item)| item).collect();
    progress(ScanProgress {
        phase: "complete".into(), files_scanned: total_files, hash_bytes_read,
    });
    Ok(CompareReport {
        reference_root: reference.display().to_string(),
        candidate_root: candidate.display().to_string(),
        reference_files: master.files.len(),
        candidate_files: candidate_side.files.len(),
        matched_candidates,
        matches: results,
        potential_logical_savings_bytes: savings,
        hash_bytes_read,
        hash_stage_elapsed_ms,
        skipped_cloud_files,
        hardlink_aliases,
        errors,
        error_samples,
        truncated: master.truncated || candidate_side.truncated,
        complete,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn pair() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let master = tmp.path().join("master");
        let candidate = tmp.path().join("candidate");
        fs::create_dir(&master).unwrap();
        fs::create_dir(&candidate).unwrap();
        (tmp, master, candidate)
    }
    fn request(master: &Path, candidate: &Path) -> CompareRequest {
        CompareRequest {
            reference_root: master.display().to_string(),
            candidate_root: candidate.display().to_string(),
            max_files: Some(100),
        }
    }
    #[test]
    fn exact_matches_only_in_candidate_folder() {
        let (_tmp, master, candidate) = pair();
        fs::write(master.join("a.bin"), b"identical").unwrap();
        fs::write(candidate.join("copy1.bin"), b"identical").unwrap();
        fs::write(candidate.join("copy2.bin"), b"identical").unwrap();
        fs::write(candidate.join("same-size.bin"), b"different").unwrap();
        let result = compare_with_control(request(&master,&candidate),
            &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(result.matched_candidates, 2);
        assert_eq!(result.potential_logical_savings_bytes, 18);
        assert!(result.complete);
        assert!(result.matches.iter().all(|item|
            item.reference_path.ends_with("a.bin")));
    }
    #[test]
    fn identical_file_linked_across_folders_is_not_a_second_copy() {
        let (_tmp, master, candidate) = pair();
        let original = master.join("a.txt");
        fs::write(&original, "content").unwrap();
        fs::hard_link(&original, candidate.join("linked.txt")).unwrap();
        let result = compare_with_control(request(&master,&candidate),
            &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(result.matched_candidates, 0);
        assert_eq!(result.hardlink_aliases, 1);
    }
    #[test]
    fn master_hardlink_with_second_independent_master_copy_is_never_savings() {
        let (_tmp, master, candidate) = pair();
        fs::write(master.join("a"), b"original").unwrap();
        fs::write(master.join("b"), b"original").unwrap();
        fs::hard_link(master.join("a"), candidate.join("alias")).unwrap();
        let report = compare_with_control(request(&master, &candidate),
            &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(report.matched_candidates, 0);
        assert_eq!(report.hardlink_aliases, 1);
    }
    #[test]
    fn overlapping_roots_and_pre_cancellation_are_rejected() {
        let (_tmp, master, candidate) = pair();
        assert!(compare_with_control(request(&master,&master),
            &AtomicBool::new(false), |_| {}).is_err());
        let token = AtomicBool::new(true);
        assert!(compare_with_control(request(&master,&candidate),
            &token, |_| {}).unwrap_err().contains("cancelada"));
    }
    #[test]
    fn cancellation_during_hash_yields_no_partial_report() {
        let (_tmp, master, candidate) = pair();
        fs::write(master.join("m"), vec![1u8; 8192]).unwrap();
        fs::write(candidate.join("c"), vec![1u8; 8192]).unwrap();
        let cancel = AtomicBool::new(false);
        let result = compare_with_control(request(&master,&candidate), &cancel, |progress| {
            if progress.phase == "hashing" { cancel.store(true, Ordering::Relaxed); }
        });
        assert!(result.unwrap_err().contains("cancelada"));
    }
    #[test]
    fn changed_or_missing_candidates_are_not_invented() {
        let (_tmp, master, candidate) = pair();
        fs::write(master.join("keep"), b"one").unwrap();
        fs::write(candidate.join("unrelated"), b"two").unwrap();
        let result = compare_with_control(request(&master,&candidate),
            &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(result.matched_candidates, 0);
    }
}
