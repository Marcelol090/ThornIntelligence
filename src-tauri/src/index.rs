use crate::scan::{FileResult, ScanProgress};
use crate::search::{SearchReport, SearchRequest};
use regex::RegexBuilder;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
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
    pub batches_written: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedSearch {
    pub report: SearchReport,
    pub completed_at_unix: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedPageCursor {
    pub generation: i64,
    pub size_bytes: u64,
    pub path: String,
    pub min_size_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedPageRequest {
    pub root: String,
    pub min_size_bytes: Option<u64>,
    pub page_size: usize,
    pub cursor: Option<IndexedPageCursor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedPage {
    pub root: String,
    pub generation: i64,
    pub completed_at_unix: i64,
    pub min_size_bytes: u64,
    pub items: Vec<FileResult>,
    pub next_cursor: Option<IndexedPageCursor>,
}

fn indexed_content_status(flags: i64) -> String {
    // Mirrors scan::content_status; no file content or cloud data is opened.
    let status = if flags & (0x1000 | 0x40000 | 0x400000) != 0 { "offline" }
        else if flags & 0x400 != 0 { "reparse" } else { "local" };
    status.to_owned()
}

fn indexed_file(path: String, size: i64, flags: i64) -> FileResult {
    let file_path = Path::new(&path);
    let name = file_path.file_name()
        .map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
    let extension = file_path.extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .filter(|s| !s.is_empty()).unwrap_or_else(|| "sem extensão".into());
    FileResult {
        name, path, size_bytes: size.max(0) as u64, extension,
        content_status: indexed_content_status(flags),
    }
}

const INDEX_BATCH_SIZE: usize = 1024;
const MAX_INDEX_PAGE_SIZE: usize = 200;

struct StagedFile {
    path: String,
    size: i64,
    modified: i64,
    flags: i64,
}

/// Spool at most 1024 metadata rows per connection-private TEMP transaction.
/// The previously published snapshot remains readable while the walk is ongoing.
fn flush_stage(conn: &mut Connection, root: &str, batch: &mut Vec<StagedFile>) -> Result<(), String> {
    if batch.is_empty() { return Ok(()); }
    let tx = conn.transaction().map_err(db_err)?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO temp.indexed_stage_session(root,path,size_bytes,modified_ns,attributes)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(root,path) DO UPDATE SET
                 size_bytes=excluded.size_bytes,
                 modified_ns=excluded.modified_ns,
                 attributes=excluded.attributes",
        ).map_err(db_err)?;
        for row in batch.iter() {
            stmt.execute(params![root, row.path, row.size, row.modified, row.flags])
                .map_err(db_err)?;
        }
    }
    tx.commit().map_err(db_err)?;
    batch.clear();
    Ok(())
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

fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || (attributes(metadata) & 0x400 != 0)
}

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
        CREATE INDEX IF NOT EXISTS idx_indexed_files_page
          ON indexed_files(root, generation, size_bytes DESC, path ASC);
    ").map_err(db_err)?;
    Ok(conn)
}

/// Pause within a running index task, without publishing an incomplete scope.
/// We cannot safely resume a filesystem walker after the app exits until we
/// can verify missed changes using a volume journal (USN where supported).
fn wait_indexer(
    paused: &AtomicBool,
    cancel: &AtomicBool,
    files: usize,
    progress: &mut impl FnMut(ScanProgress),
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Indexação cancelada; snapshot anterior preservado.".into());
    }
    if paused.load(Ordering::Relaxed) {
        progress(ScanProgress { phase: "paused".into(), files_scanned: files, hash_bytes_read: 0 });
        while paused.load(Ordering::Relaxed) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Indexação cancelada; snapshot anterior preservado.".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Indexação cancelada; snapshot anterior preservado.".into());
        }
        progress(ScanProgress { phase: "indexing".into(), files_scanned: files, hash_bytes_read: 0 });
    }
    Ok(())
}

/// Incrementally inventory metadata into bounded connection-private TEMP batches.
/// Readers continue to see the previous completed snapshot throughout traversal.
/// A separate, atomic publication transaction updates live rows only after every
/// accessible directory/file has been processed successfully. An interrupted
/// walk discards its connection-private stage on close; a new refresh must
/// enumerate the filesystem again rather than trusting interrupted work.
pub fn refresh(
    db_path: &Path,
    requested_root: &str,
    max_files: Option<usize>,
    cancel: &AtomicBool,
    paused: &AtomicBool,
    mut progress: impl FnMut(ScanProgress),
) -> Result<IndexStats, String> {
    let timer = Instant::now();
    if max_files == Some(0) {
        return Err("O limite de amostragem deve ser maior que zero.".into());
    }
    let source = std::fs::symlink_metadata(requested_root)
        .map_err(|e| format!("Pasta não encontrada: {e}"))?;
    if !source.is_dir() || is_reparse(&source) {
        return Err("Selecione uma pasta local, não uma raiz redirecionada.".into());
    }
    let root = std::fs::canonicalize(requested_root)
        .map_err(|e| format!("Pasta não encontrada: {e}"))?;
    let root_str = root.to_string_lossy().into_owned();
    let mut conn = connection(db_path)?;

    // A TEMP table belongs to this connection only. Two application processes
    // can stage the same root without overwriting each other's rows. Staging
    // vanishes on close/crash and cannot be mistaken for a published snapshot.
    conn.execute_batch("
        CREATE TEMP TABLE indexed_stage_session (
          root TEXT NOT NULL,
          path TEXT NOT NULL,
          size_bytes INTEGER NOT NULL,
          modified_ns INTEGER NOT NULL,
          attributes INTEGER NOT NULL,
          PRIMARY KEY(root,path)
        );
    ").map_err(db_err)?;
    let old_generation: i64 = conn.query_row(
        "SELECT generation FROM indexed_scopes WHERE root = ?1",
        params![root_str], |row| row.get(0),
    ).optional().map_err(db_err)?.unwrap_or(0);
    let generation = old_generation.checked_add(1)
        .ok_or("Contador de gerações do índice esgotado.")?;

    let mut batch = Vec::<StagedFile>::with_capacity(INDEX_BATCH_SIZE);
    let mut files = 0usize;
    let mut added = 0usize;
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut skipped_directories = 0usize;
    let mut batches_written = 0usize;
    let mut last_event = Instant::now();
    progress(ScanProgress { phase: "indexing".into(), files_scanned: 0, hash_bytes_read: 0 });

    let mut walker = WalkDir::new(&root).follow_links(false).into_iter();
    loop {
        wait_indexer(paused, cancel, files, &mut progress)?;
        let Some(entry) = walker.next() else { break };
        wait_indexer(paused, cancel, files, &mut progress)?;
        let entry = entry.map_err(|e| format!("Índice não publicado: entrada inacessível: {e}"))?;
        if entry.path() == root { continue; }
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|e| format!("Índice não publicado: metadados indisponíveis: {e}"))?;
        if is_reparse(&metadata) {
            if entry.file_type().is_dir() { walker.skip_current_dir(); }
            skipped_directories += usize::from(entry.file_type().is_dir());
            continue;
        }
        if entry.file_type().is_dir() { continue; }
        if !entry.file_type().is_file() { continue; }
        // Do not turn an explicit sample into a misleading "complete" index.
        if max_files.is_some_and(|limit| files >= limit) {
            return Err("Amostragem interrompeu a indexação; snapshot anterior preservado.".into());
        }
        // Never index the active SQLite database/WAL/SHM artefacts as they change.
        if entry.path() == db_path ||
            entry.path() == PathBuf::from(format!("{}-wal", db_path.display())) ||
            entry.path() == PathBuf::from(format!("{}-shm", db_path.display())) {
            continue;
        }
        let path = entry.path().to_string_lossy().into_owned();
        let size = i64::try_from(metadata.len()).map_err(db_err)?;
        let modified = mtime_ns(&metadata);
        let flags = i64::from(attributes(&metadata));
        let prior: Option<(i64, i64, i64)> = conn.query_row(
            "SELECT size_bytes,modified_ns,attributes FROM indexed_files WHERE root=?1 AND path=?2",
            params![root_str, path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(db_err)?;
        match prior {
            None => added += 1,
            Some(old) if old == (size, modified, flags) => unchanged += 1,
            Some(_) => changed += 1,
        }
        batch.push(StagedFile { path, size, modified, flags });
        files += 1;
        if batch.len() == INDEX_BATCH_SIZE {
            flush_stage(&mut conn, &root_str, &mut batch)?;
            batches_written += 1;
            // Every durable checkpoint emits progress, independently of the
            // 250-ms timer. Tests and cancellation can reliably stop here.
            progress(ScanProgress {
                phase: "indexing".into(), files_scanned: files, hash_bytes_read: 0,
            });
            last_event = Instant::now();
        }
        if last_event.elapsed() >= Duration::from_millis(250) {
            progress(ScanProgress {
                phase: "indexing".into(), files_scanned: files, hash_bytes_read: 0,
            });
            last_event = Instant::now();
        }
    }
    wait_indexer(paused, cancel, files, &mut progress)?;
    if !batch.is_empty() {
        flush_stage(&mut conn, &root_str, &mut batch)?;
        batches_written += 1;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Indexação cancelada; snapshot anterior preservado.".into());
    }

    // Only the final publication writes the shared database. BEGIN IMMEDIATE
    // serializes publishers across processes; the generation check prevents a
    // stale scan from replacing a newer completed snapshot.
    wait_indexer(paused, cancel, files, &mut progress)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
    let current_generation: i64 = tx.query_row(
        "SELECT generation FROM indexed_scopes WHERE root=?1",
        params![root_str], |row| row.get(0),
    ).optional().map_err(db_err)?.unwrap_or(0);
    if current_generation != old_generation {
        return Err("Conflito de indexação: outra instância publicou um snapshot mais recente. Atualize novamente para reconciliar.".into());
    }
    tx.execute(
        "INSERT INTO indexed_files(root,path,size_bytes,modified_ns,attributes,generation)
         SELECT root,path,size_bytes,modified_ns,attributes,?2
         FROM temp.indexed_stage_session WHERE root=?1 AND 1=1
         ON CONFLICT(root,path) DO UPDATE SET
             size_bytes=excluded.size_bytes,
             modified_ns=excluded.modified_ns,
             attributes=excluded.attributes,
             generation=excluded.generation",
        params![root_str, generation],
    ).map_err(db_err)?;
    let removed = tx.execute(
        "DELETE FROM indexed_files WHERE root=?1 AND generation<>?2",
        params![root_str, generation],
    ).map_err(db_err)?;
    let completed_at_unix = now_unix();
    tx.execute(
        "INSERT INTO indexed_scopes(root,generation,completed_at_unix,file_count)
         VALUES(?1,?2,?3,?4)
         ON CONFLICT(root) DO UPDATE SET
             generation=excluded.generation,
             completed_at_unix=excluded.completed_at_unix,
             file_count=excluded.file_count",
        params![root_str, generation, completed_at_unix, files as i64],
    ).map_err(db_err)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Indexação cancelada; snapshot anterior preservado.".into());
    }
    tx.commit().map_err(db_err)?;
    progress(ScanProgress { phase: "complete".into(), files_scanned: files, hash_bytes_read: 0 });
    Ok(IndexStats {
        root: root_str, files, added, changed, unchanged, removed,
        skipped_directories, completed_at_unix, elapsed_ms: timer.elapsed().as_millis(),
        batches_written,
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
    let mut conn = connection(db_path)?;
    // Read scope metadata and its rows from one WAL snapshot, even if a
    // different process publishes a new generation during this search.
    let tx = conn.transaction().map_err(db_err)?;
    let (generation, completed_at_unix): (i64, i64) = tx.query_row(
        "SELECT generation,completed_at_unix FROM indexed_scopes WHERE root=?1",
        params![root], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(db_err)?
        .ok_or("Esta pasta ainda não possui snapshot completo. Use Atualizar índice.")?;
    let min_size = i64::try_from(request.min_size_bytes.unwrap_or(0))
        .map_err(|_| "Tamanho mínimo fora do intervalo.")?;
    let mut stmt = tx.prepare(
        "SELECT path,size_bytes,attributes FROM indexed_files
         WHERE root=?1 AND generation=?2 AND size_bytes>=?3"
    ).map_err(db_err)?;
    let mut rows = stmt.query(params![root, generation, min_size]).map_err(db_err)?;
    let mut top = BTreeMap::<(u64,String), FileResult>::new();
    let mut total = 0usize;
    let mut candidates = 0usize;
    while let Some(row) = rows.next().map_err(db_err)? {
        let path: String = row.get(0).map_err(db_err)?;
        let size: i64 = row.get(1).map_err(db_err)?;
        let flags: i64 = row.get(2).map_err(db_err)?;
        candidates += 1;
        if regex.as_ref().is_some_and(|p| !p.is_match(&path)) { continue; }
        total += 1;
        let size_bytes = size.max(0) as u64;
        top.insert((size_bytes, path.clone()), indexed_file(path, size, flags));
        if top.len() > 500 { top.pop_first(); }
    }
    drop(rows);
    drop(stmt);
    tx.commit().map_err(db_err)?;
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


/// Bounded read-only keyset pagination of one committed SQLite generation.
/// No OFFSET, filesystem traversal, file content access, or unbounded result vector.
pub fn browse_index(db_path: &Path, request: IndexedPageRequest) -> Result<IndexedPage, String> {
    if request.page_size == 0 || request.page_size > MAX_INDEX_PAGE_SIZE {
        return Err(format!("Tamanho de página deve estar entre 1 e {MAX_INDEX_PAGE_SIZE}."));
    }
    let min_size_bytes = request.min_size_bytes.unwrap_or(0);
    let min_size = i64::try_from(min_size_bytes)
        .map_err(|_| "Tamanho mínimo fora do intervalo.")?;
    let root = std::fs::canonicalize(&request.root)
        .map_err(|e| format!("Raiz do índice indisponível: {e}"))?
        .to_string_lossy().into_owned();
    let mut conn = connection(db_path)?;
    let tx = conn.transaction().map_err(db_err)?;
    let (generation, completed_at_unix): (i64, i64) = tx.query_row(
        "SELECT generation,completed_at_unix FROM indexed_scopes WHERE root=?1",
        params![root], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(db_err)?
        .ok_or("Esta pasta ainda não possui snapshot completo. Use Atualizar índice.")?;
    if let Some(cursor) = &request.cursor {
        if cursor.generation != generation {
            return Err("Snapshot do índice atualizado; reinicie a navegação pela primeira página.".into());
        }
        if cursor.min_size_bytes != min_size_bytes || cursor.path.is_empty() {
            return Err("Cursor incompatível com o filtro; reinicie pela primeira página.".into());
        }
    }
    let cursor_size = request.cursor.as_ref()
        .map(|cursor| i64::try_from(cursor.size_bytes)
            .map_err(|_| "Cursor de tamanho inválido."))
        .transpose()?.unwrap_or(i64::MAX);
    let cursor_path = request.cursor.as_ref().map(|c| c.path.as_str()).unwrap_or("");
    let sql = if request.cursor.is_some() {
        "SELECT path,size_bytes,attributes FROM indexed_files
         WHERE root=?1 AND generation=?2 AND size_bytes>=?3
           AND (size_bytes<?4 OR (size_bytes=?4 AND path>?5))
         ORDER BY size_bytes DESC,path ASC LIMIT ?6"
    } else {
        "SELECT path,size_bytes,attributes FROM indexed_files
         WHERE root=?1 AND generation=?2 AND size_bytes>=?3
         ORDER BY size_bytes DESC,path ASC LIMIT ?6"
    };
    let limit = i64::try_from(request.page_size + 1)
        .map_err(|_| "Tamanho de página inválido.")?;
    let mut stmt = tx.prepare(sql).map_err(db_err)?;
    let mut rows = stmt.query(params![root, generation, min_size, cursor_size, cursor_path, limit])
        .map_err(db_err)?;
    let mut items = Vec::with_capacity(request.page_size + 1);
    while let Some(row) = rows.next().map_err(db_err)? {
        let path: String = row.get(0).map_err(db_err)?;
        let size: i64 = row.get(1).map_err(db_err)?;
        let flags: i64 = row.get(2).map_err(db_err)?;
        items.push(indexed_file(path, size, flags));
    }
    let has_more = items.len() > request.page_size;
    items.truncate(request.page_size);
    let next_cursor = if has_more {
        items.last().map(|last| IndexedPageCursor {
            generation, size_bytes: last.size_bytes, path: last.path.clone(), min_size_bytes,
        })
    } else { None };
    drop(rows);
    drop(stmt);
    tx.commit().map_err(db_err)?;
    Ok(IndexedPage {
        root, generation, completed_at_unix, min_size_bytes, items, next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn keyset_pages_are_bounded_complete_and_cloud_aware() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.sqlite");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        for i in 0..257 {
            fs::write(root.join(format!("item-{i:04}.bin")), vec![0u8; i % 5 + 1]).unwrap();
        }
        refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
        let marked = root.join("item-0000.bin").display().to_string();
        connection(&db).unwrap().execute(
            "UPDATE indexed_files SET attributes=4096 WHERE path=?1", params![marked],
        ).unwrap();
        let mut cursor = None;
        let mut observed = Vec::<(u64, String)>::new();
        let mut saw_offline = false;
        loop {
            let page = browse_index(&db, IndexedPageRequest {
                root: root.display().to_string(), min_size_bytes: None,
                page_size: 17, cursor,
            }).unwrap();
            assert!(page.items.len() <= 17);
            for item in page.items {
                if item.path == marked {
                    assert_eq!(item.content_status, "offline");
                    saw_offline = true;
                }
                observed.push((item.size_bytes, item.path));
            }
            cursor = page.next_cursor;
            if cursor.is_none() { break; }
        }
        assert_eq!(observed.len(), 257);
        assert!(saw_offline);
        let mut unique = std::collections::HashSet::new();
        assert!(observed.iter().all(|(_, path)| unique.insert(path.clone())));
        assert!(observed.windows(2).all(|w|
            w[0].0 > w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1)
        ));
    }

    #[test]
    fn keyset_cursor_rejects_new_generation_and_changed_filter() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.sqlite");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        for i in 0..3 { fs::write(root.join(format!("{i}.txt")), "x").unwrap(); }
        refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
        let first = browse_index(&db, IndexedPageRequest {
            root: root.display().to_string(), min_size_bytes: Some(0),
            page_size: 1, cursor: None,
        }).unwrap();
        let cursor = first.next_cursor.unwrap();
        assert!(browse_index(&db, IndexedPageRequest {
            root: root.display().to_string(), min_size_bytes: Some(1),
            page_size: 1, cursor: Some(cursor.clone()),
        }).unwrap_err().contains("Cursor incompatível"));
        fs::write(root.join("new.txt"), "new").unwrap();
        refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
        assert!(browse_index(&db, IndexedPageRequest {
            root: root.display().to_string(), min_size_bytes: Some(0),
            page_size: 1, cursor: Some(cursor),
        }).unwrap_err().contains("Snapshot do índice atualizado"));
        assert!(browse_index(&db, IndexedPageRequest {
            root: root.display().to_string(), min_size_bytes: None,
            page_size: 201, cursor: None,
        }).is_err());
    }

    #[test]
    fn updates_only_changed_entries_and_prunes_removed_entries() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("private/index.db");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "aa").unwrap();
        fs::write(root.join("b.txt"), "bbb").unwrap();
        let c = AtomicBool::new(false);
        let first = refresh(&db, root.to_str().unwrap(), None, &c, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!((first.added,first.changed,first.unchanged), (2,0,0));
        let second = refresh(&db, root.to_str().unwrap(), None, &c, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!((second.added,second.changed,second.unchanged), (0,0,2));
        fs::write(root.join("a.txt"), "longer payload").unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        let third = refresh(&db, root.to_str().unwrap(), None, &c, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!((third.added,third.changed,third.removed), (0,1,1));
        let found = search_index(&db, SearchRequest {
            root: root.to_string_lossy().to_string(),
            regex: Some("a[.]txt$".into()), min_size_bytes: Some(5), max_files: None,
        }).unwrap();
        assert_eq!(found.report.total_matches, 1);
        assert_eq!(found.report.matches[0].name, "a.txt");
    }

    #[test]
    fn batches_above_1024_rows_and_publishes_one_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("private/index.db");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        for i in 0..1035 {
            fs::write(root.join(format!("{i:04}.txt")), b"content").unwrap();
        }
        let stats = refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(stats.files, 1035);
        assert_eq!(stats.added, 1035);
        assert_eq!(stats.batches_written, 2);
        let conn = connection(&db).unwrap();
        let live: i64 = conn.query_row("SELECT COUNT(*) FROM indexed_files", [], |r| r.get(0)).unwrap();
        let legacy_stage_tables: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='indexed_stage'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(live, 1035);
        assert_eq!(legacy_stage_tables, 0);
    }

    #[test]
    fn competing_refreshes_cannot_publish_each_others_staged_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.sqlite");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        for i in 0..(INDEX_BATCH_SIZE + 8) {
            fs::write(root.join(format!("item-{i:04}.bin")), b"x").unwrap();
        }
        let mut competing_refresh_completed = false;
        let first = refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |event| {
                if event.phase == "indexing"
                    && event.files_scanned == INDEX_BATCH_SIZE
                    && !competing_refresh_completed
                {
                    let newer = refresh(&db, root.to_str().unwrap(), None,
                        &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
                    assert_eq!(newer.files, INDEX_BATCH_SIZE + 8);
                    competing_refresh_completed = true;
                }
            });
        assert!(competing_refresh_completed);
        assert!(first.unwrap_err().contains("Conflito de indexação"));
        let published = search_index(&db, SearchRequest {
            root: root.display().to_string(), regex: None,
            min_size_bytes: None, max_files: None,
        }).unwrap();
        assert_eq!(published.report.total_matches, INDEX_BATCH_SIZE + 8);
        let conn = connection(&db).unwrap();
        let (generation, count): (i64, i64) = conn.query_row(
            "SELECT generation,file_count FROM indexed_scopes WHERE root=?1",
            params![root.display().to_string()], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!((generation, count), (1, (INDEX_BATCH_SIZE + 8) as i64));
    }

    #[test]
    fn read_transaction_keeps_generation_and_rows_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.sqlite");
        let root = dir.path().join("scan");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("old.txt"), "old").unwrap();
        refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
        let mut reader = connection(&db).unwrap();
        let tx = reader.transaction().unwrap();
        let generation: i64 = tx.query_row(
            "SELECT generation FROM indexed_scopes WHERE root=?1",
            params![root.display().to_string()], |r| r.get(0),
        ).unwrap();
        fs::write(root.join("new.txt"), "new").unwrap();
        refresh(&db, root.to_str().unwrap(), None,
            &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).unwrap();
        let snapshot_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM indexed_files WHERE root=?1 AND generation=?2",
            params![root.display().to_string(), generation], |r| r.get(0),
        ).unwrap();
        assert_eq!(snapshot_count, 1);
        tx.commit().unwrap();
        assert_eq!(search_index(&db, SearchRequest {
            root: root.display().to_string(), regex: None,
            min_size_bytes: None, max_files: None,
        }).unwrap().report.total_matches, 2);
    }

    #[test]
    fn cancellation_after_durable_batch_preserves_published_generation() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("idx.sqlite");
        let root = dir.path().join("scope");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("original"), b"keep").unwrap();
        let cancel = AtomicBool::new(false);
        refresh(&db, root.to_str().unwrap(), None, &cancel, &AtomicBool::new(false), |_| {}).unwrap();
        for i in 0..1050 {
            fs::write(root.join(format!("new-{i:04}")), b"x").unwrap();
        }
        let result = refresh(&db, root.to_str().unwrap(), None, &cancel, &AtomicBool::new(false), |progress| {
            if progress.files_scanned >= 1024 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(result.is_err());
        let unchanged = search_index(&db, SearchRequest {
            root: root.display().to_string(), regex: None,
            min_size_bytes: None, max_files: None,
        }).unwrap();
        assert_eq!(unchanged.report.total_matches, 1);
        // Restart performs a complete re-enumeration; staged data isn't
        // mistaken for a valid filesystem snapshot.
        cancel.store(false, Ordering::Relaxed);
        let done = refresh(&db, root.to_str().unwrap(), None, &cancel, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(done.files, 1051);
    }

    #[test]
    fn pause_then_resume_reaches_a_complete_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        for i in 0..1030 {
            fs::write(root.join(format!("item-{i:04}")), "x").unwrap();
        }
        let paused = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        let mut observed_pause = false;
        let report = refresh(&dir.path().join("idx.sqlite"), root.to_str().unwrap(),
            None, &cancel, &paused, |event| {
                if event.phase == "indexing" && event.files_scanned == INDEX_BATCH_SIZE
                    && !observed_pause {
                    paused.store(true, Ordering::Relaxed);
                }
                if event.phase == "paused" {
                    observed_pause = true;
                    paused.store(false, Ordering::Relaxed);
                }
            }).unwrap();
        assert!(observed_pause);
        assert_eq!(report.files, 1030);
        assert_eq!(report.batches_written, 2);
    }

    #[test]
    fn cancel_while_paused_preserves_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("original"), "keep").unwrap();
        let db = dir.path().join("idx.sqlite");
        let paused = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        refresh(&db, root.to_str().unwrap(), None, &cancel, &paused, |_| {}).unwrap();
        for i in 0..1040 { fs::write(root.join(format!("new-{i:04}")), "x").unwrap(); }
        let mut observed_pause = false;
        let result = refresh(&db, root.to_str().unwrap(), None, &cancel, &paused, |event| {
            if event.phase == "indexing" && event.files_scanned == INDEX_BATCH_SIZE {
                paused.store(true, Ordering::Relaxed);
            }
            if event.phase == "paused" {
                observed_pause = true;
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(observed_pause);
        assert!(result.unwrap_err().contains("cancelada"));
        let prior = search_index(&db, SearchRequest {
            root: root.display().to_string(), regex: None, min_size_bytes: None,
            max_files: None,
        }).unwrap();
        assert_eq!(prior.report.total_matches, 1);
    }

    #[test]
    fn rejects_explicit_truncated_index_instead_of_marking_complete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a"), "1").unwrap();
        fs::write(root.join("b"), "2").unwrap();
        assert!(refresh(&dir.path().join("idx.sqlite"), root.to_str().unwrap(),
            Some(1), &AtomicBool::new(false), &AtomicBool::new(false), |_| {}).is_err());
    }

    #[test]
    fn cancel_does_not_publish_incomplete_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("scope");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.bin"), "one").unwrap();
        let db = dir.path().join("idx.sqlite");
        let token = AtomicBool::new(false);
        refresh(&db, root.to_str().unwrap(), None, &token, &AtomicBool::new(false), |_| {}).unwrap();
        fs::write(root.join("b.bin"), "two").unwrap();
        token.store(true, Ordering::Relaxed);
        assert!(refresh(&db, root.to_str().unwrap(), None, &token, &AtomicBool::new(false), |_| {}).is_err());
        let results = search_index(&db, SearchRequest {
            root: root.to_string_lossy().to_string(), regex: None,
            min_size_bytes: None, max_files: None,
        }).unwrap();
        assert_eq!(results.report.total_matches, 1);
    }
}
