use blake3::Hasher;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use same_file::Handle;
use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub phase: String,
    pub files_scanned: usize,
    pub hash_bytes_read: u64,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Operação cancelada pelo usuário.".into())
    } else {
        Ok(())
    }
}

const MAX_HASH_BYTES: u64 = 8 * 1024 * 1024 * 1024;
// Cheap content sampling never certifies a duplicate; only a full BLAKE3 does.
const PREFIX_BYTES: usize = 16 * 1024;
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
    /// Optional explicit sampling limit; None scans every accessible file.
    pub max_files: Option<usize>,
    /// Defaults to true for older callers. Frontend now defaults to quick metadata scan.
    #[serde(default)]
    pub analyze_duplicates: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub extension: String,
    /// Metadata only. Does not open or download a cloud placeholder.
    pub content_status: String,
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
    pub hashing_skipped: bool,
    pub hardlink_aliases: usize,
    /// Candidates skipped to avoid opening cloud/offline/reparse content.
    pub skipped_content_files: usize,
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
    hash_eligible: bool,
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

/// Attribute-only classification. Reparse points are not necessarily OneDrive;
/// never assume their content is locally readable.
#[cfg(windows)]
pub(crate) fn content_status(meta: &std::fs::Metadata) -> &'static str {
    use std::os::windows::fs::MetadataExt;
    let flags = meta.file_attributes();
    if flags & (0x1000 | 0x40000 | 0x400000) != 0 { "offline" }
    else if flags & 0x400 != 0 { "reparse" }
    else { "local" }
}
#[cfg(not(windows))]
pub(crate) fn content_status(_: &std::fs::Metadata) -> &'static str { "local" }

fn extension(path: &Path) -> String {
    path.extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "sem extensão".into())
}

// On Windows these flags identify offline, recall-on-access and reparse
// content. Skip all reparse files conservatively: opening a cloud placeholder
// may hydrate gigabytes even when the UI is only browsing metadata.
#[cfg(windows)]
fn unsafe_content_attributes(attributes: u32) -> bool {
    const REPARSE_POINT: u32 = 0x0000_0400;
    const OFFLINE: u32 = 0x0000_1000;
    const RECALL_ON_OPEN: u32 = 0x0004_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    attributes & (REPARSE_POINT | OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(windows)]
pub(crate) fn avoid_content_read(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    unsafe_content_attributes(metadata.file_attributes())
}

#[cfg(not(windows))]
pub(crate) fn avoid_content_read(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Returns both the digest outcome and the count of content bytes actually
/// returned by successful read() calls. Error/changed-file paths retain their
/// I/O cost so repeated failures cannot silently bypass the scan budget.
///
/// OS caches and filesystem read-ahead may cause physical device I/O to differ;
/// this counter deliberately measures successful application-level read bytes.
#[cfg(test)]
fn content_hash(path: &Path, expected_size: u64) -> (std::io::Result<Option<String>>, u64) {
    content_hash_with_cancel(path, expected_size, &AtomicBool::new(false))
}

pub(crate) fn content_hash_with_cancel(
    path: &Path,
    expected_size: u64,
    cancel: &AtomicBool,
) -> (std::io::Result<Option<String>>, u64) {
    let mut bytes_read = 0u64;
    let result = (|| -> std::io::Result<Option<String>> {
        let before = std::fs::symlink_metadata(path)?;
        if before.file_type().is_symlink()
            || avoid_content_read(&before)
            || before.len() != expected_size
        {
            return Ok(None);
        }
        let modified = before.modified()?;
        let mut file = File::open(path)?;
        let opened = file.metadata()?;
        if opened.len() != expected_size
            || opened.modified()? != modified
            || avoid_content_read(&opened)
        {
            return Ok(None);
        }

        let mut hasher = Hasher::new();
        let mut buffer = [0u8; 65536];
        while bytes_read < expected_size {
            if cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Operação cancelada pelo usuário.",
                ));
            }
            // Never read beyond the originally measured size. A file growing
            // concurrently must not consume an unbounded I/O budget.
            let remaining = expected_size - bytes_read;
            let limit = (remaining.min(buffer.len() as u64)) as usize;
            let count = file.read(&mut buffer[..limit])?;
            if count == 0 {
                return Ok(None);
            }
            bytes_read += count as u64;
            hasher.update(&buffer[..count]);
        }

        let after = file.metadata()?;
        if after.len() != expected_size
            || after.modified()? != modified
            || avoid_content_read(&after)
        {
            return Ok(None);
        }
        let path_after = std::fs::symlink_metadata(path)?;
        if path_after.file_type().is_symlink()
            || avoid_content_read(&path_after)
            || path_after.len() != expected_size
            || path_after.modified()? != modified
        {
            return Ok(None);
        }
        Ok(Some(hasher.finalize().to_hex().to_string()))
    })();
    (result, bytes_read)
}

/// A bounded prefix fingerprint for the first PREFIX_BYTES only.
/// Errors and partial reads are accounted just like a full BLAKE3 pass.
/// The prefix is exclusively a rejection filter, never duplicate evidence.
fn fingerprint_prefix(
    path: &Path, expected_size: u64, cancel: &AtomicBool,
) -> (std::io::Result<Option<String>>, u64) {
    let mut bytes_read = 0u64;
    let result = (|| -> std::io::Result<Option<String>> {
        let before = std::fs::symlink_metadata(path)?;
        if before.file_type().is_symlink()
            || avoid_content_read(&before)
            || before.len() != expected_size {
            return Ok(None);
        }
        let modified = before.modified()?;
        let mut file = File::open(path)?;
        let opened = file.metadata()?;
        if avoid_content_read(&opened)
            || opened.len() != expected_size
            || opened.modified()? != modified {
            return Ok(None);
        }
        let mut hasher = Hasher::new();
        let to_read = expected_size.min(PREFIX_BYTES as u64);
        let mut buffer = [0u8; 8192];
        while bytes_read < to_read {
            if cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted, "Operação cancelada pelo usuário.",
                ));
            }
            let remaining = (to_read - bytes_read).min(buffer.len() as u64) as usize;
            let amount = file.read(&mut buffer[..remaining])?;
            if amount == 0 { return Ok(None); }
            hasher.update(&buffer[..amount]);
            bytes_read += amount as u64;
        }
        let after = file.metadata()?;
        let path_after = std::fs::symlink_metadata(path)?;
        if avoid_content_read(&after)
            || avoid_content_read(&path_after)
            || path_after.file_type().is_symlink()
            || after.len() != expected_size
            || path_after.len() != expected_size
            || after.modified()? != modified
            || path_after.modified()? != modified {
            return Ok(None);
        }
        Ok(Some(hasher.finalize().to_hex().to_string()))
    })();
    (result, bytes_read)
}

fn prepare_regex(pattern: Option<&str>) -> Result<Option<Regex>, String> {
    match pattern.map(str::trim).filter(|value| !value.is_empty()) {
        Some(pattern) => {
            if pattern.len() > 4096 {
                return Err("Regex excede o limite de 4096 bytes.".into());
            }
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .size_limit(4 * 1024 * 1024)
                .build()
                .map(Some)
                .map_err(|err| format!("Expressão regular inválida: {err}"))
        },
        None => Ok(None),
    }
}

#[cfg(test)]
pub fn scan(request: ScanRequest) -> Result<ScanReport, String> {
    scan_with_control(request, &AtomicBool::new(false), |_| {})
}

pub fn scan_with_control(
    request: ScanRequest,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> Result<ScanReport, String> {
    let timer = Instant::now();
    let hashing_skipped = !request.analyze_duplicates.unwrap_or(true);
    let mut last_update = Instant::now();
    progress(ScanProgress { phase: "scanning".into(), files_scanned: 0, hash_bytes_read: 0 });
    let requested_root = Path::new(&request.root);
    let root_metadata = std::fs::symlink_metadata(requested_root)
        .map_err(|err| format!("Pasta não encontrada ou inacessível: {err}"))?;
    if root_metadata.file_type().is_symlink() || content_status(&root_metadata) != "local" {
        return Err("Selecione uma pasta local, não uma raiz virtual ou junction.".into());
    }
    let root = std::fs::canonicalize(requested_root)
        .map_err(|err| format!("Pasta não encontrada ou inacessível: {err}"))?;
    if !root.is_dir() {
        return Err("Selecione um diretório, não um arquivo.".into());
    }
    let pattern = prepare_regex(request.regex.as_deref())?;
    let min_size = request.min_size_bytes.unwrap_or(0);
    // None means a complete traversal. A cap is only applied when the caller
    // explicitly requests a sample; it is not a hidden product limitation.
    if request.max_files == Some(0) {
        return Err("O limite de arquivos deve ser maior que zero.".into());
    }
    let max_files = request.max_files;
    let mut files_scanned = 0usize;
    // Metadata-only mode must NOT retain every path in RAM. Full BLAKE3 still
    // needs candidate paths, but keeps only path/size/eligibility (not a cloned
    // FileResult for every file).
    let mut records = Vec::<FileRecord>::new();
    let mut top_ranking = BTreeMap::<(u64, String), FileResult>::new();
    let mut directories = HashMap::<PathBuf, DirectorySize>::new();
    let mut types = HashMap::<String, (u64, u64)>::new();
    // A bounded ranking avoids cloning/sorting up to a million matching paths.
    let mut matches = BTreeMap::<(u64, String), FileResult>::new();
    let mut total_matches = 0_usize;
    let mut errors = 0_usize;
    let mut error_samples = Vec::new();
    let mut directories_scanned = 0_usize;
    let mut skipped_reparse_directories = 0_usize;
    let mut logical_bytes = 0_u64;
    let mut truncated = false;

    directories.insert(root.clone(), DirectorySize::default());
    let mut walker = WalkDir::new(&root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        check_cancel(cancel)?;
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
            // Do not recurse into junctions, reparse dirs or cloud virtual folders.
            if let Ok(meta) = std::fs::symlink_metadata(entry.path()) {
                if content_status(&meta) != "local" {
                    walker.skip_current_dir();
                    skipped_reparse_directories += 1;
                    continue;
                }
            }
            directories_scanned += 1;
            directories.entry(entry.path().to_path_buf()).or_default();
            continue;
        }
        if !entry.file_type().is_file() { continue; }
        if max_files.is_some_and(|limit| files_scanned >= limit) {
            truncated = true;
            break;
        }
        let metadata = match std::fs::symlink_metadata(entry.path()) {
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
            content_status: content_status(&metadata).to_owned(),
        };
        files_scanned = files_scanned.saturating_add(1);
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
            matches.insert((size, result.path.clone()), result.clone());
            if matches.len() > MAX_MATCHES { matches.pop_first(); }
        }
        // The UI needs only the largest 300 results; track them as we walk.
        // No O(number_of_files) clone-and-sort pass or full metadata cache.
        top_ranking.insert((size, result.path.clone()), result);
        if top_ranking.len() > MAX_RESULTS { top_ranking.pop_first(); }

        if !hashing_skipped {
            let hash_eligible = !avoid_content_read(&metadata);
            records.push(FileRecord { path, size, hash_eligible });
        }
        if last_update.elapsed() >= Duration::from_millis(250) {
            progress(ScanProgress {
                phase: "scanning".into(),
                files_scanned,
                hash_bytes_read: 0,
            });
            last_update = Instant::now();
        }
    }

    let top_files: Vec<FileResult> = top_ranking.into_iter().rev()
        .map(|(_, value)| value).collect();
    let matches: Vec<FileResult> = matches.into_iter().rev().map(|(_, file)| file).collect();

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

    // Fast inventory mode intentionally skips all content I/O. This is not a
    // duplicate report; do not imply a zero duplicates result or savings.
    if hashing_skipped {
        check_cancel(cancel)?;
        progress(ScanProgress {
            phase: "complete".into(),
            files_scanned,
            hash_bytes_read: 0,
        });
        return Ok(ScanReport {
            root: root.display().to_string(),
            files_scanned,
            directories_scanned,
            logical_bytes,
            elapsed_ms: timer.elapsed().as_millis(),
            errors, error_samples, truncated,
            duplicate_analysis_complete: false,
            hashing_skipped: true,
            hardlink_aliases: 0,
            skipped_content_files: skipped_reparse_directories,
            hash_bytes_read: 0,
            top_files, top_directories, file_types,
            duplicates: Vec::new(), matches,
            total_matches, potential_savings_bytes: 0,
        });
    }

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
    // Directory reparse exclusions make the duplicate inventory deliberately partial.
    let mut skipped_content_files = skipped_reparse_directories;
    // A fully hashed *subset* is not a complete scan if traversal skipped
    // any inaccessible entry, reparse directory, or an explicitly capped tail.
    let mut duplicate_analysis_complete =
        skipped_reparse_directories == 0 && errors == 0 && !truncated;
    let mut groups = HashMap::<(u64, String), Vec<String>>::new();

    // Stage 1: sample only 16 KiB per candidate, grouped by size.
    // This avoids full reads for files that share a size but not a prefix.
    let mut prefix_candidates = Vec::<(u64, Vec<&FileRecord>)>::new();
    let mut sampled = 0usize;
    progress(ScanProgress {
        phase: "fingerprinting".into(), files_scanned: 0, hash_bytes_read: 0,
    });
    for (size, bucket) in candidate_buckets {
        let mut prefixes = HashMap::<String, Vec<&FileRecord>>::new();
        for record in bucket {
            check_cancel(cancel)?;
            sampled += 1;
            if !record.hash_eligible {
                skipped_content_files += 1;
                duplicate_analysis_complete = false;
                continue;
            }
            let planned = size.min(PREFIX_BYTES as u64);
            if hashed_bytes.saturating_add(planned) > MAX_HASH_BYTES {
                duplicate_analysis_complete = false;
                continue;
            }
            let (digest, consumed) = fingerprint_prefix(&record.path, record.size, cancel);
            hashed_bytes = hashed_bytes.saturating_add(consumed);
            check_cancel(cancel)?;
            match digest {
                Ok(Some(prefix)) => prefixes.entry(prefix).or_default().push(record),
                Ok(None) => {
                    errors += 1;
                    duplicate_analysis_complete = false;
                    if error_samples.len() < 12 {
                        error_samples.push(format!(
                            "Arquivo alterado ou indisponível durante amostragem: {}",
                            record.path.display()
                        ));
                    }
                    continue;
                }
                Err(err) => {
                    errors += 1;
                    duplicate_analysis_complete = false;
                    if error_samples.len() < 12 {
                        error_samples.push(format!("{}: {err}", record.path.display()));
                    }
                    continue;
                }
            }
            if last_update.elapsed() >= Duration::from_millis(250) {
                progress(ScanProgress {
                    phase: "fingerprinting".into(), files_scanned: sampled,
                    hash_bytes_read: hashed_bytes,
                });
                last_update = Instant::now();
            }
        }
        for (_, items) in prefixes {
            if items.len() > 1 { prefix_candidates.push((size, items)); }
        }
    }

    // Stage 2: expensive *complete* BLAKE3 only for prefix-matching sets.
    // A matching prefix is NOT considered a duplicate on its own.
    prefix_candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut fully_hashed = 0usize;
    progress(ScanProgress {
        phase: "hashing".into(), files_scanned: 0, hash_bytes_read: hashed_bytes,
    });
    for (_size, bucket) in prefix_candidates {
        for record in bucket {
            check_cancel(cancel)?;
            if hashed_bytes.saturating_add(record.size) > MAX_HASH_BYTES {
                duplicate_analysis_complete = false;
                continue;
            }
            let (hash_result, consumed_bytes) =
                content_hash_with_cancel(&record.path, record.size, cancel);
            hashed_bytes = hashed_bytes.saturating_add(consumed_bytes);
            fully_hashed += 1;
            check_cancel(cancel)?;
            if last_update.elapsed() >= Duration::from_millis(250) {
                progress(ScanProgress {
                    phase: "hashing".into(), files_scanned: fully_hashed,
                    hash_bytes_read: hashed_bytes,
                });
                last_update = Instant::now();
            }
            match hash_result {
                Ok(Some(hash)) => {
                    groups.entry((record.size, hash)).or_default().push(record.path.display().to_string());
                }
                Ok(None) => {
                    errors += 1;
                    duplicate_analysis_complete = false;
                    if error_samples.len() < 12 {
                        error_samples.push(format!(
                            "Arquivo alterado ou indisponível durante hash: {}",
                            record.path.display()
                        ));
                    }
                }
                Err(err) => {
                    errors += 1;
                    duplicate_analysis_complete = false;
                    if error_samples.len() < 12 {
                        error_samples.push(format!("{}: {err}", record.path.display()));
                    }
                }
            }
        }
    }

    let mut hardlink_aliases = 0_usize;
    let mut duplicates = Vec::<DuplicateGroup>::new();
    progress(ScanProgress {
        phase: "verifying".into(),
        files_scanned,
        hash_bytes_read: hashed_bytes,
    });
    for ((size_bytes, hash), copies) in groups {
        check_cancel(cancel)?;
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
            check_cancel(cancel)?;
            // Identity inspection also opens a handle; recheck before doing so.
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if !meta.file_type().is_symlink() && !avoid_content_read(&meta) => {}
                _ => {
                    identity_verified = false;
                    duplicate_analysis_complete = false;
                    skipped_content_files += 1;
                    break;
                }
            }
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

    check_cancel(cancel)?;
    progress(ScanProgress {
        phase: "complete".into(),
        files_scanned,
        hash_bytes_read: hashed_bytes,
    });
    Ok(ScanReport {
        root: root.display().to_string(),
        files_scanned,
        directories_scanned,
        logical_bytes,
        elapsed_ms: timer.elapsed().as_millis(),
        errors,
        error_samples,
        truncated,
        duplicate_analysis_complete,
        hashing_skipped: false,
        hardlink_aliases,
        skipped_content_files,
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
    fn scan_respects_cancellation_before_reading_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("data"), b"test").unwrap();
        let token = AtomicBool::new(true);
        let report = scan_with_control(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None, analyze_duplicates: None,
        }, &token, |_| {});
        assert!(report.unwrap_err().contains("cancelada"));
    }

    #[test]
    fn cancellation_during_hash_stage_drops_partial_report() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), vec![1u8; 4096]).unwrap();
        fs::write(dir.path().join("b"), vec![1u8; 4096]).unwrap();
        let token = AtomicBool::new(false);
        let result = scan_with_control(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None, analyze_duplicates: None,
        }, &token, |p| {
            if p.phase == "hashing" {
                token.store(true, Ordering::Relaxed);
            }
        });
        assert!(result.unwrap_err().contains("cancelada"));
    }

    #[test]
    fn scan_reports_all_phases_without_exposing_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), b"same").unwrap();
        fs::write(dir.path().join("b"), b"same").unwrap();
        let mut events = Vec::new();
        let report = scan_with_control(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None, analyze_duplicates: None,
        }, &AtomicBool::new(false), |p| events.push(p));
        assert_eq!(report.unwrap().files_scanned, 2);
        assert!(events.iter().any(|p| p.phase == "hashing"));
        assert_eq!(events.last().unwrap().phase, "complete");
    }

    #[test]
    fn unlimited_metadata_scan_streams_bounded_results() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..360 {
            fs::write(dir.path().join(format!("item_{i:04}")), [i as u8]).unwrap();
        }
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None,
            analyze_duplicates: Some(false),
        }).unwrap();
        assert_eq!(report.files_scanned, 360);
        assert!(!report.truncated);
        assert_eq!(report.top_files.len(), MAX_RESULTS);
        assert_eq!(report.total_matches, 360);
        assert_eq!(report.matches.len(), 360);
        assert!(report.hashing_skipped);
        assert_eq!(report.hash_bytes_read, 0);
    }

    #[test]
    fn explicit_sampling_limit_remains_available_and_is_disclosed() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..8 {
            fs::write(dir.path().join(format!("file{i}")), [i as u8]).unwrap();
        }
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: Some(3),
            analyze_duplicates: Some(false),
        }).unwrap();
        assert_eq!(report.files_scanned, 3);
        assert!(report.truncated);
    }

    #[test]
    fn explicit_sample_never_claims_complete_duplicate_coverage() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..4 {
            fs::write(dir.path().join(format!("copy_{i}")), b"same").unwrap();
        }
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: Some(2),
            analyze_duplicates: Some(true),
        }).unwrap();
        assert_eq!(report.files_scanned, 2);
        assert!(report.truncated);
        assert!(!report.duplicate_analysis_complete);
    }

    #[test]
    fn zero_file_limit_is_rejected_instead_of_silently_clamped() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: Some(0),
            analyze_duplicates: Some(false),
        }).is_err());
    }

    #[test]
    fn metadata_only_mode_skips_content_hash_and_marks_duplicate_metrics_unknown() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("copy_1.bin"), vec![11u8; 2_000_000]).unwrap();
        fs::write(dir.path().join("copy_2.bin"), vec![11u8; 2_000_000]).unwrap();
        let mut phases = Vec::new();
        let report = scan_with_control(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None,
            analyze_duplicates: Some(false),
        }, &AtomicBool::new(false), |p| phases.push(p.phase)).unwrap();
        assert_eq!(report.files_scanned, 2);
        assert!(report.hashing_skipped);
        assert!(!report.duplicate_analysis_complete);
        assert!(report.duplicates.is_empty());
        assert_eq!(report.hash_bytes_read, 0);
        assert_eq!(report.top_files.len(), 2);
        assert!(!phases.iter().any(|phase| phase == "hashing" || phase == "fingerprinting"));
    }

    #[test]
    fn partial_prefix_rejects_same_size_different_content_without_full_hash() {
        let dir = tempfile::tempdir().unwrap();
        // Prefixes differ at byte zero; no full-file hashing is needed.
        fs::write(dir.path().join("one"), vec![0x11; 64 * 1024]).unwrap();
        fs::write(dir.path().join("two"), vec![0x22; 64 * 1024]).unwrap();
        let mut phases = Vec::new();
        let report = scan_with_control(ScanRequest {
            root: dir.path().display().to_string(), regex: None,
            min_size_bytes: None, max_files: None, analyze_duplicates: None,
        }, &AtomicBool::new(false), |p| phases.push(p.phase)).unwrap();
        assert!(report.duplicates.is_empty());
        assert_eq!(report.hash_bytes_read, 2 * PREFIX_BYTES as u64);
        assert!(report.duplicate_analysis_complete);
        assert!(phases.iter().any(|p| p == "fingerprinting"));
    }

    #[test]
    fn equal_prefix_different_tail_requires_full_content_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = vec![0x11; PREFIX_BYTES + 100];
        let mut second = first.clone();
        first[PREFIX_BYTES] = 1;
        second[PREFIX_BYTES] = 2;
        fs::write(dir.path().join("one"), first).unwrap();
        fs::write(dir.path().join("two"), second).unwrap();
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(), regex: None,
            min_size_bytes: None, max_files: None, analyze_duplicates: None,
        }).unwrap();
        assert!(report.duplicates.is_empty());
        assert_eq!(report.hash_bytes_read, 2 * PREFIX_BYTES as u64
            + 2 * (PREFIX_BYTES as u64 + 100));
        assert!(report.duplicate_analysis_complete);
    }

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
            max_files: None, analyze_duplicates: None,
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
            max_files: None, analyze_duplicates: None,
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
            max_files: None, analyze_duplicates: None,
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
            max_files: None, analyze_duplicates: None,
        }).unwrap();
        assert_eq!(report.files_scanned, 2);
        assert!(report.duplicates.is_empty());
        assert_eq!(report.potential_savings_bytes, 0);
    }

    #[test]
    fn rejects_size_mismatch_before_reading_file_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("size.dat");
        fs::write(&file, b"sample").unwrap();
        assert!(content_hash(&file, 100).0.unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_offline_and_reparse_flags_are_never_hashed() {
        assert!(!unsafe_content_attributes(0));
        for flag in [0x0000_0400, 0x0000_1000, 0x0004_0000, 0x0040_0000] {
            assert!(unsafe_content_attributes(flag));
        }
    }

    #[test]
    fn rejects_invalid_regular_expression() {
        assert!(prepare_regex(Some("[")).is_err());
        assert!(prepare_regex(Some(&"x".repeat(4097))).is_err());
        assert!(prepare_regex(Some(&"x".repeat(4096))).is_ok());
    }

    #[test]
    fn counts_all_successful_read_bytes_for_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload.dat");
        let payload = vec![37u8; 128 * 1024 + 13];
        fs::write(&path, &payload).unwrap();

        let (result, consumed) = content_hash(&path, payload.len() as u64);
        assert!(result.unwrap().is_some());
        assert_eq!(consumed, payload.len() as u64);

        let (mismatch, consumed) = content_hash(&path, payload.len() as u64 + 1);
        assert!(mismatch.unwrap().is_none());
        assert_eq!(consumed, 0);
    }

    #[test]
    fn limits_big_file_results_without_losing_largest_files() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..330 {
            let path = dir.path().join(format!("f{i:03}.dat"));
            fs::write(path, vec![i as u8; i + 1]).unwrap();
        }
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: None, analyze_duplicates: None,
        }).unwrap();
        assert_eq!(report.top_files.len(), MAX_RESULTS);
        assert_eq!(report.top_files[0].size_bytes, 330);
        assert_eq!(report.top_files.last().unwrap().size_bytes, 31);
    }

    #[test]
    fn caps_scanned_files_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("1"), "one").unwrap();
        fs::write(dir.path().join("2"), "two").unwrap();
        let report = scan(ScanRequest {
            root: dir.path().display().to_string(),
            regex: None, min_size_bytes: None, max_files: Some(1), analyze_duplicates: None,
        }).unwrap();
        assert!(report.truncated);
        assert_eq!(report.files_scanned, 1);
    }
}
