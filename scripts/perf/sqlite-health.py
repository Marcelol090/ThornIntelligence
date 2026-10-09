#!/usr/bin/env python3
"""Read-only SQLite index fragmentation report with optional scratch VACUUM A/B.

Never VACUUM the user's database; backup then optimize an expendable copy.
"""
import argparse
from contextlib import closing
import json
import pathlib
import sqlite3
import tempfile
import time

def health(connection):
    def scalar(command):
        return connection.execute(command).fetchone()[0]
    count = int(scalar("PRAGMA page_count"))
    free = int(scalar("PRAGMA freelist_count"))
    page_size = int(scalar("PRAGMA page_size"))
    return {
        "pages": count,
        "free_pages": free,
        "free_page_pct": round(100 * free / count, 3) if count else 0,
        "page_size_bytes": page_size,
        "db_pages_bytes": count * page_size,
        "journal_mode": scalar("PRAGMA journal_mode"),
    }

def inspect(db_path, simulate_vacuum=False, quick_check=False):
    db_path = pathlib.Path(db_path).resolve(strict=True)
    if not db_path.is_file():
        raise ValueError("SQLite database must be a regular file")
    uri = db_path.as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True, timeout=10)) as source:
        result = {"schema": 1, "database": str(db_path),
                  "file_size_bytes": db_path.stat().st_size,
                  "before": health(source)}
        if quick_check:
            result["quick_check"] = source.execute("PRAGMA quick_check").fetchone()[0]
        if simulate_vacuum:
            with tempfile.TemporaryDirectory(prefix="thorn-sqlite-scratch-") as folder:
                scratch = pathlib.Path(folder) / "snapshot.sqlite"
                with closing(sqlite3.connect(scratch)) as target:
                    # Includes committed WAL changes via SQLite snapshot/backup API.
                    start = time.perf_counter()
                    source.backup(target)
                    result["backup_elapsed_ms"] = round((time.perf_counter() - start) * 1000, 3)
                    result["scratch_before"] = health(target)
                    start = time.perf_counter()
                    target.execute("VACUUM")
                    result["scratch_vacuum_elapsed_ms"] = round((time.perf_counter() - start) * 1000, 3)
                    result["scratch_after"] = health(target)
                result["scratch_compaction_pct"] = round(
                    100 * (1 - result["scratch_after"]["db_pages_bytes"] /
                           max(1, result["scratch_before"]["db_pages_bytes"])), 3)
    result["source_untouched"] = True
    result["note"] = ("Freelist pages quantify internal SQLite reuse, not NTFS disk fragmentation. "
                      "VACUUM was only executed on a temporary backup, if requested.")
    return result

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--database", required=True, type=pathlib.Path)
    p.add_argument("--simulate-vacuum", action="store_true")
    p.add_argument("--quick-check", action="store_true")
    p.add_argument("--output", type=pathlib.Path)
    args = p.parse_args()
    result = inspect(args.database, args.simulate_vacuum, args.quick_check)
    output = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output, encoding="utf-8")
    print(output, end="")

if __name__ == "__main__":
    main()
