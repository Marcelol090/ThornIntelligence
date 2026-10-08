//! Reversible, non-destructive Windows quarantine.
//!
//! No permanent-delete API exists. Only same-volume MoveFileExW moves without
//! COPY_ALLOWED or REPLACE_EXISTING. No automatic candidates from the index.
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const PREVIEW_TTL: Duration = Duration::from_secs(5 * 60);
const CONFIRM_MOVE: &str = "MOVER PARA QUARENTENA";
const CONFIRM_RESTORE: &str = "RESTAURAR";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantinePreview {
    pub preview_id: String,
    pub path: String,
    pub size_bytes: u64,
    pub warning: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineItem {
    pub id: String,
    pub original_path: String,
    pub size_bytes: u64,
    pub created_at_unix: i64,
    pub status: String,
}

#[derive(Clone)]
struct Preview {
    path: PathBuf,
    size: u64,
    modified_ns: i64,
    identity: u128,
    issued: Instant,
}

#[derive(Default)]
pub struct QuarantineGate {
    previews: Mutex<HashMap<String, Preview>>,
}

fn stamp() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .unwrap_or_default().as_secs().min(i64::MAX as u64) as i64
}
fn modified_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn protect_error() -> String {
    "Arquivo protegido, link, pasta sincronizada ou metadados inseguros: operação bloqueada.".into()
}

fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(windows)]
fn checked_ancestors(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    // Reparse or symlinked parent directories could redirect paths between
    // preview and move. Guard known links; hostile concurrent mutation is
    // not fully solved by the current path-based Win32 API.
    for parent in path.ancestors().skip(1) {
        let metadata = std::fs::symlink_metadata(parent).map_err(|_| protect_error())?;
        if metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0 {
            return Err(protect_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn safe_metadata(path: &Path, app_data: &Path, managed_restore: bool) -> Result<(u64, i64, u128), String> {
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    if !path.is_absolute() { return Err(protect_error()); }
    checked_ancestors(path)?;
    let meta = std::fs::symlink_metadata(path).map_err(|_| protect_error())?;
    const REJECT: u32 = 0x400 | 0x1000 | 0x40000 | 0x400000 | 0x4;
    if !meta.is_file() || meta.file_type().is_symlink() || (meta.file_attributes() & REJECT) != 0 {
        return Err(protect_error());
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| protect_error())?;
    if managed_restore {
        if !is_within(&canonical, &app_data.join("quarantine")) { return Err(protect_error()); }
    } else if is_within(&canonical, app_data) { return Err(protect_error()); }
    if !managed_restore { for name in [
        "SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)",
        "ProgramData", "APPDATA", "LOCALAPPDATA",
        "OneDrive", "OneDriveConsumer", "OneDriveCommercial"
    ] {
        if let Some(folder) = std::env::var_os(name) {
            if let Ok(protected) = std::fs::canonicalize(PathBuf::from(folder)) {
                if is_within(&canonical, &protected) {
                    // Allow user-requested cleanup of their own temp files,
                    // but not the rest of LocalAppData (browser/app state).
                    let allowed_temp = name == "LOCALAPPDATA"
                        && canonical.starts_with(protected.join("Temp"));
                    if !allowed_temp { return Err(protect_error()); }
                }
            }
        }
    }}
    // Open only after rejecting cloud/offline attributes. Obtains a stable
    // physical identity and link count, but does not read file contents.
    let file = std::fs::File::open(&canonical).map_err(|_| protect_error())?;
    let opened = file.metadata().map_err(|_| protect_error())?;
    if opened.len() != meta.len() || modified_ns(&opened) != modified_ns(&meta) ||
        opened.file_attributes() & REJECT != 0 {
        return Err(protect_error());
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandle(file.as_raw_handle(), &mut info)
    };
    if ok == 0 || info.nNumberOfLinks != 1 { return Err(protect_error()); }
    let identity = (u128::from(info.dwVolumeSerialNumber) << 64)
        | (u128::from(info.nFileIndexHigh) << 32)
        | u128::from(info.nFileIndexLow);
    Ok((meta.len(), modified_ns(&meta), identity))
}

#[cfg(not(windows))]
fn safe_metadata(_: &Path, _: &Path, _: bool) -> Result<(u64,i64,u128),String> {
    Err("Quarentena real disponível somente no Windows.".into())
}

fn db(app_data: &Path) -> Result<rusqlite::Connection, String> {
    let conn = crate::index::connection(&app_data.join("storage-index.sqlite"))?;
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS quarantine_entries(
          id TEXT PRIMARY KEY,
          original_path TEXT NOT NULL,
          size_bytes INTEGER NOT NULL,
          modified_ns INTEGER NOT NULL,
          identity_hex TEXT NOT NULL,
          created_at_unix INTEGER NOT NULL,
          state TEXT NOT NULL CHECK(state IN ('prepared','quarantined','restored','failed'))
        );
    ").map_err(|e| format!("Manifesto da quarentena indisponível: {e}"))?;
    Ok(conn)
}

pub fn preview(path: String, app_data: &Path, gate: &QuarantineGate)
    -> Result<QuarantinePreview, String> {
    let source = PathBuf::from(path);
    let (size, modified, identity) = safe_metadata(&source, app_data, false)?;
    let id = Uuid::new_v4().to_string();
    let mut pending = gate.previews.lock().map_err(|_| "Pré-visualização indisponível.")?;
    pending.retain(|_,entry| entry.issued.elapsed() <= PREVIEW_TTL);
    if pending.len() > 64 { pending.clear(); }
    pending.insert(id.clone(), Preview {
        path: source.clone(), size, modified_ns: modified, identity, issued: Instant::now(),
    });
    Ok(QuarantinePreview {
        preview_id: id,
        path: source.to_string_lossy().into_owned(),
        size_bytes: size,
        warning: "Movimentação no mesmo volume: não libera espaço físico. \
                  Não equivale a backup; arquivos sincronizados, sistema, hardlinks e \
                  conteúdo em nuvem são bloqueados. Confirme apenas após revisar.".into(),
    })
}

#[cfg(windows)]
fn move_same_volume_no_replace(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let src: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
    // Flags 0: no COPY_ALLOWED, no REPLACE_EXISTING, no delayed delete.
    let ok = unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), 0) };
    if ok == 0 {
        Err(format!("Não foi possível mover com segurança, sem sobrescrever e no mesmo volume: {}",
            std::io::Error::last_os_error()))
    } else { Ok(()) }
}
#[cfg(not(windows))]
fn move_same_volume_no_replace(_: &Path, _: &Path) -> Result<(), String> {
    Err("Quarentena real disponível somente no Windows.".into())
}

pub fn quarantine(
    preview_id: String, confirmation: String, app_data: &Path, gate: &QuarantineGate,
) -> Result<QuarantineItem, String> {
    if confirmation != CONFIRM_MOVE {
        return Err("Confirmação explícita obrigatória: MOVER PARA QUARENTENA.".into());
    }
    // Consume the approved preview before moving anything. Never accept an
    // arbitrary path directly in a mutating command.
    let pending = gate.previews.lock().map_err(|_| "Pré-visualização indisponível.")?
        .remove(&preview_id).ok_or("Pré-visualização inexistente ou já utilizada.")?;
    if pending.issued.elapsed() > PREVIEW_TTL {
        return Err("Pré-visualização expirada. Revise o arquivo novamente.".into());
    }
    let (size, modified, identity) = safe_metadata(&pending.path, app_data, false)?;
    if (size, modified, identity) != (pending.size, pending.modified_ns, pending.identity) {
        return Err("Arquivo alterado após a pré-visualização. Nenhuma movimentação executada.".into());
    }
    let quarantine_root = app_data.join("quarantine");
    std::fs::create_dir_all(&quarantine_root)
        .map_err(|e| format!("Não foi possível criar a quarentena local: {e}"))?;
    let target = quarantine_root.join(&preview_id);
    if target.exists() { return Err("Destino já ocupado; nenhuma movimentação executada.".into()); }

    let mut conn = db(app_data)?;
    // Commit an intent BEFORE rename. After a crash, a prepared record with
    // target present is recoverable by the restore/list path.
    conn.execute("INSERT INTO quarantine_entries
        (id, original_path, size_bytes, modified_ns, identity_hex, created_at_unix, state)
        VALUES (?1,?2,?3,?4,?5,?6,'prepared')",
        params![
            preview_id, pending.path.to_string_lossy().as_ref(),
            i64::try_from(size).map_err(|_| "Arquivo grande demais.")?,
            modified, identity.to_string(), stamp(),
        ]).map_err(|e| format!("Não foi possível registrar a intenção de quarentena: {e}"))?;

    if let Err(error) = move_same_volume_no_replace(&pending.path, &target) {
        let _ = conn.execute("UPDATE quarantine_entries SET state='failed' WHERE id=?1",
            params![preview_id]);
        return Err(error);
    }
    // If this DB update fails, 'prepared' plus an existing target remains
    // discoverable and restorable after relaunch; do not move the file back
    // without a fresh explicit restore command.
    conn.execute("UPDATE quarantine_entries SET state='quarantined' WHERE id=?1",
        params![preview_id]).map_err(|e| format!(
            "Arquivo movido, mas manifesto requer recuperação: {e}"
        ))?;
    read_item(&conn, &preview_id)?.ok_or("Registro recém-criado ausente.".into())
}

fn read_item(conn: &rusqlite::Connection, id: &str)
    -> Result<Option<QuarantineItem>, String> {
    conn.query_row(
        "SELECT id,original_path,size_bytes,created_at_unix,state
         FROM quarantine_entries WHERE id=?1",
        params![id],
        |row| Ok(QuarantineItem {
            id: row.get(0)?, original_path: row.get(1)?,
            size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
            created_at_unix: row.get(3)?, status: row.get(4)?,
        })
    ).optional().map_err(|e| e.to_string())
}

pub fn list(app_data: &Path) -> Result<Vec<QuarantineItem>, String> {
    let conn = db(app_data)?;
    let mut stmt = conn.prepare(
        "SELECT id FROM quarantine_entries
         WHERE state IN ('prepared','quarantined') ORDER BY created_at_unix DESC LIMIT 500"
    ).map_err(|e| e.to_string())?;
    let ids = stmt.query_map([], |row| row.get::<_,String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>,_>>().map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for id in ids {
        // Never display 'prepared' entries that were never moved.
        if app_data.join("quarantine").join(&id).is_file() {
            if let Some(item) = read_item(&conn, &id)? { result.push(item); }
        }
    }
    Ok(result)
}

pub fn restore(id: String, confirmation: String, app_data: &Path)
    -> Result<QuarantineItem, String> {
    if confirmation != CONFIRM_RESTORE { return Err("Digite RESTAURAR para confirmar.".into()); }
    let conn = db(app_data)?;
    let item = read_item(&conn, &id)?.ok_or("Item desconhecido.")?;
    if item.status != "prepared" && item.status != "quarantined" {
        return Err("Este item não está mais em quarentena.".into());
    }
    // Validate the untrusted-looking ID even though it originated from UUID.
    let parsed = Uuid::parse_str(&id).map_err(|_| "Identificador inválido.")?;
    if parsed.to_string() != id { return Err("Identificador inválido.".into()); }
    let source = app_data.join("quarantine").join(&id);
    let original = PathBuf::from(&item.original_path);
    if !source.is_file() { return Err("Arquivo da quarentena ausente.".into()); }
    // Avoid reparse destinations, accidental overwrite or recreation of a
    // deleted parent. The source remains untouched on a collision.
    if std::fs::symlink_metadata(&original).is_ok() {
        return Err("O destino original já existe: restauração bloqueada.".into());
    }
    #[cfg(windows)]
    {
        checked_ancestors(&original)?;
        checked_ancestors(&source)?;
        let parent = original.parent().ok_or_else(protect_error)?;
        let parent = std::fs::canonicalize(parent).map_err(|_| protect_error())?;
        if parent.starts_with(app_data) { return Err(protect_error()); }
        for name in [
            "SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)",
            "ProgramData", "APPDATA", "LOCALAPPDATA",
            "OneDrive", "OneDriveConsumer", "OneDriveCommercial"
        ] {
            if let Some(folder) = std::env::var_os(name) {
                if let Ok(protected) = std::fs::canonicalize(PathBuf::from(folder)) {
                    if parent.starts_with(&protected) {
                        let allowed_temp = name == "LOCALAPPDATA" &&
                            parent.starts_with(protected.join("Temp"));
                        if !allowed_temp { return Err(protect_error()); }
                    }
                }
            }
        }
    }
    let (size, _modified, identity) = safe_metadata(&source, app_data, true)?;
    let expected_identity: u128 = conn.query_row(
        "SELECT identity_hex FROM quarantine_entries WHERE id=?1", params![id],
        |row| row.get::<_,String>(0)
    ).map_err(|e| e.to_string())?.parse()
        .map_err(|_| "Identidade inválida no manifesto.")?;
    if size != item.size_bytes || identity != expected_identity {
        return Err("Identidade ou tamanho diferente do manifesto: restauração bloqueada.".into());
    }
    move_same_volume_no_replace(&source, &original)?;
    conn.execute("UPDATE quarantine_entries SET state='restored' WHERE id=?1",
        params![id]).map_err(|e| format!(
            "Arquivo restaurado, mas o manifesto necessita reconciliação: {e}"
        ))?;
    read_item(&conn, &id)?.ok_or("Registro ausente após restauração.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_is_required_before_any_mutation() {
        let gate = QuarantineGate::default();
        let dir = tempfile::tempdir().unwrap();
        assert!(quarantine("x".into(), "SIM".into(), dir.path(), &gate).is_err());
        assert!(restore("x".into(), "SIM".into(), dir.path()).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn windows_quarantine_restore_round_trip_and_no_overwrite() {
        let app = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("temporary.log");
        std::fs::write(&source, "recover me").unwrap();
        let gate = QuarantineGate::default();
        let approved = preview(source.display().to_string(), app.path(), &gate).unwrap();
        let item = quarantine(
            approved.preview_id, CONFIRM_MOVE.into(), app.path(), &gate
        ).unwrap();
        assert!(!source.exists());
        assert_eq!(list(app.path()).unwrap().len(), 1);
        std::fs::write(&source, "collision").unwrap();
        assert!(restore(item.id.clone(), CONFIRM_RESTORE.into(), app.path()).is_err());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "collision");
        std::fs::remove_file(&source).unwrap();
        let restored = restore(item.id, CONFIRM_RESTORE.into(), app.path()).unwrap();
        assert_eq!(restored.status, "restored");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "recover me");
        assert!(list(app.path()).unwrap().is_empty());
    }

    #[test]
    fn preview_ids_cannot_be_reused_or_spoofed() {
        let gate = QuarantineGate::default();
        let dir = tempfile::tempdir().unwrap();
        assert!(quarantine(Uuid::new_v4().to_string(), CONFIRM_MOVE.into(),
                           dir.path(), &gate).is_err());
    }
}
