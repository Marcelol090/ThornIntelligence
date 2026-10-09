#!/usr/bin/env python3
"""Algorithm lab, NOT a Thorn backend benchmark: compare query index shapes.

Synthetic ASCII names isolate substring vs prefix characteristics. Verify
result equivalence before considering an architectural migration.
"""
import argparse
import bisect
import json
import pathlib
import platform
import sqlite3
import statistics
import tempfile
import time

def timing(action, repeats):
    samples = []
    result = None
    for _ in range(repeats):
        start = time.perf_counter()
        result = action()
        samples.append(round((time.perf_counter() - start) * 1000, 5))
    ordered = sorted(samples)
    return result, samples, ordered[len(ordered) // 2], ordered[max(0, (95 * len(ordered) + 99) // 100 - 1)]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--files", type=int, default=30000)
    parser.add_argument("--repeats", type=int, default=7)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    if not (100 <= args.files <= 300000 and 2 <= args.repeats <= 50):
        parser.error("--files must be 100..300000 and --repeats 2..50")
    names = [f"targetneedle-{i:09d}.bin" if i % 23 == 0
             else f"asset-{i:09d}-collection.dat" for i in range(args.files)]
    ordered_names = sorted(names)
    metrics = []

    def add(name, func, expected, details=None):
        actual, samples, p50, p95 = timing(func, args.repeats)
        if actual != expected:
            raise AssertionError(f"Incorrect results for {name}: {actual} != {expected}")
        metrics.append({
            "schema": 1, "scenario": name, "files": args.files, "iterations": args.repeats,
            "p50_ms": p50, "p95_ms": p95, "samples_ms": samples,
            "details": {"matches": expected, **(details or {})}
        })

    with tempfile.TemporaryDirectory(prefix="thorn-structure-lab-") as folder:
        db = pathlib.Path(folder) / "lab.sqlite"
        with sqlite3.connect(db) as con:
            con.execute("CREATE TABLE files(id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            con.executemany("INSERT INTO files(name) VALUES(?)", ((x,) for x in names))
            con.execute("CREATE INDEX idx_files_name ON files(name)")
            con.commit()
            needle = "needle"
            needle_count = sum(needle in name for name in names)
            add("lab_python_substring", lambda: sum(needle in n for n in names), needle_count)
            add("lab_sqlite_like_scan",
                lambda: con.execute("SELECT COUNT(*) FROM files WHERE name LIKE ?", ("%needle%",)).fetchone()[0],
                needle_count)
            prefix = "target"
            prefix_count = sum(n.startswith(prefix) for n in names)
            add("lab_python_sorted_prefix",
                lambda: bisect.bisect_left(ordered_names, prefix + "\uffff") - bisect.bisect_left(ordered_names, prefix),
                prefix_count)
            add("lab_sqlite_btree_prefix",
                lambda: con.execute(
                    "SELECT COUNT(*) FROM files WHERE name >= ? AND name < ?",
                    (prefix, prefix + "\uffff")
                ).fetchone()[0], prefix_count)
            btree_plan = con.execute(
                "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM files WHERE name >= ? AND name < ?",
                (prefix, prefix + "\uffff")
            ).fetchall()
            fts_status = "unsupported"
            start = time.perf_counter()
            try:
                con.execute("CREATE VIRTUAL TABLE tri USING fts5(name, tokenize='trigram', detail='none')")
                con.execute("INSERT INTO tri(rowid,name) SELECT id,name FROM files")
                con.commit()
                fts_build_ms = round((time.perf_counter() - start) * 1000, 3)
                fts_status = "supported"
                add("lab_sqlite_fts5_trigram",
                    lambda: con.execute("SELECT COUNT(*) FROM tri WHERE name LIKE ?", ("%needle%",)).fetchone()[0],
                    needle_count)
                short = "as"
                short_count = sum(short in name for name in names)
                add("lab_sqlite_fts5_short_fallback",
                    lambda: con.execute("SELECT COUNT(*) FROM tri WHERE name LIKE ?", ("%as%",)).fetchone()[0],
                    short_count, {"note": "LIKE with fewer than 3 literal characters may scan all rows"})
            except sqlite3.OperationalError as exc:
                fts_status = f"unavailable: {exc}"
                fts_build_ms = None
            output = {
                "schema": 1, "kind": "synthetic_algorithm_lab",
                "environment": {"machine": platform.node(), "os": platform.platform(),
                                "cpu": platform.processor(), "sqlite_version": sqlite3.sqlite_version},
                "metrics": metrics, "fts5_status": fts_status, "fts_build_ms": fts_build_ms,
                "btree_query_plan": btree_plan, "sqlite_size_bytes": db.stat().st_size,
                "warning": "These Python/SQLite timings are not equivalent to the compiled Rust/Tauri pipeline. No production data is accessed."
            }
    result = json.dumps(output, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(result, encoding="utf-8")
    print(result, end="")

if __name__ == "__main__":
    main()
