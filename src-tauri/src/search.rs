use crate::scan::{FileResult, ScanProgress};
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub root: String,
    pub regex: Option<String>,
    pub min_size_bytes: Option<u64>,
    pub max_files: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchReport {
    pub root: String,
    pub files_scanned: usize,
    pub total_matches: usize,
    pub matches: Vec<FileResult>,
    pub elapsed_ms: u128,
    pub errors: usize,
    pub truncated: bool,
}

pub fn search(request: SearchRequest) -> Result<SearchReport, String> {
    search_with_control(request, &AtomicBool::new(false), |_| {})
}

pub fn search_with_control(
    request: SearchRequest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> Result<SearchReport, String> {
    let started = Instant::now();
    let mut last_update = Instant::now();
    progress(ScanProgress { phase: "searching".into(), files_scanned: 0, hash_bytes_read: 0 });
    let root = std::fs::canonicalize(&request.root)
        .map_err(|e| format!("Pasta inacessível: {e}"))?;
    if !root.is_dir() { return Err("Selecione uma pasta.".into()); }
    let regex = request.regex.as_deref().map(str::trim).filter(|s| !s.is_empty())
        .map(|p| {
            if p.len() > 4096 {
                return Err("Regex excede o limite de 4096 bytes.".to_owned());
            }
            RegexBuilder::new(p).case_insensitive(true).size_limit(4 * 1024 * 1024)
                .build().map_err(|e| format!("Regex inválida: {e}"))
        }).transpose()?;
    let max_files = request.max_files.unwrap_or(250_000).clamp(1, 1_000_000);
    let min_size = request.min_size_bytes.unwrap_or(0);
    let mut top = BTreeMap::<(u64, String), FileResult>::new();
    let (mut files_scanned, mut total_matches, mut errors) = (0, 0, 0);
    let mut truncated = false;
    for entry in WalkDir::new(&root).follow_links(false) {
        if cancel.load(Ordering::Relaxed) {
            return Err("Operação cancelada pelo usuário.".into());
        }
        let entry = match entry { Ok(e) => e, Err(_) => { errors += 1; continue; } };
        if !entry.file_type().is_file() { continue; }
        if files_scanned >= max_files { truncated = true; break; }
        let metadata = match entry.metadata() {
            Ok(m) => m, Err(_) => { errors += 1; continue; }
        };
        files_scanned += 1;
        if last_update.elapsed() >= Duration::from_millis(250) {
            progress(ScanProgress {
                phase: "searching".into(),
                files_scanned,
                hash_bytes_read: 0,
            });
            last_update = Instant::now();
        }
        let size = metadata.len();
        if size < min_size { continue; }
        let path = entry.path().to_string_lossy().into_owned();
        if regex.as_ref().is_some_and(|re| !re.is_match(&path)) { continue; }
        total_matches += 1;
        let extension = entry.path().extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .filter(|e| !e.is_empty()).unwrap_or_else(|| "sem extensão".into());
        top.insert((size, path.clone()), FileResult {
            name: entry.file_name().to_string_lossy().into_owned(),
            path, size_bytes: size, extension,
        });
        if top.len() > 500 { top.pop_first(); }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Operação cancelada pelo usuário.".into());
    }
    progress(ScanProgress {
        phase: "complete".into(),
        files_scanned,
        hash_bytes_read: 0,
    });
    Ok(SearchReport {
        root: root.display().to_string(), files_scanned, total_matches,
        matches: top.into_iter().rev().map(|(_, f)| f).collect(),
        elapsed_ms: started.elapsed().as_millis(), errors, truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn search_can_be_cancelled_without_partially_publishing_results() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("sample.bin"), b"data").unwrap();
        let cancelled = AtomicBool::new(true);
        let result = search_with_control(SearchRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None,
        }, &cancelled, |_| {});
        assert!(result.unwrap_err().contains("cancelada"));
    }

    #[test]
    fn search_filters_metadata_and_preserves_order() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("big.iso"), vec![1u8; 80]).unwrap();
        fs::write(dir.path().join("small.iso"), vec![1u8; 10]).unwrap();
        fs::write(dir.path().join("other.txt"), vec![1u8; 100]).unwrap();
        let report = search(SearchRequest {
            root: dir.path().display().to_string(),
            regex: Some(String::from("iso$")),
            min_size_bytes: Some(20),
            max_files: None,
        }).unwrap();
        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.total_matches, 1);
        assert_eq!(report.matches[0].name, "big.iso");
    }

    #[test]
    fn search_rejects_bad_regex_and_marks_partial_scans() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("one"), "1").unwrap();
        fs::write(dir.path().join("two"), "2").unwrap();
        let req = SearchRequest {
            root: dir.path().display().to_string(),
            regex: Some(String::from("[")),
            min_size_bytes: None,
            max_files: None,
        };
        assert!(search(req).is_err());
        assert!(search(SearchRequest {
            root: dir.path().display().to_string(),
            regex: Some("a".repeat(4097)),
            min_size_bytes: None,
            max_files: None,
        }).is_err());
        let report = search(SearchRequest {
            root: dir.path().display().to_string(),
            regex: None,
            min_size_bytes: None,
            max_files: Some(1),
        }).unwrap();
        assert_eq!(report.files_scanned, 1);
        assert!(report.truncated);
    }
}
