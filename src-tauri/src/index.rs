use crate::scan::{FileResult, ScanProgress};
use crate::search::{SearchReport, SearchRequest};
use regex::RegexBuilder;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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

const INDEX_BATCH_SIZE: usize = 1024;
const TREE_PAGE_DEFAULT: usize = 100;
const TREE_PAGE_MAX: usize = 200;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeCursor {
    pub kind: String,
    pub size_bytes: u64,
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeRequest {
    pub root: String,
    pub parent_path: String,
    pub after: Option<TreeCursor>,
    pub limit: Option<usize>,
    pub generation: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size_bytes: u64,
    pub files: u64,
    pub content_status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreePage {
    pub root: String,
    pub parent_path: String,
    pub nodes: Vec<TreeNode>,
    pub next_cursor: Option<TreeCursor>,
    pub completed_at_unix: i64,
    pub generation: i64,
}


struct StagedFile {
    path: String,
    parent_path: String,
    size: i64,
    modified: i64,
    flags: i64,
}

/// Durably spool at most 1024 metadata rows per WAL write transaction. The
/// previously published snapshot remains readable while the walk is ongoing.
fn flush_stage(conn: &mut Connection, root: &str, batch: &mut Vec<StagedFile>) -> Result<(), String> {
    if batch.is_empty() { return Ok(()); }
    let tx = conn.transaction().map_err(db_err)?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO indexed_stage(root,path,parent_path,size_bytes,modified_ns,attributes)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(root,path) DO UPDATE SET
                 parent_path=excluded.parent_path,
                 size_bytes=excluded.size_bytes,
                 modified_ns=excluded.modified_ns,
                 attributes=excluded.attributes",
        ).map_err(db_err)?;
        for row in batch.iter() {
            stmt.execute(params![root, row.path, row.parent_path, row.size, row.modified, row.flags])
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
        -- Incomplete staged rows are not referenced by the published scope.
        CREATE TABLE IF NOT EXISTS indexed_stage (
          root TEXT NOT NULL,
          path TEXT NOT NULL,
          size_bytes INTEGER NOT NULL,
          modified_ns INTEGER NOT NULL,
          attributes INTEGER NOT NULL,
          PRIMARY KEY(root,path)
        );
        CREATE TABLE IF NOT EXISTS indexed_directories (
          root TEXT NOT NULL,
          path TEXT NOT NULL,
          parent_path TEXT NOT NULL,
          logical_bytes INTEGER NOT NULL,
          file_count INTEGER NOT NULL,
          generation INTEGER NOT NULL,
          PRIMARY KEY(root,path)
        );
        CREATE INDEX IF NOT EXISTS idx_indexed_dirs_children
          ON indexed_directories(root,parent_path,generation,logical_bytes DESC,path);
    ").map_err(db_err)?;
    // Idempotent migration: existing installations already have the file and
    // stage tables, but not the direct-parent column.
    for table in ["indexed_files", "indexed_stage"] {
        let sql = format!("PRAGMA table_info({table})");
        let mut info = conn.prepare(&sql).map_err(db_err)?;
        let columns = info.query_map([], |row| row.get::<_,String>(1)).map_err(db_err)?;
        let mut has_parent = false;
        for column in columns {
            if column.map_err(db_err)? == "parent_path" { has_parent = true; }
        }
        drop(info);
        if !has_parent {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN parent_path TEXT NOT NULL DEFAULT ''"
            )).map_err(db_err)?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_indexed_files_children
          ON indexed_files(root,parent_path,generation,size_bytes DESC,path);"
    ).map_err(db_err)?;
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

/// Incrementally inventory metadata into bounded, durable SQLite WAL batches.
/// Readers continue to see the previous completed snapshot throughout traversal.
/// A separate, atomic publication transaction updates live rows only after every
/// accessible directory/file has been processed successfully. An interrupted
/// walk may leave unused staging rows; the next refresh discards those rows
/// rather than treating a stale checkpoint as proof of current filesystem state.
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

    // The previous snapshot is never modified until final publication.
    // Cleanup of a canceled staging generation is safe and isolated by root.
    conn.execute("DELETE FROM indexed_stage WHERE root=?1", params![root_str])
        .map_err(db_err)?;
    let old_generation: i64 = conn.query_row(
        "SELECT generation FROM indexed_scopes WHERE root = ?1",
        params![root_str], |row| row.get(0),
    ).optional().map_err(db_err)?.unwrap_or(0);
    let generation = old_generation.checked_add(1)
        .ok_or("Contador de gerações do índice esgotado.")?;

    let mut batch = Vec::<StagedFile>::with_capacity(INDEX_BATCH_SIZE);
    // O(number of distinct directories) memory, not one React node per file.
    // Both empty folders and ancestor totals are retained for tree navigation.
    let mut folder_totals = HashMap::<PathBuf,(u64,u64)>::new();
    folder_totals.insert(root.clone(), (0,0));
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
        if entry.file_type().is_dir() {
            folder_totals.entry(entry.path().to_path_buf()).or_insert((0,0));
            continue;
        }
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
        let parent_path = entry.path().parent().unwrap_or(&root).to_string_lossy().into_owned();
        let size = i64::try_from(metadata.len()).map_err(db_err)?;
        for ancestor in entry.path().parent().into_iter().flat_map(Path::ancestors) {
            if !ancestor.starts_with(&root) { break; }
            let total = folder_totals.entry(ancestor.to_path_buf()).or_insert((0,0));
            total.0 = total.0.saturating_add(metadata.len());
            total.1 = total.1.saturating_add(1);
            if ancestor == root { break; }
        }
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
        batch.push(StagedFile { path, parent_path, size, modified, flags });
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

    // Publication still requires one atomic SQL transaction. For multi-million
    // files this final merge can produce substantial WAL I/O; it does not hold
    // a write transaction during the preceding, potentially hours-long walk.
    wait_indexer(paused, cancel, files, &mut progress)?;
    let tx = conn.transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO indexed_files(root,path,parent_path,size_bytes,modified_ns,attributes,generation)
         SELECT root,path,parent_path,size_bytes,modified_ns,attributes,?2
         FROM indexed_stage WHERE root=?1 AND 1=1
         ON CONFLICT(root,path) DO UPDATE SET
             parent_path=excluded.parent_path,
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
    // Replace all folder aggregates in the SAME publication transaction as
    // indexed_files and indexed_scopes; canceled refresh preserves old tree.
    tx.execute("DELETE FROM indexed_directories WHERE root=?1", params![root_str])
        .map_err(db_err)?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO indexed_directories
             (root,path,parent_path,logical_bytes,file_count,generation)
             VALUES(?1,?2,?3,?4,?5,?6)"
        ).map_err(db_err)?;
        for (directory, (logical, file_count)) in &folder_totals {
            let directory_path = directory.to_string_lossy().into_owned();
            let parent_path = if directory == &root { String::new() } else {
                directory.parent().unwrap_or(&root).to_string_lossy().into_owned()
            };
            stmt.execute(params![
                root_str, directory_path, parent_path,
                (*logical).min(i64::MAX as u64) as i64,
                (*file_count).min(i64::MAX as u64) as i64,
                generation,
            ]).map_err(db_err)?;
        }
    }
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
    tx.execute("DELETE FROM indexed_stage WHERE root=?1", params![root_str])
        .map_err(db_err)?;
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
    let conn = connection(db_path)?;
    let (generation, completed_at_unix): (i64, i64) = conn.query_row(
        "SELECT generation,completed_at_unix FROM indexed_scopes WHERE root=?1",
        params![root], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(db_err)?
        .ok_or("Esta pasta ainda não possui snapshot completo. Use Atualizar índice.")?;
    let min_size = i64::try_from(request.min_size_bytes.unwrap_or(0))
        .map_err(|_| "Tamanho mínimo fora do intervalo.")?;
    let mut stmt = conn.prepare(
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
        let attr: i64 = row.get(2).map_err(db_err)?;
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
        let content_status = if attr & (0x1000 | 0x40000 | 0x400000) != 0 {
            "offline"
        } else if attr & 0x400 != 0 { "reparse" } else { "local" };
        top.insert((size_bytes, path.clone()), FileResult {
            name, path, size_bytes, extension, content_status: content_status.into()
        });
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

/// Index-backed, keyset-paginated, lazy children of one directory.
/// This never walks the filesystem or reads user file contents.
pub fn browse_tree(db_path: &Path, request: TreeRequest) -> Result<TreePage, String> {
    let root_path = std::fs::canonicalize(&request.root)
        .map_err(|e| format!("Raiz indexada indisponível: {e}"))?;
    let root = root_path.to_string_lossy().into_owned();
    let parent = Path::new(&request.parent_path);
    if !parent.starts_with(&root_path) || request.parent_path.is_empty() {
        return Err("Diretório fora do escopo indexado.".into());
    }
    if let Some(cursor) = &request.after {
        if !matches!(cursor.kind.as_str(), "directory" | "file")
            || Path::new(&cursor.path).parent() != Some(parent) {
            return Err("Cursor de navegação inválido.".into());
        }
    }
    let limit = request.limit.unwrap_or(TREE_PAGE_DEFAULT).clamp(1,TREE_PAGE_MAX);
    if !db_path.is_file() {
        return Err("Nenhum índice SQLite encontrado. Use Atualizar índice.".into());
    }
    // Direct-child navigation must not execute schema migrations or CREATE
    // INDEX checks on each click. A published index can be read in SQLite WAL
    // mode with a read-only connection.
    let mut conn = Connection::open_with_flags(
        db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    ).map_err(db_err)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(db_err)?;
    // SQLite WAL snapshot: the generation, folder children and file children
    // must all refer to the same committed index version.
    let tx = conn.transaction().map_err(db_err)?;
    let (generation,completed_at_unix): (i64,i64) = tx.query_row(
        "SELECT generation,completed_at_unix FROM indexed_scopes WHERE root=?1",
        params![root], |r| Ok((r.get(0)?,r.get(1)?))
    ).optional().map_err(db_err)?
        .ok_or("Nenhum índice publicado para esta pasta. Use Atualizar índice SQLite.")?;
    if request.generation.is_some_and(|value| value != generation) {
        return Err("O índice foi atualizado. Reabra a árvore para carregar a geração atual.".into());
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM indexed_directories
         WHERE root=?1 AND path=?2 AND generation=?3)",
        params![root,request.parent_path,generation],
        |r| r.get(0),
    ).map_err(db_err)?;
    if !exists {
        return Err("Árvore indisponível para esta pasta. Atualize o índice SQLite.".into());
    }
    let mut nodes = Vec::<TreeNode>::with_capacity(limit+1);
    if request.after.as_ref().is_none_or(|cursor| cursor.kind == "directory") {
        let after_size = request.after.as_ref().map(|c| c.size_bytes.min(i64::MAX as u64) as i64);
        let after_path = request.after.as_ref().map(|c| c.path.as_str()).unwrap_or("");
        let mut stmt = tx.prepare(
            "SELECT path,logical_bytes,file_count FROM indexed_directories
             WHERE root=?1 AND parent_path=?2 AND generation=?3 AND path<>?4
             AND (?5 IS NULL OR logical_bytes<?5 OR (logical_bytes=?5 AND path>?6))
             ORDER BY logical_bytes DESC,path ASC LIMIT ?7"
        ).map_err(db_err)?;
        let mut rows = stmt.query(params![
            root,request.parent_path,generation,root,after_size,after_path,
            (limit+1) as i64,
        ]).map_err(db_err)?;
        while let Some(row) = rows.next().map_err(db_err)? {
            let path: String = row.get(0).map_err(db_err)?;
            let logical: i64 = row.get(1).map_err(db_err)?;
            let files: i64 = row.get(2).map_err(db_err)?;
            let name = Path::new(&path).file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            nodes.push(TreeNode {
                path,name,kind:"directory".into(),
                size_bytes:logical.max(0) as u64,
                files:files.max(0) as u64,
                content_status:"local".into(),
            });
        }
    }
    if nodes.len() <= limit {
        let after_file = request.after.as_ref().filter(|c| c.kind == "file");
        let after_size = after_file.map(|c| c.size_bytes.min(i64::MAX as u64) as i64);
        let after_path = after_file.map(|c| c.path.as_str()).unwrap_or("");
        let fetch = limit+1-nodes.len();
        let mut stmt = tx.prepare(
            "SELECT path,size_bytes,attributes FROM indexed_files
             WHERE root=?1 AND parent_path=?2 AND generation=?3
             AND (?4 IS NULL OR size_bytes<?4 OR (size_bytes=?4 AND path>?5))
             ORDER BY size_bytes DESC,path ASC LIMIT ?6"
        ).map_err(db_err)?;
        let mut rows = stmt.query(params![
            root,request.parent_path,generation,after_size,after_path,fetch as i64
        ]).map_err(db_err)?;
        while let Some(row) = rows.next().map_err(db_err)? {
            let path: String = row.get(0).map_err(db_err)?;
            let size: i64 = row.get(1).map_err(db_err)?;
            let attr: i64 = row.get(2).map_err(db_err)?;
            let name = Path::new(&path).file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let status = if attr & (0x1000 | 0x40000 | 0x400000) != 0 {
                "offline"
            } else if attr & 0x400 != 0 { "reparse" } else { "local" };
            nodes.push(TreeNode {
                path,name,kind:"file".into(),
                size_bytes:size.max(0) as u64,
                files:1,content_status:status.into(),
            });
        }
    }
    let has_more = nodes.len()>limit;
    nodes.truncate(limit);
    let next_cursor = if has_more {
        nodes.last().map(|last| TreeCursor {
            kind:last.kind.clone(),size_bytes:last.size_bytes,path:last.path.clone()
        })
    } else { None };
    tx.commit().map_err(db_err)?;
    Ok(TreePage {
        root,parent_path:request.parent_path,nodes,next_cursor,
        completed_at_unix,generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree_req(root: &Path, parent: &Path, limit: usize, after: Option<TreeCursor>,
        generation: Option<i64>) -> TreeRequest {
        TreeRequest {
            root:root.display().to_string(),parent_path:parent.display().to_string(),
            after,limit:Some(limit),generation,
        }
    }

    #[test]
    fn tree_browsing_without_index_is_read_only_and_does_not_create_db() {
        let tmp=tempfile::tempdir().unwrap();
        let root=tmp.path().join("scope");
        fs::create_dir(&root).unwrap();
        let db=tmp.path().join("not-created.sqlite");
        assert!(browse_tree(&db,tree_req(&root,&root,100,None,None)).is_err());
        assert!(!db.exists());
    }

    #[test]
    fn existing_sqlite_index_migrates_parent_columns_without_corrupting_old_files() {
        let tmp=tempfile::tempdir().unwrap();
        let root=tmp.path().join("scope");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep.txt"),b"hello").unwrap();
        let db=tmp.path().join("legacy.sqlite");
        let legacy=Connection::open(&db).unwrap();
        legacy.execute_batch(
            "CREATE TABLE indexed_scopes(
                root TEXT PRIMARY KEY,generation INTEGER NOT NULL,
                completed_at_unix INTEGER NOT NULL,file_count INTEGER NOT NULL);
             CREATE TABLE indexed_files(
                root TEXT NOT NULL,path TEXT NOT NULL,size_bytes INTEGER NOT NULL,
                modified_ns INTEGER NOT NULL,attributes INTEGER NOT NULL,
                generation INTEGER NOT NULL,PRIMARY KEY(root,path));
             CREATE TABLE indexed_stage(
                root TEXT NOT NULL,path TEXT NOT NULL,size_bytes INTEGER NOT NULL,
                modified_ns INTEGER NOT NULL,attributes INTEGER NOT NULL,
                PRIMARY KEY(root,path));"
        ).unwrap();
        legacy.execute(
            "INSERT INTO indexed_scopes VALUES(?1,1,123,1)",
            params![root.display().to_string()],
        ).unwrap();
        legacy.execute(
            "INSERT INTO indexed_files VALUES(?1,?2,5,0,0,1)",
            params![root.display().to_string(),root.join("keep.txt").display().to_string()],
        ).unwrap();
        drop(legacy);
        let upgraded=connection(&db).unwrap();
        let prior: i64=upgraded.query_row(
            "SELECT COUNT(*) FROM indexed_files WHERE size_bytes=5",
            [],|r| r.get(0),
        ).unwrap();
        assert_eq!(prior,1);
        drop(upgraded);
        // Existing file rows have no derived directory tree until reindexed.
        assert!(browse_tree(&db,tree_req(&root,&root,100,None,None)).is_err());
        refresh(&db,root.to_str().unwrap(),None,&AtomicBool::new(false),
            &AtomicBool::new(false), |_| {}).unwrap();
        let page=browse_tree(&db,tree_req(&root,&root,100,None,None)).unwrap();
        assert_eq!(page.nodes.len(),1);
        assert_eq!(page.nodes[0].name,"keep.txt");
    }

    #[test]
    fn lazy_tree_paginates_direct_children_with_files_after_directories() {
        let tmp=tempfile::tempdir().unwrap();
        let root=tmp.path().join("scope");
        let sub=root.join("sub");
        let empty=root.join("empty");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(&empty).unwrap();
        fs::write(sub.join("a"),b"abc").unwrap();
        fs::write(sub.join("b"),b"abcd").unwrap();
        fs::write(root.join("one"),b"1").unwrap();
        fs::write(root.join("two"),b"22").unwrap();
        let db=tmp.path().join("index.sqlite");
        refresh(&db,root.to_str().unwrap(),None,&AtomicBool::new(false),
            &AtomicBool::new(false), |_| {}).unwrap();
        let mut cursor=None;
        let mut results=Vec::new();
        let mut generation=None;
        loop {
            let page=browse_tree(&db,tree_req(&root,&root,1,cursor,generation)).unwrap();
            generation=Some(page.generation);
            results.extend(page.nodes.iter().map(|n| (n.kind.clone(),n.name.clone())));
            cursor=page.next_cursor;
            if cursor.is_none() { break; }
            assert!(results.len()<20,"pagination cursor did not advance");
        }
        assert_eq!(results.len(),4);
        assert!(results[0].0=="directory" && results[1].0=="directory");
        assert!(results[2].0=="file" && results[3].0=="file");
        let subpage=browse_tree(&db,tree_req(&root,&sub,100,None,None)).unwrap();
        assert_eq!(subpage.nodes.len(),2);
        let emptypage=browse_tree(&db,tree_req(&root,&empty,100,None,None)).unwrap();
        assert!(emptypage.nodes.is_empty());
    }

    #[test]
    fn tree_counts_nested_bytes_and_refuses_out_of_scope_or_invalid_cursor() {
        let tmp=tempfile::tempdir().unwrap();
        let root=tmp.path().join("scope");
        let sub=root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("a"),b"1234").unwrap();
        fs::write(root.join("file"),b"123").unwrap();
        let db=tmp.path().join("index.sqlite");
        refresh(&db,root.to_str().unwrap(),None,&AtomicBool::new(false),
            &AtomicBool::new(false), |_| {}).unwrap();
        let first=browse_tree(&db,tree_req(&root,&root,100,None,None)).unwrap();
        assert_eq!(first.nodes[0].kind,"directory");
        assert_eq!(first.nodes[0].size_bytes,4);
        assert_eq!(first.nodes[0].files,1);
        assert!(browse_tree(&db,tree_req(&root,tmp.path(),100,None,None)).is_err());
        assert!(browse_tree(&db,tree_req(&root,&root,1,Some(TreeCursor {
            kind:"file".into(),size_bytes:3,path:tmp.path().join("outside").display().to_string(),
        }),None)).is_err());
    }

    #[test]
    fn canceled_refresh_preserves_tree_and_stale_cursor_is_rejected() {
        let tmp=tempfile::tempdir().unwrap();
        let root=tmp.path().join("scope");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("before"),b"old").unwrap();
        let db=tmp.path().join("index.sqlite");
        refresh(&db,root.to_str().unwrap(),None,&AtomicBool::new(false),
            &AtomicBool::new(false), |_| {}).unwrap();
        let page=browse_tree(&db,tree_req(&root,&root,100,None,None)).unwrap();
        let cancel=AtomicBool::new(true);
        fs::write(root.join("new"),b"new").unwrap();
        assert!(refresh(&db,root.to_str().unwrap(),None,&cancel,
            &AtomicBool::new(false), |_| {}).is_err());
        let after=browse_tree(&db,tree_req(&root,&root,100,None,None)).unwrap();
        assert_eq!(page.generation,after.generation);
        assert_eq!(after.nodes.len(),1);
        cancel.store(false,Ordering::Relaxed);
        refresh(&db,root.to_str().unwrap(),None,&cancel,&AtomicBool::new(false), |_| {}).unwrap();
        assert!(browse_tree(&db,tree_req(&root,&root,100,None,Some(page.generation))).is_err());
        let updated=browse_tree(&db,tree_req(&root,&root,100,None,None)).unwrap();
        assert_eq!(updated.nodes.len(),2);
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
        let staging: i64 = conn.query_row("SELECT COUNT(*) FROM indexed_stage", [], |r| r.get(0)).unwrap();
        assert_eq!(live, 1035);
        assert_eq!(staging, 0);
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
