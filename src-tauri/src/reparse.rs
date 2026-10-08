//! Classify Windows reparse entries without opening their file contents.
//!
//! A reparse point is NOT necessarily a symlink. Cloud Files API placeholders
//! use non-surrogate tags; discarding all reparse directories silently hides
//! OneDrive trees. Only recognized CLOUD tags are safe for metadata indexing.
//! All other reparse tags are conservatively excluded from traversal.
use std::fs::Metadata;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReparseKind {
    Ordinary,
    CloudPlaceholder,
    RedirectedOrUnknown,
}

const REPARSE_POINT: u32 = 0x0000_0400;
const CLOUD_TAG: u32 = 0x9000_001A;
const CLOUD_TAG_MASK: u32 = 0xFFFF_0FFF;

// IO_REPARSE_TAG_CLOUD through IO_REPARSE_TAG_CLOUD_F: the provider index
// occupies bits 12..15. Junctions and symlinks have different tags.
fn classify_tag(attributes: u32, tag: u32) -> ReparseKind {
    if attributes & REPARSE_POINT == 0 {
        ReparseKind::Ordinary
    } else if (tag & CLOUD_TAG_MASK) == CLOUD_TAG {
        ReparseKind::CloudPlaceholder
    } else {
        ReparseKind::RedirectedOrUnknown
    }
}

#[cfg(windows)]
pub(crate) fn classify(path: &Path, metadata: &Metadata) -> Result<ReparseKind, String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileW, WIN32_FIND_DATAW,
    };

    let attributes = metadata.file_attributes();
    if attributes & REPARSE_POINT == 0 {
        return Ok(ReparseKind::Ordinary);
    }
    // FindFirstFileW returns the tag in dwReserved0 for reparse points,
    // without opening the target's content stream (Microsoft Win32 docs).
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe { FindFirstFileW(wide.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Não foi possível classificar reparse point sem ler conteúdo {}: {}",
            path.display(), std::io::Error::last_os_error()
        ));
    }
    unsafe { FindClose(handle); }
    // Abort rather than trust a stale classification after a concurrent
    // replacement. Snapshot transaction will roll back on this error.
    if data.dwFileAttributes & REPARSE_POINT == 0 {
        return Err(format!(
            "Atributos reparse mudaram durante a indexação: {}",
            path.display()
        ));
    }
    Ok(classify_tag(data.dwFileAttributes, data.dwReserved0))
}

#[cfg(not(windows))]
pub(crate) fn classify(_path: &Path, metadata: &Metadata) -> Result<ReparseKind, String> {
    if metadata.file_type().is_symlink() {
        Ok(ReparseKind::RedirectedOrUnknown)
    } else {
        Ok(ReparseKind::Ordinary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_all_cloud_tags_without_accepting_junctions() {
        assert_eq!(classify_tag(0, 0x9000_001A), ReparseKind::Ordinary);
        for provider in 0..=15u32 {
            let tag = CLOUD_TAG | (provider << 12);
            assert_eq!(classify_tag(REPARSE_POINT, tag), ReparseKind::CloudPlaceholder);
        }
        for tag in [
            0xA000_0003, // mount point/junction (name surrogate)
            0xA000_000C, // symbolic link (name surrogate)
            0x8000_0008, // WIM
            0x8000_0021, // older OneDrive tag: unknown -> conservative
            0,
        ] {
            assert_eq!(classify_tag(REPARSE_POINT, tag), ReparseKind::RedirectedOrUnknown);
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_symlink_is_not_treated_as_local_content() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target"), "data").unwrap();
        let link = dir.path().join("alias");
        symlink(dir.path().join("target"), &link).unwrap();
        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert_eq!(
            classify(&link, &metadata).unwrap(),
            ReparseKind::RedirectedOrUnknown
        );
    }
}
