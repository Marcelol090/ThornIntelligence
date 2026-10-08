use blake3::Hasher;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use same_file::Handle;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

const MAX_HASH_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_RESULTS: usize = 300;
const MAX_MATCHES: usize = 500;
// Avoid exhausting OS handles when inspecting huge identical-content groups.
const MAX_IDENTITY_GROUP_FILES: usize = 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    pub root: String,
    pub regex: Option<String>,
    pub min_size_bytes: Option<u64>,
    pub max_files: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub extension: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryResult {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub files: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeResult {
    pub extension: String,
    pub size_bytes: u64,
    pub files: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub hash: String,
    pub size_bytes: u64,
    pub copies: Vec<String>,
    pub potential_savings_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub root: String,
    pub files_scanned: usize,
    pub directories_scanned: usize,
    pub logical_bytes: u64,
    pub elapsed_ms: u128,
    pub errors: usize,
    pub error_samples: Vec<String>,
    pub truncated: bool,
    pub duplicate_analysis_complete: bool,
    pub hardlink_aliases: usize,
    pub hash_bytes_read: u64,
    pub top_files: Vec<FileResult>,
    pub top_directories: Vec<DirectoryResult>,
    pub file_types: Vec<TypeResult>,
    pub duplicates: Vec<DuplicateGroup>,
    pub matches: Vec<FileResult>,
    pub total_matches: usize,
    pub potential_savings_bytes: u64,
}

#[derive(Clone)]
struct FileRecord {
    path: PathBuf,
    size: u64,
    result: FileResult,
}

#[derive(Default)]
struct DirectorySize {
    bytes: u64,
    files: u64,
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "sem extensão".into())
}

fn content_hash(path: &Path, expected_size: u64) -> std::io::Result<Option<String>> {
    let file = File::open(path)?;
    if file.metadata()?.len() != expected_size {
        return Ok(None);
    }
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Hasher::new();
    let mut buffer = [0u8; 65536];
    let mut count = 0_u64;
    loop {
        let bytes = reader.read(&mut buffer)?;
        if bytes == 0 { break; }
        count += bytes as u64;
        hasher.update(&buffer[..bytes]);
    }
    if count != expected_size { return Ok(None); }
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

fn prepare_regex(pattern: Option<&str>) -> Result<Option<Regex>, String> {
    match pattern.map(str::trim).filter(|value| !value.is_empty()) {
        Some(pattern) => RegexBuilder::new(pattern)
            .case_insensitive(true)
            .size_limit(4 * 1024 * 1024)
            .build()
            .map(Some)
            .map_err(|err| format!("Expressão regular inválida: {err}")),
        None => Ok(None),
    }
}

pub fn scan(request: ScanRequest) -> Result<ScanReport, String> {
    let timer = Instant::now();
    let root = std::fs::canonicalize(Path::new(&request.root))
        .map_err(|err| format!("Pasta não encontrada ou inacessível: {err}"))?;
    if !root.is_dir() {
        return Err("Selecione um diretório, não um arquivo.".into());
    }
    let pattern = prepare_regex(request.regex.as_deref())?;
    let min_size = request.min_size_bytes.unwrap_or(0);
    let max_files = request.max_files.unwrap_or(250_000).clamp(1, 1_000_000);
    let mut records = Vec::<FileRecord>::new();
    let mut directories = HashMap::<PathBuf, DirectorySize>::new();
    let mut types = HashMap::<String, (u64, u64)>::new();
    let mut matches = Vec::<FileResult>::new();
    let mut total_matches = 0_usize;
    let mut errors = 0_usize;
    let mut error_samples = Vec::new();
    let mut directories_scanned = 0_usize;
    let mut logical_bytes = 0_u64;
    let mut truncated = false;

    directories.insert(root.clone(), DirectorySize::default());
    for entry in WalkDir::new(&root).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(value) => value,
            Err(err) => {
                errors += 1;
                if error_samples.len() < 12 { error_samples.push(err.to_string()); }
                continue;
            }
        };
        if entry.file_type().is_symlink() { continue; }
        if entry.file_type().is_dir() {
            directories_scanned += 1;
            directories.entry(entry.path().to_path_buf()).or_default();
            continue;
        }
        if !entry.file_type().is_file() { continue; }
        if records.len() >= max_files {
            truncated = true;
            break;
        }
        let metadata = match entry.metadata() {
            Ok(value) => value,
            Err(err) => {
                errors += 1;
                if error_samples.len() < 12 { error_samples.push(err.to_string()); }
                continue;
            }
        };
        let size = metadata.len();
        let path = entry.path().to_path_buf();
        let file_type = extension(&path);
        let result = FileResult {
            name: name(&path),
            path: path.display().to_string(),
            size_bytes: size,
            extension: file_type.clone(),
        };
        logical_bytes = logical_bytes.saturating_add(size);
        let category = types.entry(file_type).or_default();
        category.0 = category.0.saturating_add(size);
        category.1 += 1;

        let mut parent = path.parent();
        while let Some(dir) = parent {
            if !dir.starts_with(&root) { break; }
            let tally = directories.entry(dir.to_path_buf()).or_default();
            tally.bytes = tally.bytes.saturating_add(size);
            tally.files += 1;
            if dir == root { break; }
            parent = dir.parent();
        }

        if size >= min_size
            && pattern.as_ref().is_none_or(|regex| regex.is_match(&result.path)) {
            total_matches += 1;
            matches.push(result.clone());
        }
        records.push(FileRecord { path, size, result });
    }

    let mut top_files: Vec<FileResult> = records.iter().map(|file| file.result.clone()).collect();
    top_files.sort_unstable_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    top_files.truncate(MAX_RESULTS);
    matches.sort_unstable_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    matches.truncate(MAX_MATCHES);

    let mut top_directories: Vec<DirectoryResult> = directories
        .into_iter()
        .filter(|(path, _)| path != &root)
        .map(|(path, size)| DirectoryResult {
            name: name(&path),
            path: path.display().to_string(),
            size_bytes: size.bytes,
            files: size.files,
        })
        .collect();
    top_directories.sort_unstable_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    top_directories.truncate(MAX_RESULTS);

    let mut file_types: Vec<TypeResult> = types.into_iter().map(|(extension, (size_bytes, files))| {
        TypeResult { extension, size_bytes, files }
    }).collect();
    file_types.sort_unstable_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    file_types.truncate(30);

    let mut size_buckets = HashMap::<u64, Vec<&FileRecord>>::new();
    for record in &records {
        // Empty files consume no content bytes and are not space-saving candidates.
        if record.size > 0 {
            size_buckets.entry(record.size).or_default().push(record);
        }
    }
    let mut candidate_buckets: Vec<_> = size_buckets.into_iter()
        .filter(|(_, files)| files.len() > 1)
        .collect();
    candidate_buckets.sort_unstable_by(|a, b| b.0.cmp(&a.0));

    let mut hashed_bytes = 0_u64;
    let mut duplicate_analysis_complete = true;
    let mut groups = HashMap::<(u64, String), Vec<String>>::new();
    for (size, bucket) in candidate_buckets {
        for record in bucket {
            if hashed_bytes.saturating_add(size) > MAX_HASH_BYTES {
                duplicate_analysis_complete = false;
                continue;
            }
            match content_hash(&record.path, record.size) {
                Ok(Some(hash)) => {
                    hashed_bytes = hashed_bytes.saturating_add(size);
                    groups.entry((size, hash)).or_default().push(record.result.path.clone());
                }
                Ok(None) => { errors += 1; }
                Err(err) => {
                    errors += 1;
                    if error_samples.len() < 12 {
                        error_samples.push(format!("{}: {err}", record.path.display()));
                    }
                }
            }
        }
    }

    let mut hardlink_aliases = 0_usize;
    let mut duplicates = Vec::<DuplicateGroup>::new();
    for ((size_bytes, hash), copies) in groups {
        if copies.len() < 2 {
            continue;
        }
        if copies.len() > MAX_IDENTITY_GROUP_FILES {
            // Don't open an unbounded number of handles. Missing results are
            // flagged rather than presenting an unsafe savings estimate.
            duplicate_analysis_complete = false;
            continue;
        }

        let mut unique_files = HashSet::<Handle>::new();
        let mut independently_allocated = Vec::<String>::new();
        let mut identity_verified = true;
        for path in copies {
            match Handle::from_path(&path) {
                Ok(handle) if unique_files.insert(handle) => {
                    independently_allocated.push(path);
                }
                Ok(_) => {
                    hardlink_aliases += 1;
                }
                Err(err) => {
                    errors += 1;
                    if error_samples.len() < 12 {
                        error_samples.push(format!("Identidade física {}: {err}", path));
                    }
                    identity_verified = false;
                    break;
                }
            }
        }
        if !identity_verified {
            duplicate_analysis_complete = false;
            continue;
        }
        if independently_allocated.len() < 2 {
            continue; // Hardlink aliases do not release bytes independently.
        }
        let potential_savings_bytes = size_bytes.saturating_mul(
            (independently_allocated.len() - 1) as u64
        );
        duplicates.push(DuplicateGroup {
            hash,
            size_bytes,
            copies: independently_allocated,
            potential_savings_bytes,
        });
    }
    duplicates.sort_unstable_by(|a, b| b.potential_savings_bytes.cmp(&a.potential_savings_bytes));
    let potential_savings_bytes = duplicates.iter()
        .fold(0_u64, |total, item| total.saturating_add(item.potential_savings_bytes));
    duplicates.truncate(MAX_RESULTS);

    Ok(ScanReport {
        root: root.display().to_string(),
        files_scanned: records.len(),
        directories_scanned,
        logical_bytes,
        elapsed_ms: timer.elapsed().as_millis(),
        errors,
        error_samples,
        truncated,
        duplicate_analysis_complete,
        hardlink_aliases,
        hash_bytes_read: hashed_bytes,
        top_files,
        top_directories,
        file_types,
        duplicates,
        matches,
        total_matches,
        potential_savings_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_sizes_and_verifies_exact_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.txt"), "matching payload").unwrap();
        fs::write(dir.path().join("sub/b.txt"), "matching payload").unwrap();
        fs::write(dir.path().join("other.log"), "different content").unwrap();
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: Some(".*\\.txt$".into()),
            min_size_bytes: Some(1),
            max_files: None,
        }).unwrap();
        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.total_matches, 2);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].copies.len(), 2);
        assert!(!report.truncated);
    }


    #[test]
    fn hardlinks_do_not_create_fictitious_recoverable_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.dat");
        let alias = dir.path().join("alias.dat");
        fs::write(&original, "same physical bytes").unwrap();
        fs::hard_link(&original, &alias).unwrap();

        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None,
            min_size_bytes: None,
            max_files: None,
        }).unwrap();

        assert_eq!(report.files_scanned, 2);
        assert_eq!(report.hardlink_aliases, 1);
        assert!(report.duplicates.is_empty());
        assert_eq!(report.potential_savings_bytes, 0);
        assert!(report.duplicate_analysis_complete);
    }

    #[test]
    fn hardlinks_and_independent_copies_are_not_confused() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.dat");
        fs::write(&original, "matching bytes").unwrap();
        fs::hard_link(&original, dir.path().join("alias.dat")).unwrap();
        fs::copy(&original, dir.path().join("copy.dat")).unwrap();

        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None,
            min_size_bytes: None,
            max_files: None,
        }).unwrap();

        assert_eq!(report.hardlink_aliases, 1);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].copies.len(), 2);
        assert_eq!(report.potential_savings_bytes, "matching bytes".len() as u64);
    }

    #[test]
    fn empty_files_are_excluded_from_space_savings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("empty_1"), "").unwrap();
        fs::write(dir.path().join("empty_2"), "").unwrap();
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None,
            min_size_bytes: None,
            max_files: None,
        }).unwrap();
        assert_eq!(report.files_scanned, 2);
        assert!(report.duplicates.is_empty());
        assert_eq!(report.potential_savings_bytes, 0);
    }

    #[test]
    fn rejects_invalid_regular_expression() {
        assert!(prepare_regex(Some("[")).is_err());
    }

    #[test]
    fn caps_scanned_files_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("1"), "one").unwrap();
        fs::write(dir.path().join("2"), "two").unwrap();
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: Some(1),
        }).unwrap();
        assert!(report.truncated);
        assert_eq!(report.files_scanned, 1);
    }
}
