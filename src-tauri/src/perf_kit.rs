//! Opt-in performance benchmarks for the real Rust scanner/indexer.
 //! Run only on expendable synthetic fixtures. No user data is modified.
use crate::index::{self, TreeRequest};
use crate::scan::{self, ScanRequest};
use crate::search::{self, SearchRequest};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn setting(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn fixture() -> (tempfile::TempDir, PathBuf, usize) {
    let count = setting("THORN_PERF_FILES", 1_500, 100, 120_000);
    let temporary = tempfile::tempdir().expect("create private benchmark directory");
    let root = temporary.path().join("scope");
    fs::create_dir(&root).unwrap();
    for i in 0..32 {
        fs::create_dir(root.join(format!("bucket-{i:02}"))).unwrap();
    }
    for i in 0..count {
        let folder = root.join(format!("bucket-{:02}", i % 32));
        let name = if i % 7 == 0 {
            format!("targetneedle-{i:08}.bin")
        } else {
            format!("other-{i:08}.bin")
        };
        // 1 in 41 files has identical head/tail but different middle:
        // partial fingerprints must never be used as duplicate proof.
        let bytes = if i % 41 == 0 {
            let mut value = vec![0x5Au8; 65_536];
            value[32_768..32_776].copy_from_slice(&(i as u64).to_le_bytes());
            value
        } else if i % 13 == 0 {
            vec![0x43u8; 256] // actual duplicates
        } else {
            let mut value = vec![0u8; 256];
            value[..8].copy_from_slice(&(i as u64).to_le_bytes());
            value
        };
        fs::write(folder.join(name), bytes).unwrap();
    }
    (temporary, root, count)
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let index = ((sorted.len() as f64 * quantile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn measure(mut action: impl FnMut(), repeats: usize) -> Vec<f64> {
    (0..repeats)
        .map(|_| {
            let start = Instant::now();
            action();
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect()
}

fn emit(scenario: &str, times: Vec<f64>, files: usize, details: Value) {
    assert!(!times.is_empty());
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    let entry = json!({
        "schema": 1,
        "scenario": scenario,
        "files": files,
        "iterations": times.len(),
        "samples_ms": times,
        "p50_ms": percentile(&sorted, 0.50),
        "p95_ms": percentile(&sorted, 0.95),
        "min_ms": sorted[0],
        "max_ms": sorted[sorted.len() - 1],
        "details": details
    });
    println!("THORN_PERF_METRIC {entry}");
}

#[test]
#[ignore = "explicit local performance run only; writes synthetic fixtures"]
fn perf_kit_scan_metadata_and_verified_blake3() {
    let (_temp, root, count) = fixture();
    let runs = setting("THORN_PERF_REPEAT", 5, 2, 30);
    let cancel = AtomicBool::new(false);
    let request = |duplicates| ScanRequest {
        root: root.to_string_lossy().into_owned(),
        regex: None,
        min_size_bytes: None,
        max_files: None,
        analyze_duplicates: Some(duplicates),
        hash_budget: Some("standard".into()),
    };
    let metadata = measure(
        || {
            let report =
                scan::scan_with_control(request(false), &cancel, |_| {}).unwrap();
            assert_eq!(report.files_scanned, count);
            assert!(report.hashing_skipped);
            assert_eq!(report.hash_bytes_read, 0);
        },
        runs,
    );
    emit("scan_metadata", metadata, count, json!({"hash":false}));
    let mut hash_bytes = 0;
    let hashing = measure(
        || {
            let report = scan::scan_with_control(request(true), &cancel, |_| {}).unwrap();
            assert_eq!(report.files_scanned, count);
            assert!(!report.hashing_skipped);
            assert!(report.duplicate_analysis_complete);
            assert!(!report.duplicates.is_empty());
            hash_bytes = report.hash_bytes_read;
        },
        runs,
    );
    emit("scan_blake3_verified", hashing, count, json!({"hash":true, "bytes_read":hash_bytes}));
}

#[test]
#[ignore = "explicit local performance run only; writes synthetic fixtures"]
fn perf_kit_search_walkdir_vs_sqlite_and_tree() {
    let (_temp, root, count) = fixture();
    let db = root.parent().unwrap().join("index.sqlite");
    let runs = setting("THORN_PERF_REPEAT", 5, 2, 30);
    let cancel = AtomicBool::new(false);
    let paused = AtomicBool::new(false);
    let expected = (0..count).filter(|n| n % 7 == 0).count();
    let request = || SearchRequest {
        root: root.to_string_lossy().into_owned(),
        regex: Some("targetneedle".into()),
        min_size_bytes: None,
        max_files: None,
    };
    let live = measure(
        || {
            let report = search::search_with_control(request(), &cancel, |_| {}).unwrap();
            assert_eq!(report.total_matches, expected);
            assert!(!report.truncated);
        },
        runs,
    );
    emit("search_walkdir_regex", live, count, json!({"matches":expected}));

    let started = Instant::now();
    let first = index::refresh(
        &db, root.to_str().unwrap(), None, &cancel, &paused, |_| {},
    ).unwrap();
    assert_eq!(first.files, count);
    emit(
        "index_initial", vec![started.elapsed().as_secs_f64() * 1000.0],
        count, json!({"method":first.index_method, "batches":first.batches_written}),
    );

    let mut strategy = String::new();
    let incremental = measure(
        || {
            let stats = index::refresh(
                &db, root.to_str().unwrap(), None, &cancel, &paused, |_| {},
            ).unwrap();
            assert_eq!(stats.files, count);
            strategy = stats.index_method;
        },
        runs,
    );
    emit("index_unchanged", incremental, count, json!({"method":strategy}));

    let indexed = measure(
        || {
            let response = index::search_index(&db, request()).unwrap();
            assert_eq!(response.report.total_matches, expected);
        },
        runs,
    );
    emit("search_sqlite_regex", indexed, count, json!({"matches":expected}));

    let browse = || TreeRequest {
        root: root.to_string_lossy().into_owned(),
        parent_path: root.to_string_lossy().into_owned(),
        after: None,
        limit: Some(16),
        generation: None,
    };
    let page = measure(
        || {
            let response = index::browse_tree(&db, browse()).unwrap();
            assert_eq!(response.nodes.len(), 16);
            assert!(response.next_cursor.is_some());
        },
        runs,
    );
    emit("tree_first_page", page, count, json!({"page_limit":16}));

    let mut pages = 0usize;
    let traversal = measure(
        || {
            let mut cursor = None;
            let mut generation = None;
            let mut children = 0;
            pages = 0;
            loop {
                let response = index::browse_tree(&db, TreeRequest {
                    root: root.to_string_lossy().into_owned(),
                    parent_path: root.to_string_lossy().into_owned(),
                    after: cursor,
                    limit: Some(16),
                    generation,
                }).unwrap();
                pages += 1;
                children += response.nodes.len();
                cursor = response.next_cursor;
                generation = Some(response.generation);
                if cursor.is_none() { break; }
                assert!(pages <= 4, "tree cursor did not advance");
            }
            assert_eq!(children, 32);
        },
        runs,
    );
    emit("tree_all_pages", traversal, count, json!({"pages":pages}));

    let connection = rusqlite::Connection::open_with_flags(
        &db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).unwrap();
    let page_count: u64 = connection.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    let freelist_count: u64 = connection.query_row("PRAGMA freelist_count", [], |r| r.get(0)).unwrap();
    println!("THORN_PERF_STORAGE {}", json!({
        "schema":1, "page_count":page_count, "freelist_count":freelist_count,
        "free_page_pct": if page_count > 0 {100.0*freelist_count as f64/page_count as f64} else {0.0},
        "db_bytes":fs::metadata(&db).unwrap().len(),
        "files":count
    }));
}
