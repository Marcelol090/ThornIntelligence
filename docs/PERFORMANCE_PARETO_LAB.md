# ThornIntelligence — Continuous Pareto Performance Lab
9 October 2026. All benchmarks are opt-in and non-destructive.

## Quick start: actual Rust/Tauri pipeline

Prerequisites: PowerShell 7, Rust stable/MSVC, Windows SDK, Python 3, and
a reproducible CPU/power profile. Run from the repository root:

~~~powershell
pwsh -File .\scripts\perf\run-windows.ps1 -Files 1500 -Repeats 7
pwsh -File .\scripts\perf\run-windows.ps1 -Files 50000 -Repeats 5
python .\scripts\perf\pareto.py --candidate .\reports\performance\BASELINE.json
python .\scripts\perf\pareto.py --baseline .\reports\performance\BASELINE.json --candidate .\reports\performance\CANDIDATE.json --output .\reports\performance\pareto.json --fail-on-regression
~~~

The runner saves JSON and raw logs to reports/performance/. Benchmarks execute
release-mode tests explicitly marked ignored: normal cargo test remains fast.
File fixtures and the SQLite benchmark database are generated in a private
temporary folder. Neither the scanner nor the scripts delete or defragment
user files. Rust scenario timings exclude compilation and fixture creation.

### Native scenarios and correctness

| Scenario | Workload | Verification |
|---|---|---|
| scan_metadata | Complete WalkDir metadata mode | Count is complete, hash bytes zero |
| scan_blake3_verified | Candidate sampling + full BLAKE3 | Genuine duplicate group, complete report |
| search_walkdir_regex | Full live filesystem Regex search | Expected match count, no hidden cap |
| index_initial | Persistent SQLite index build | Snapshot and method label |
| index_unchanged | Repeated incremental refresh | Full file count maintained |
| search_sqlite_regex | Search committed SQLite snapshot | Same matches as live walker |
| tree_first_page | Lazy 16-entry keyset request | Cursor and page size |
| tree_all_pages | Paginate entire root tree | Exactly 32 direct children |

Default: 1,500 synthetic files and five repetitions. Environment settings:
THORN_PERF_FILES (100..120000) and THORN_PERF_REPEAT (2..30). Files include
real duplicate content and identical head/tail with different middle content.
The test suite asserts full BLAKE3; a partial fingerprint never certifies
a duplicate. The release runner captures p50, p95, min/max, logical hash-read
bytes, native process sampled CPU seconds, RSS and process I/O transfer counts.
Win32_Process transfer counts include cached/nonphysical I/O.

Sampling can miss short lived peaks. Repeated tests use warm-ish OS cache, not
a proven cold cache. Reboot experiments must be separately described rather
than calling any second run cold. Keep privilege, same hardware, filesystem,
power settings, Rust release profile, and the actual index_method comparable.
Specifically compare ntfs_mft, usn_unchanged, and walkdir independently.

## Experimental structure comparison — not a Rust benchmark

~~~powershell
python .\scripts\perf\structure-lab.py --files 30000 --repeats 7 --output .\reports\performance\structure-lab.json
~~~

Synthetic Python/SQLite laboratory compares linear substring, B-tree sorted
prefix, SQLite indexed range, SQLite LIKE scan and SQLite FTS5 trigram.
Every implementation must return the same result count. It also records
FTS5 availability, index build overhead, file size and EXPLAIN QUERY PLAN.

FTS5 trigram favors some substring queries of at least 3 characters, but
short queries may scan the entire index. Trigram search does not preserve
arbitrary Rust Regex semantics automatically; any future Regex accelerator
must generate a superset and apply the canonical Regex as a final filter.
Measure Unicode/case differences, storage cost and update amplification.
Finite-state transducers (fst) are suitable for compressed immutable prefix
indices, not cheap per-file online mutations. The original jwalk library
currently declares itself unmaintained: prefer evaluating maintained
alternatives such as ignore after setting a reliable baseline.

## Read-only fragmentation and storage telemetry

~~~powershell
python .\scripts\perf\sqlite-health.py --database "C:\path\storage-index.sqlite"
python .\scripts\perf\sqlite-health.py --database "C:\path\storage-index.sqlite" --simulate-vacuum --quick-check
pwsh -File .\scripts\perf\storage-diagnostics.ps1 -DriveLetter C -FilePath "C:\path\file.bin"
pwsh -File .\scripts\perf\storage-diagnostics.ps1 -DriveLetter C -AnalyzeVolume
~~~

- SQLite: page_count, freelist_count, page size and reusable free percentage
  indicate logical index fragmentation, not physical cluster layout.
- VACUUM simulation creates a consistent SQLite backup in a throwaway
  directory and compacts ONLY THAT COPY. The original source is opened
  read-only, and no maintenance is performed on the live database.
- NTFS: fsutil file queryextents reports allocation extents read-only;
  defrag X: /A /V is analyze-only (optional, may need elevation).
- Heap: profile excess path-string allocations, vector clones, and hash
  bucket overhead separately from NTFS and SQLite fragmentation.
- Never force -Defrag on SSD. Windows differentiates HDD defragmentation
  and SSD TRIM through Optimize Drives. No script performs either action.

## Continuous Pareto policy

~~~powershell
python .\scripts\perf\pareto.py --self-test
python .\scripts\perf\pareto.py --candidate .\reports\performance\BASELINE.json
python .\scripts\perf\pareto.py --baseline .\reports\performance\BASELINE.json --candidate .\reports\performance\CANDIDATE.json --regression-pct 10 --fail-on-regression
~~~

Before improvements, rank the heaviest operations by p95_ms multiplied by a
user-frequency/criticality weight. After improvements, rank the saved latency:
max(0, baseline_p95-candidate_p95) times the same weight. The Pareto group
is the smallest prefix reaching 80% of that measured weighted total.

Default usage weights are hypotheses (search=8, tree-first-page=20,
index-warm=4, full BLAKE3=0.2) and should be replaced with anonymized,
opt-in UI telemetry or observed workloads. Do not interpret the 80/20 rule
as an already established distribution. The comparator rejects fixture-count
mismatches and known hardware/OS mismatches, reports missing scenarios, and
optionally fails on >10% p95 regression. Hosted CI can smoke-test behavior
but is too noisy for binding microbenchmark SLAs.

## Test matrix and promotion gate

| Workload | Risk checked |
|---|---|
| 100, 1500, 50000, 120000 files | Nonlinear growth and RSS limits |
| Flat vs 32+ deeply nested directories | Walker parallelism crossover |
| 256B, 64KiB, 4MiB, multi-GB content | BLAKE3, I/O budgets and CPU saturation |
| Identical edge fingerprints, different centers | No false positives |
| Sparse, hardlink, junction, symlink, OneDrive placeholder | Data safety and scope |
| Warm/cold-like, NTFS MFT/USN vs WalkDir | Privilege, cache, delta correctness |
| Short, absent, common and rare search term | Prefix/trigram crossover |
| Concurrent index reader + canceled writer | Snapshot isolation and atomicity |
| 16/100/200-keyset pages, stale cursor | Tree correctness under updates |
| High SQLite freelist and WAL | Logical maintenance tradeoffs |
| HDD/SATA SSD/NVMe | Media-specific strategies |

P0: avoid repeated full-index Regex scans, reduce allocations in full
duplicate analysis, and track memory/I/O by phase.
P1: benchmark a candidate FTS5 trigram index, verify Regex via post-filter,
and compare contiguous arena with per-node allocations. Audit NTFS USN
rollover, rename and delete handling before relying on a cache.
P2: explore fst/sorted prefix and maintained parallel walker only if
measured query traffic justifies incremental maintenance and disk cost.
P3: tune adaptive concurrency per SSD/HDD; expand ETW and UI telemetry.

For detailed hotspots use WPR/WPA (CPU + Disk I/O), samply, Criterion,
cargo flamegraph (platform-supported), and SQLite EXPLAIN QUERY PLAN.
Never recommend destructive optimization based solely on timing anecdotes.

### Exa-vetted sources

- SQLite FTS5 trigram: https://sqlite.org/fts5.html#the_trigram_tokenizer
- Rust fst: https://docs.rs/fst/latest/fst/
- BLAKE3 rayon/mmap: https://docs.rs/blake3/latest/blake3/
- Windows WPR command line: https://learn.microsoft.com/en-us/windows-hardware/test/wpt/wpr-command-line-options
- Windows NTFS retrieval pointers: https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_retrieval_pointers
- Windows fsutil queryextents: https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-file
- Windows Optimize Drives HDD/SSD: https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/defragment-optimize-your-data-drives-in-windows
- Original jwalk no longer maintained: https://github.com/Byron/jwalk
- CI timing caveats: https://nexte.st/docs/integrations/criterion/

No performance improvement in this document is claimed as already measured.
