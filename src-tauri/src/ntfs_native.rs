//! Conservative NTFS acceleration: read-only MFT enumeration and USN cursor.
//! Fail closed to WalkDir if complete filename coverage cannot be established.
//!
//! Requirements: local drive-letter NTFS volume and permission to read the
//! volume handle. No journal is created/modified; no user content is opened.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

const FSCTL_ENUM_USN_DATA: u32 = 0x000900b3;
const FSCTL_QUERY_USN_JOURNAL: u32 = 0x000900f4;
const FSCTL_READ_USN_JOURNAL: u32 = 0x000900bb;
const GENERIC_READ: u32 = 0x80000000;
const FILE_SHARE_ALL: u32 = 0x00000007;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x00000010;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x00001000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
const ERROR_HANDLE_EOF: u32 = 38;
const MAX_ENUM_RECORDS: usize = 2_000_000;
const BUFFER_BYTES: usize = 256 * 1024;
const USN_REASON_CLOSE: u32 = 0x80000000;

#[repr(C)]
struct MftEnumDataV0 { start_file_reference_number: u64, low_usn: i64, high_usn: i64 }
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JournalDataV0 {
    journal_id: u64, first_usn: i64, next_usn: i64,
    lowest_valid_usn: i64, max_usn: i64,
    maximum_size: u64, allocation_delta: u64,
}
#[repr(C)]
struct ReadUsnDataV0 {
    start_usn: i64, reason_mask: u32, return_only_on_close: u32,
    timeout: u64, bytes_to_wait_for: u64, journal_id: u64,
}
#[repr(C)]
#[derive(Default)]
struct ByHandleFileInformation {
    attributes: u32, creation: [u32;2], access: [u32;2], write: [u32;2],
    volume_serial: u32, file_size_high: u32, file_size_low: u32,
    number_of_links: u32, file_index_high: u32, file_index_low: u32,
}
#[link(name="kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16, access: u32, sharing: u32,
        security: *const c_void, creation: u32, flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn DeviceIoControl(
        handle: *mut c_void, control: u32,
        input: *const c_void, input_size: u32,
        output: *mut c_void, output_size: u32,
        bytes: *mut u32, overlapped: *mut c_void,
    ) -> i32;
    fn GetLastError() -> u32;
    fn GetVolumeInformationW(
        root: *const u16, name: *mut u16, name_size: u32,
        serial: *mut u32, max_component: *mut u32, fs_flags: *mut u32,
        fs_name: *mut u16, fs_name_size: u32,
    ) -> i32;
    fn GetFileInformationByHandle(handle: *mut c_void, data: *mut ByHandleFileInformation) -> i32;
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}
struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}
fn open_file(path: &Path, flags: u32) -> Result<Handle, String> {
    let name=wide(path.as_os_str());
    // Metadata-only handles. Never use FILE_FLAG_BACKUP_SEMANTICS without
    // FILE_FLAG_OPEN_REPARSE_POINT for local user paths.
    let h=unsafe { CreateFileW(
        name.as_ptr(), 0, FILE_SHARE_ALL, ptr::null(), OPEN_EXISTING,
        flags, ptr::null_mut(),
    ) };
    if h as isize == -1 { Err(format!("Open file metadata failed: {}", unsafe { GetLastError() })) }
    else { Ok(Handle(h)) }
}
fn drive(root: &Path) -> Option<char> {
    let text=root.to_string_lossy();
    let text=text.strip_prefix(r"\\?\").unwrap_or(&text);
    let bytes=text.as_bytes();
    if bytes.len()<3 || bytes[1]!=b':' || bytes[2]!=b'\\' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    Some((bytes[0] as char).to_ascii_uppercase())
}
fn open_ntfs_volume(root: &Path) -> Result<(Handle,char),String> {
    let drive=drive(root).ok_or("Somente volumes NTFS locais com letra são suportados.")?;
    let mount=format!("{drive}:\\");
    let mut fs=[0u16;64];
    let ok=unsafe { GetVolumeInformationW(
        wide(std::ffi::OsStr::new(&mount)).as_ptr(),ptr::null_mut(),0,
        ptr::null_mut(),ptr::null_mut(),ptr::null_mut(),
        fs.as_mut_ptr(),fs.len() as u32,
    ) };
    if ok==0 || String::from_utf16_lossy(&fs[..fs.iter().position(|c| *c==0).unwrap_or(fs.len())])!="NTFS" {
        return Err("Volume não é NTFS local ou está inacessível.".into());
    }
    let vol=format!(r"\\.\{drive}:");
    let name=wide(std::ffi::OsStr::new(&vol));
    let h=unsafe { CreateFileW(
        name.as_ptr(),GENERIC_READ,FILE_SHARE_ALL,ptr::null(),
        OPEN_EXISTING,0,ptr::null_mut(),
    ) };
    if h as isize==-1 { Err(format!("Volume NTFS indisponível (permissões): {}",unsafe { GetLastError() })) }
    else { Ok((Handle(h),drive)) }
}
fn ioctl<T>(handle: &Handle, code: u32, input: Option<&T>, output: &mut [u8]) -> Result<usize,u32> {
    let mut written=0u32;
    let (ptr,len)=if let Some(data)=input {
        (data as *const T as *const c_void,std::mem::size_of::<T>() as u32)
    } else { (ptr::null(),0) };
    let ok=unsafe { DeviceIoControl(
        handle.0,code,ptr,len,output.as_mut_ptr().cast(),
        output.len() as u32,&mut written,ptr::null_mut(),
    ) };
    if ok==0 { Err(unsafe { GetLastError() }) } else { Ok(written as usize) }
}
fn read_u16(bytes:&[u8],offset:usize)->Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset+2)?.try_into().ok()?))
}
fn read_u32(bytes:&[u8],offset:usize)->Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset+4)?.try_into().ok()?))
}
fn read_u64(bytes:&[u8],offset:usize)->Option<u64> {
    Some(u64::from_le_bytes(bytes.get(offset..offset+8)?.try_into().ok()?))
}
#[derive(Clone)]
struct MftRecord { parent:u64, name:OsString, attributes:u32 }
fn parse_mft_record(bytes:&[u8]) -> Result<(u64,MftRecord),String> {
    let len=read_u32(bytes,0).ok_or("Registro USN truncado")? as usize;
    if len!=bytes.len() || len<60 || read_u16(bytes,4)!=Some(2) {
        return Err("Formato USN V2 desconhecido; voltar para WalkDir".into());
    }
    let file_id=read_u64(bytes,8).ok_or("FRN inválido")?;
    let parent=read_u64(bytes,16).ok_or("Parent FRN inválido")?;
    let attributes=read_u32(bytes,52).ok_or("Atributos inválidos")?;
    let name_len=read_u16(bytes,56).ok_or("Nome inválido")? as usize;
    let name_at=read_u16(bytes,58).ok_or("Offset do nome inválido")? as usize;
    if name_len%2!=0 || name_at<60 || name_at.checked_add(name_len).is_none_or(|v|v>len) {
        return Err("Nome USN fora do registro".into());
    }
    let name:Vec<u16>=bytes[name_at..name_at+name_len].chunks_exact(2)
        .map(|c|u16::from_le_bytes([c[0],c[1]])).collect();
    let name=OsString::from_wide(&name);
    if name.is_empty() { return Err("Nome USN vazio".into()); }
    Ok((file_id,MftRecord{parent,name,attributes}))
}
fn records(buffer:&[u8], mut f:impl FnMut(&[u8])->Result<(),String>) -> Result<u64,String> {
    if buffer.len()<8 { return Err("Resposta FSCTL vazia".into()); }
    let next=read_u64(buffer,0).ok_or("Cursor FSCTL truncado")?;
    let mut pos=8;
    while pos<buffer.len() {
        let len=read_u32(buffer,pos).ok_or("Registro FSCTL truncado")? as usize;
        if len<60 || pos.checked_add(len).is_none_or(|x|x>buffer.len()) {
            return Err("Registro FSCTL malformado".into());
        }
        f(&buffer[pos..pos+len])?;
        pos+=len;
    }
    Ok(next)
}
fn journal(handle:&Handle)->Result<JournalDataV0,String> {
    let mut data=[0u8;64];
    let n=ioctl::<u8>(handle,FSCTL_QUERY_USN_JOURNAL,None,&mut data)
        .map_err(|e|format!("USN query: {e}"))?;
    if n<56 { return Err("USN journal header truncado".into()); }
    Ok(JournalDataV0{
        journal_id:read_u64(&data,0).unwrap(),
        first_usn:read_u64(&data,8).unwrap() as i64,
        next_usn:read_u64(&data,16).unwrap() as i64,
        lowest_valid_usn:read_u64(&data,24).unwrap() as i64,
        max_usn:read_u64(&data,32).unwrap() as i64,
        maximum_size:read_u64(&data,40).unwrap(),
        allocation_delta:read_u64(&data,48).unwrap(),
    })
}
#[derive(Debug,Clone,Copy)]
pub struct Cursor {
    pub journal_id:u64,pub first_usn:i64,pub next_usn:i64,
    pub volume_serial:u32,pub root_file_id:u64,
}
impl Cursor {
    pub fn retains(&self,old:&Self)->bool {
        self.journal_id==old.journal_id
            && self.volume_serial==old.volume_serial
            && self.root_file_id==old.root_file_id
            && old.next_usn>=self.first_usn
            && old.next_usn<=self.next_usn && self.first_usn>=0
    }
}
pub fn cursor(root:&Path)->Result<Cursor,String> {
    let (handle,_)=open_ntfs_volume(root)?;
    let j=journal(&handle)?;
    let root_info=file_info(root)?;
    Ok(Cursor {
        journal_id:j.journal_id,first_usn:j.first_usn.max(j.lowest_valid_usn),
        next_usn:j.next_usn,volume_serial:root_info.volume_serial,
        root_file_id:root_info.file_index(),
    })
}

/// Check every USN record in (previous watermark, bounded current watermark).
/// Return true only if ALL records have no change reason except CLOSE.
/// Any malformed record, wrap, reset or overflow forces full filesystem walk.
pub fn unchanged_since(root:&Path, old:Cursor) -> Result<bool,String> {
    let (handle,_)=open_ntfs_volume(root)?;
    let now=journal(&handle)?;
    let info=file_info(root)?;
    let current=Cursor {journal_id:now.journal_id,
        first_usn:now.first_usn.max(now.lowest_valid_usn),next_usn:now.next_usn,
        volume_serial:info.volume_serial,root_file_id:info.file_index()};
    if !current.retains(&old) { return Err("USN journal alterado, truncado ou substituído.".into()); }
    if current.next_usn==old.next_usn { return Ok(true); }
    let mut start=old.next_usn;
    let mut buffer=vec![0u8;BUFFER_BYTES];
    let mut count=0usize;
    while start<current.next_usn {
        let query=ReadUsnDataV0 {
            start_usn:start,reason_mask:u32::MAX,return_only_on_close:0,
            timeout:0,bytes_to_wait_for:0,journal_id:old.journal_id,
        };
        let read=ioctl(&handle,FSCTL_READ_USN_JOURNAL,Some(&query),&mut buffer)
            .map_err(|e|format!("USN read: {e}"))?;
        let mut relevant=false;
        let next=records(&buffer[..read],|record|{
            if read_u16(record,4)!=Some(2) { return Err("USN V3/format desconhecido".into()); }
            let reason=read_u32(record,40).ok_or("USN reason ausente")?;
            let usn=read_u64(record,24).ok_or("USN offset ausente")? as i64;
            if usn<current.next_usn && reason & !USN_REASON_CLOSE !=0 { relevant=true; }
            count+=1;
            if count>100_000 {return Err("Mudanças USN demais para a verificação incremental".into());}
            Ok(())
        })?;
        if relevant { return Ok(false); }
        let next=next as i64;
        if next<=start { return Err("Cursor USN não avançou".into()); }
        start=next;
    }
    Ok(true)
}

pub struct Enumeration { pub paths:Vec<(PathBuf,bool)>, pub checkpoint:Cursor }
/// MFT-based path enumeration. Only use when journal remains unchanged and
/// *every* file in requested subtree has one hardlink and valid metadata.
/// Native failure never publishes partial data; caller falls back to WalkDir.
pub fn enumerate(root:&Path,cancel:&AtomicBool)->Result<Enumeration,String> {
    let (handle,letter)=open_ntfs_volume(root)?;
    let beginning=journal(&handle)?;
    let root_info=file_info(root)?;
    let checkpoint=Cursor {
        journal_id:beginning.journal_id,
        first_usn:beginning.first_usn.max(beginning.lowest_valid_usn),
        next_usn:beginning.next_usn,
        volume_serial:root_info.volume_serial,root_file_id:root_info.file_index(),
    };
    let root_disk=PathBuf::from(format!(r"\\?\{letter}:\"));
    let mut next_frn=0u64;
    let mut entries=HashMap::<u64,MftRecord>::new();
    let mut buffer=vec![0u8;BUFFER_BYTES];
    loop {
        if cancel.load(Ordering::Relaxed) {return Err("Cancelado".into());}
        let input=MftEnumDataV0 { start_file_reference_number:next_frn,
            low_usn:0,high_usn:i64::MAX };
        let n=match ioctl(&handle,FSCTL_ENUM_USN_DATA,Some(&input),&mut buffer) {
            Ok(n)=>n,
            Err(ERROR_HANDLE_EOF)=>break,
            Err(error)=>return Err(format!("MFT indisponível: {error}")),
        };
        let mut count=0usize;
        let next=records(&buffer[..n],|record|{
            let (frn,rec)=parse_mft_record(record)?;
            if entries.insert(frn,rec).is_some() {
                return Err("FRN duplicado na enumeração".into());
            }
            count+=1;
            if entries.len()>MAX_ENUM_RECORDS {
                return Err("Enumeração MFT muito grande; fallback para WalkDir.".into());
            }
            Ok(())
        })?;
        if count==0 || next<=next_frn {
            return Err("Cursor da enumeração MFT não avançou".into());
        }
        next_frn=next;
    }
    // In NTFS, 5 is the root directory FRN. Verify it before using it.
    let disk_root=entries.get(&5).ok_or("Registro raiz NTFS ausente")?;
    if disk_root.attributes & FILE_ATTRIBUTE_DIRECTORY==0 {
        return Err("FRN raiz NTFS inválido".into());
    }
    fn build_path(
        id:u64, entries:&HashMap<u64,MftRecord>,disk_root:&Path,
        memo:&mut HashMap<u64,PathBuf>,walking:&mut HashSet<u64>,
    )->Result<PathBuf,String> {
        if id==5 {return Ok(disk_root.to_path_buf());}
        if let Some(p)=memo.get(&id) { return Ok(p.clone()); }
        if !walking.insert(id) || walking.len()>128 {
            return Err("Ciclo/profundidade MFT inválida".into());
        }
        let entry=entries.get(&id).ok_or("Pai FRN ausente")?;
        let parent=build_path(entry.parent,entries,disk_root,memo,walking)?;
        walking.remove(&id);
        if entry.name==std::ffi::OsStr::new(".") || entry.name==std::ffi::OsStr::new("..") {
            return Err("Nome MFT inválido".into());
        }
        let value=parent.join(&entry.name);
        memo.insert(id,value.clone());
        Ok(value)
    }
    let mut memo=HashMap::<u64,PathBuf>::new();
    let root_id=file_info(root)?.file_index();
    if root_id != 5 && !entries.contains_key(&root_id) {
        return Err("FRN do escopo não encontrado na MFT".into());
    }
    let resolved_root=build_path(root_id,&entries,&root_disk,&mut memo,&mut HashSet::new())?;
    if resolved_root!=root {
        return Err("Caminho da MFT e raiz canônica não coincidem".into());
    }
    // Build one set of reparse directories, not O(all MFT records) work
    // for every ancestor of every path.
    let mut reparse_ancestors=HashSet::<PathBuf>::new();
    for (&id,rec) in &entries {
        if rec.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 &&
           rec.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            let path=build_path(id,&entries,&root_disk,&mut memo,&mut HashSet::new())?;
            reparse_ancestors.insert(path);
        }
    }
    let mut output=Vec::<(PathBuf,bool)>::new();
    for (&id,rec) in &entries {
        if id==5 {continue;}
        if cancel.load(Ordering::Relaxed) {return Err("Cancelado".into());}
        let path=build_path(id,&entries,&root_disk,&mut memo,&mut HashSet::new())?;
        if !path.starts_with(root) || path==root { continue; }
        // Parent reparse points must never be descended through.
        let mut parent=path.parent();
        let mut unsafe_ancestor=false;
        while let Some(p)=parent {
            if !p.starts_with(root) {break;}
            if p==root {break;}
            if reparse_ancestors.contains(p) {unsafe_ancestor=true;break;}
            parent=p.parent();
        }
        if unsafe_ancestor {continue;}
        if rec.attributes & FILE_ATTRIBUTE_REPARSE_POINT!=0 {continue;}
        let is_dir=rec.attributes & FILE_ATTRIBUTE_DIRECTORY!=0;
        if !is_dir {
            if rec.attributes & FILE_ATTRIBUTE_OFFLINE !=0 {
                // Refuse MFT shortcut if identity cannot be verified without
                // potentially hydrating a cloud placeholder. WalkDir remains
                // metadata-only and knows every directory entry.
                return Err("Arquivo offline no escopo; fallback sem conteúdo.".into());
            }
            // MFT enumeration returns only one filename per FRN.
            // Hardlinks require name-aware WalkDir to avoid missing aliases.
            let info=file_info(&path)?;
            if info.number_of_links!=1 || info.file_index()!=id {
                return Err("Hardlink/FRN ambíguo; fallback completo.".into());
            }
        }
        output.push((path,is_dir));
    }
    let after=journal(&handle)?;
    if after.journal_id!=beginning.journal_id || after.next_usn!=beginning.next_usn {
        return Err("Volume alterado durante enumeração MFT; fallback.".into());
    }
    Ok(Enumeration {paths:output,checkpoint})
}
impl ByHandleFileInformation {
    fn file_index(&self)->u64 { ((self.file_index_high as u64)<<32) | self.file_index_low as u64 }
}
fn file_info(path:&Path)->Result<ByHandleFileInformation,String> {
    let h=open_file(path,FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_BACKUP_SEMANTICS)?;
    let mut info=ByHandleFileInformation::default();
    let ok=unsafe{GetFileInformationByHandle(h.0,&mut info)};
    if ok==0 { Err(format!("Falha na identidade NTFS: {}",unsafe { GetLastError() })) }
    else {Ok(info)}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_watermark_rejects_gaps_and_new_ids() {
        let old=Cursor{journal_id:7,first_usn:1,next_usn:120,
            volume_serial:55,root_file_id:500};
        let changed=|id,first,next,serial,root|Cursor{
            journal_id:id,first_usn:first,next_usn:next,
            volume_serial:serial,root_file_id:root};
        assert!(changed(7,100,130,55,500).retains(&old));
        assert!(!changed(8,100,130,55,500).retains(&old));
        assert!(!changed(7,121,130,55,500).retains(&old));
        assert!(!changed(7,100,119,55,500).retains(&old));
        assert!(!changed(7,100,130,56,500).retains(&old));
        assert!(!changed(7,100,130,55,501).retains(&old));
    }
    #[test]
    fn truncated_or_unknown_usn_records_are_rejected() {
        assert!(parse_mft_record(&[]).is_err());
        assert!(records(&[0;7], |_|Ok(())).is_err());
        let mut data=vec![0u8;70];
        data[0..4].copy_from_slice(&70u32.to_le_bytes());
        data[4..6].copy_from_slice(&3u16.to_le_bytes());
        assert!(parse_mft_record(&data).is_err());
    }
}
