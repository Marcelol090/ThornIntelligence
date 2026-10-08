use crate::scan::{FileResult, ScanProgress};
use crate::search::{SearchReport, SearchRequest};
use crate::reparse::{self, ReparseKind};
use regex::RegexBuilder;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub root: String,
    pub files: usize,
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub skipped_directories: usize,
    pub completed_at_unix: i64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedSearch {
    pub report: SearchReport,
    pub completed_at_unix: i64,
}

fn db_err(err: impl std::fmt::Display) -> String {
    format!("Falha no índice local SQLite: {err}")
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .unwrap_or_default().as_secs().min(i64::MAX as u64) as i64
}

fn mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata.modified().ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn attributes(metadata: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
}
#[cfg(not(windows))]
fn attributes(_: &std::fs::Metadata) -> u32 { 0 }

pub(crate) fn connection(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(db_err)?;
    }
    let conn = Connection::open(path).map_err(db_err)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(db_err)?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(db_err)?;
    conn.pragma_update(None, "synchronous", "FULL").map_err(db_err)?;
    conn.execute_batch("
        PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS indexed_scopes (
          root TEXT PRIMARY KEY,
          generation INTEGER NOT NULL,
          completed_at_unix INTEGER NOT NULL,
          file_count INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS indexed_files (
          root TEXT NOT NULL,
          path TEXT NOT NULL,
          size_bytes INTEGER NOT NULL,
          modified_ns INTEGER NOT NULL,
          attributes INTEGER NOT NULL,
          generation INTEGER NOT NULL,
          PRIMARY KEY(root, path)
        );
        CREATE INDEX IF NOT EXISTS idx_indexed_files_generation
          ON indexed_files(root, generation, size_bytes DESC);
    ").map_err(db_err)?;
    Ok(conn)
}

/// Metadata-only incremental refresh. No cloud file contents are opened.
/// Every change and generation bump is one atomic transaction. Cancellation,
/// errors, or file limits roll it back, preserving the last completed snapshot.
pub fn refresh(
    db_path: &Path,
    requested_root: &str,
    max_files: usize,
    cancel: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> Result<IndexStats, String> {
    let timer = Instant::now();
    let selected = Path::new(requested_root);
    let selected_meta = std::fs::symlink_metadata(selected).map_err(db_err)?;
    if reparse::classify(selected, &selected_meta)? == ReparseKind::RedirectedOrUnknown {
        return Err("Raiz redirecionada ou reparse desconhecido: indexação recusada.".into());
    }
    let root = std::fs::canonicalize(requested_root)
        .map_err(|e| format!("Pasta não encontrada: {e}"))?;
    if !root.is_dir() { return Err("Selecione uma pasta.".into()); }
    if reparse::classify(&root, &std::fs::symlink_metadata(&root).map_err(db_err)?)?
        == ReparseKind::RedirectedOrUnknown {
        return Err("Raiz redirecionada ou reparse desconhecido: indexação recusada.".into());
    }
    let root_str = root.to_string_lossy().into_owned();
    let mut conn = connection(db_path)?;
    let tx = conn.transaction().map_err(db_err)?;
    let old_generation: i64 = tx.query_row(
        "SELECT generation FROM indexed_scopes WHERE root = ?1",
        params![root_str], |row| row.get(0),
    ).optional().map_err(db_err)?.unwrap_or(0);
    let generation = old_generation.checked_add(1)
        .ok_or("Contador de gerações do índice esgotado.")?;

    let mut lookup = tx.prepare_cached(
        "SELECT size_bytes, modified_ns, attributes FROM indexed_files WHERE root=?1 AND path=?2"
    ).map_err(db_err)?;
    let mut upsert = tx.prepare_cached("
      INSERT INTO indexed_files(root,path,size_bytes,modified_ns,attributes,generation)
      VALUES(?1,?2,?3,?4,?5,?6)
      ON CONFLICT(root,path) DO UPDATE SET
        size_bytes=excluded.size_bytes,
        modified_ns=excluded.modified_ns,
        attributes=excluded.attributes,
        generation=excluded.generation
    ").map_err(db_err)?;
    let limit = max_files.clamp(1, 1_000_000);
    let mut files = 0usize;
    let mut added = 0usize;
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut skipped_directories = 0usize;
    let mut last_event = Instant::now();
    progress(ScanProgress { phase: "indexing".into(), files_scanned: 0, hash_bytes_read: 0 });

    let mut walker = WalkDir::new(&root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Indexação cancelada pelo usuário. Snapshot anterior preservado.".into());
        }
        let entry = entry.map_err(|e| format!("Índice não publicado: entrada inacessível: {e}"))?;
        if entry.path() == root { continue; }
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|e| format!("Índice não publicado: metadados indisponíveis: {e}"))?;
        if reparse::classify(entry.path(), &metadata)? == ReparseKind::RedirectedOrUnknown {
            if entry.file_type().is_dir() { walker.skip_current_dir(); }
            skipped_directories += usize::from(entry.file_type().is_dir());
            continue;
        }
        if entry.file_type().is_dir() { continue; }
        if !entry.file_type().is_file() { continue; }
        if files >= limit {
            return Err("Indexação excedeu o limite de arquivos; snapshot anterior preservado.".into());
        }
        // Never index our own changing WAL/SHM DB artefacts when scanning AppData.
        if entry.path() == db_path ||
            entry.path() == PathBuf::from(format!("{}-wal", db_path.display())) ||
            entry.path() == PathBuf::from(format!("{}-shm", db_path.display())) {
            continue;
        }
        let path = entry.path().to_string_lossy().into_owned();
        let size = i64::try_from(metadata.len()).map_err(db_err)?;
        let modified = mtime_ns(&metadata);
        let flags = i64::from(attributes(&metadata));
        let prior: Option<(i64, i64, i64)> = lookup.query_row(
            params![root_str, path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ).optional().map_err(db_err)?;
        match prior {
            None => added += 1,
            Some(old) if old == (size, modified, flags) => unchanged += 1,
            Some(_) => changed += 1,
        }
        upsert.execute(params![root_str, path, size, modified, flags, generation])
            .map_err(db_err)?;
        files += 1;
        if last_event.elapsed() >= Duration::from_millis(250) {
            progress(ScanProgress {
                phase: "indexing".into(), files_scanned: files, hash_bytes_read: 0,
            });
            last_event = Instant::now();
        }
    }
    drop(lookup);
    drop(upsert);
    if cancel.load(Ordering::Relaxed) {
        return Err("Indexação cancelada pelo usuário. Snapshot anterior preservado.".into());
    }
    // Rows absent from this generation become invisible to search and are
    // pruned atomically. Reparse subtrees are deliberately outside the scope.
    let removed = tx.execute(
        "DELETE FROM indexed_files WHERE root=?1 AND generation<>?2",
        params![root_str, generation],
    ).map_err(db_err)?;
    let completed_at_unix = now_unix();
    tx.execute(
        "INSERT INTO indexed_scopes(root,generation,completed_at_unix,file_count)
         VALUES(?1,?2,?3,?4)
         ON CONFLICT(root) DO UPDATE SET generation=excluded.generation,
         completed_at_unix=excluded.completed_at_unix,file_count=excluded.file_count",
        params![root_str, generation, completed_at_unix, files as i64],
    ).map_err(db_err)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Indexação cancelada pelo usuário. Snapshot anterior preservado.".into());
    }
    tx.commit().map_err(db_err)?;
    progress(ScanProgress { phase: "complete".into(), files_scanned: files, hash_bytes_read: 0 });
    Ok(IndexStats {
        root: root_str, files, added, changed, unchanged, removed,
        skipped_directories, completed_at_unix, elapsed_ms: timer.elapsed().as_millis(),
    })
}

/// Query only the last committed SQLite generation, without filesystem
/// traversal or opening user files. Results remain snapshots, not live truth.
pub fn search_index(db_path: &Path, request: SearchRequest) -> Result<IndexedSearch, String> {
    let timer = Instant::now();
    let root = std::fs::canonicalize(&request.root)
        .map_err(|e| format!("Raiz do índice indisponível: {e}"))?;
    let root = root.to_string_lossy().into_owned();
    let pattern = request.regex.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let regex = match pattern {
        Some(p) if p.len() > 4096 => return Err("Regex excede 4096 bytes.".into()),
        Some(p) => Some(RegexBuilder::new(p).case_insensitive(true)
            .size_limit(4 * 1024 * 1024).build()
            .map_err(|e| format!("Regex inválida: {e}"))?),
        None => None,
    };
    let conn = connection(db_path)?;
    let (generation, completed_at_unix): (i64, i64) = conn.query_row(
        "SELECT generation,completed_at_unix FROM indexed_scopes WHERE root=?1",
        params![root], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(db_err)?
        .ok_or("Esta pasta ainda não possui snapshot completo. Use Atualizar índice.")?;
    let min_size = i64::try_from(request.min_size_bytes.unwrap_or(0))
        .map_err(|_| "Tamanho mínimo fora do intervalo.")?;
    let mut stmt = conn.prepare(
        "SELECT path,size_bytes FROM indexed_files
         WHERE root=?1 AND generation=?2 AND size_bytes>=?3"
    ).map_err(db_err)?;
    let mut rows = stmt.query(params![root, generation, min_size]).map_err(db_err)?;
    let mut top = BTreeMap::<(u64,String), FileResult>::new();
    let mut total = 0usize;
    let mut candidates = 0usize;
    while let Some(row) = rows.next().map_err(db_err)? {
        let path: String = row.get(0).map_err(db_err)?;
        let size: i64 = row.get(1).map_err(db_err)?;
        candidates += 1;
        if regex.as_ref().is_some_and(|p| !p.is_match(&path)) { continue; }
        total += 1;
        let file_path = Path::new(&path);
        let name = file_path.file_name()
            .map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        let extension = file_path.extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .filter(|s| !s.is_empty()).unwrap_or_else(|| "sem extensão".into());
        let size_bytes = size.max(0) as u64;
        top.insert((size_bytes, path.clone()), FileResult { name, path, size_bytes, extension });
        if top.len() > 500 { top.pop_first(); }
    }
    Ok(IndexedSearch {
        report: SearchReport {
            root, files_scanned: candidates, total_matches: total,
            matches: top.into_iter().rev().map(|(_,f)| f).collect(),
            elapsed_ms: timer.elapsed().as_millis(),
            errors: 0, truncated: false,
        },
        completed_at_unix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn updates_only_changed_entries_and_prunes_removed_entries() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("private/index.db");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "aa").unwrap();
        fs::write(root.join("b.txt"), "bbb").unwrap();
        let c = AtomicBool::new(false);
        let first = refresh(&db, root.to_str().unwrap(), 100, &c, |_| {}).unwrap();
        assert_eq!((first.added,first.changed,first.unchanged), (2,0,0));
        let second = refresh(&db, root.to_str().unwrap(), 100, &c, |_| {}).unwrap();
        assert_eq!((second.added,second.changed,second.unchanged), (0,0,2));
        fs::write(root.join("a.txt"), "longer payload").unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        let third = refresh(&db, root.to_str().unwrap(), 100, &c, |_| {}).unwrap();
        assert_eq!((third.added,third.changed,third.removed), (0,1,1));
        let found = search_index(&db, SearchRequest {
            root: root.to_string_lossy().to_string(),
            regex: Some("a[.]txt$".into()), min_size_bytes: Some(5), max_files: None,
        }).unwrap();
        assert_eq!(found.report.total_matches, 1);
        assert_eq!(found.report.matches[0].name, "a.txt");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_redirected_roots_and_ignores_external_links() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("scope");
        let outside = dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("local.txt"), "local").unwrap();
        fs::write(outside.join("external.txt"), "external").unwrap();
        symlink(&outside, root.join("redirect")).unwrap();
        let db = dir.path().join("index.db");
        let token = AtomicBool::new(false);
        let stats = refresh(&db, root.to_str().unwrap(), 100, &token, |_| {}).unwrap();
        assert_eq!(stats.files, 1);
        symlink(&root, dir.path().join("root-alias")).unwrap();
        assert!(refresh(
            &db, dir.path().join("root-alias").to_str().unwrap(),
            100, &token, |_| {}
        ).is_err());
    }

    #[test]
    fn cancel_does_not_publish_incomplete_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("scope");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.bin"), "one").unwrap();
        let db = dir.path().join("idx.sqlite");
        let token = AtomicBool::new(false);
        refresh(&db, root.to_str().unwrap(), 30, &token, |_| {}).unwrap();
        fs::write(root.join("b.bin"), "two").unwrap();
        token.store(true, Ordering::Relaxed);
        assert!(refresh(&db, root.to_str().unwrap(), 30, &token, |_| {}).is_err());
        let results = search_index(&db, SearchRequest {
            root: root.to_string_lossy().to_string(), regex: None,
            min_size_bytes: None, max_files: None,
        }).unwrap();
        assert_eq!(results.report.total_matches, 1);
    }
}
