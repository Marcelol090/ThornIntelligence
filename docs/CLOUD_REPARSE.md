# Windows cloud reparse points

Problem: the initial SQLite index rejected every reparse point. OneDrive Files On-Demand uses reparse points for ordinary cloud-backed files and directories; rejecting them makes metadata disappear from the index.

This patch adds a tag-aware Windows classifier. FindFirstFileW exposes the tag through WIN32_FIND_DATAW.dwReserved0. The 16 CLOUD family tags are allowed for metadata-only indexing. Junctions, symbolic links and unknown tags remain excluded. Content streams are never opened by the classifier. If classification fails, the index transaction is not committed.

The selected root is classified before canonicalization to reject redirected roots. The index still represents logical sizes, not allocated physical bytes. Cloud content is not hydrated, and no deletion decision may be made from this metadata.

Tests cover all 16 CLOUD tags and reject common redirect tags. Unix regression verifies that a redirected root cannot be indexed and that external symlink content is ignored.

Windows integration tests with OneDrive Files On-Demand are still required. No destructive operations are included.

Sources:
- https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-point-tags
- https://github.com/BurntSushi/ripgrep/issues/705
- https://github.com/rust-lang/rust/issues/121189
