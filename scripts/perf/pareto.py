#!/usr/bin/env python3
"""Compare Thorn's opt-in benchmark runs and rank bottlenecks using Pareto.

Only compare equivalent fixtures and machines. Hosted CI is for smoke tests,
not authoritative timing thresholds.
"""
import argparse
import json
import math
import pathlib
import sys

DEFAULT_WEIGHTS = {
    "scan_metadata": 1.0,
    "scan_blake3_verified": 0.2,
    "search_walkdir_regex": 8.0,
    "index_initial": 0.2,
    "index_unchanged": 4.0,
    "search_sqlite_regex": 8.0,
    "tree_first_page": 20.0,
    "tree_all_pages": 4.0,
}

def load(path):
    with open(path, encoding="utf-8") as handle:
        obj = json.load(handle)
    if not isinstance(obj, dict) or not isinstance(obj.get("metrics"), list):
        raise ValueError(f"{path}: expected {{'metrics': [...]}}")
    found = {}
    for entry in obj["metrics"]:
        scenario = entry.get("scenario")
        p95 = entry.get("p95_ms")
        if not isinstance(scenario, str) or not isinstance(p95, (int, float)):
            raise ValueError(f"{path}: invalid metric entry")
        if not math.isfinite(p95) or p95 < 0 or scenario in found:
            raise ValueError(f"{path}: invalid/duplicate p95 for {scenario}")
        found[scenario] = entry
    return obj, found

def compare(candidate, baseline=None, weights=None, threshold_pct=10.0,
            allow_host_mismatch=False):
    weights = weights or DEFAULT_WEIGHTS
    cmeta, current = candidate
    bmeta, previous = baseline if baseline else ({}, {})
    if baseline and not allow_host_mismatch:
        for field in ("machine", "cpu", "os"):
            a, b = cmeta.get("environment", {}).get(field), bmeta.get("environment", {}).get(field)
            if a and b and a != b:
                raise ValueError(f"Environment mismatch ({field}): {b!r} != {a!r}")
    rows = []
    for name, value in current.items():
        prior = previous.get(name)
        if prior and prior.get("files") != value.get("files"):
            raise ValueError(f"{name}: fixture count differs; rerun with same THORN_PERF_FILES")
        ms = float(value["p95_ms"])
        old = float(prior["p95_ms"]) if prior else None
        weight = float(weights.get(name, 1))
        if not math.isfinite(weight) or weight < 0:
            raise ValueError(f"{name}: invalid weight")
        gain = (old - ms) if old is not None else 0.0
        delta_pct = ((ms / old - 1) * 100) if old is not None and old > 0 else None
        rows.append({
            "scenario": name, "files": value.get("files"),
            "p95_ms": round(ms, 3), "baseline_p95_ms": round(old, 3) if old is not None else None,
            "delta_pct": round(delta_pct, 2) if delta_pct is not None else None,
            "weight": weight, "weighted_latency": round(ms * weight, 3),
            "weighted_saved_ms": round(max(gain, 0) * weight, 3),
            "regression": delta_pct is not None and delta_pct > threshold_pct,
        })
    # First run: rank costly operations by p95 * frequency/criticality.
    # A/B runs: rank delivered improvements by weighted time actually saved.
    scoring = "weighted_saved_ms" if baseline else "weighted_latency"
    ranked = sorted(rows, key=lambda row: (-row[scoring], row["scenario"]))
    total = sum(row[scoring] for row in ranked)
    accumulated = 0.0
    pareto = []
    for row in ranked:
        if total <= 0 or accumulated / total >= 0.80:
            break
        accumulated += row[scoring]
        pareto.append(row["scenario"])
    return {
        "schema": 1, "mode": "comparison" if baseline else "baseline",
        "ranking_metric": scoring, "total_weighted_ms": round(total, 3),
        "pareto_80_scenarios": pareto, "rows": ranked,
        "regressions": [r["scenario"] for r in rows if r["regression"]],
        "missing_in_candidate": sorted(set(previous) - set(current)),
    }

def self_test():
    example = {"environment":{"machine":"same"}, "metrics":[
        {"scenario":"a","files":100,"p95_ms":100.0},
        {"scenario":"b","files":100,"p95_ms":10.0}]}
    new = {"environment":{"machine":"same"}, "metrics":[
        {"scenario":"a","files":100,"p95_ms":80.0},
        {"scenario":"b","files":100,"p95_ms":12.0}]}
    p = compare((new, {x["scenario"]:x for x in new["metrics"]}),
                (example,{x["scenario"]:x for x in example["metrics"]}),
                {"a":1,"b":1})
    assert p["pareto_80_scenarios"] == ["a"]
    assert p["regressions"] == ["b"]
    assert compare((example,{x["scenario"]:x for x in example["metrics"]}))["mode"] == "baseline"
    print("Pareto comparator self-test passed.")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=pathlib.Path)
    parser.add_argument("--baseline", type=pathlib.Path)
    parser.add_argument("--weights", type=pathlib.Path, help="JSON mapping scenario -> relative frequency/criticality")
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--regression-pct", type=float, default=10.0)
    parser.add_argument("--fail-on-regression", action="store_true")
    parser.add_argument("--allow-host-mismatch", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if not args.candidate:
        parser.error("--candidate is required")
    weights = json.loads(args.weights.read_text(encoding="utf-8")) if args.weights else DEFAULT_WEIGHTS
    result = compare(load(args.candidate), load(args.baseline) if args.baseline else None,
                     weights, args.regression_pct, args.allow_host_mismatch)
    print(f"Pareto 80% ({result['ranking_metric']}): {', '.join(result['pareto_80_scenarios']) or 'no measurable savings'}")
    print(f"{'SCENARIO':32s} {'P95 ms':>10s} {'DELTA %':>10s} {'WEIGHTED':>12s}")
    for row in result["rows"]:
        print(f"{row['scenario'][:32]:32s} {row['p95_ms']:10.2f} "
              f"{row['delta_pct'] if row['delta_pct'] is not None else float('nan'):10.2f} "
              f"{row[result['ranking_metric']]:12.2f}")
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if result["missing_in_candidate"]:
        print("WARNING: missing scenarios: " + ", ".join(result["missing_in_candidate"]), file=sys.stderr)
        if args.fail_on_regression:
            return 2
    if args.fail_on_regression and result["regressions"]:
        print("REGRESSION: " + ", ".join(result["regressions"]), file=sys.stderr)
        return 2
    return 0

if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError, json.JSONDecodeError) as exc:
        print(f"Benchmark comparison error: {exc}", file=sys.stderr)
        sys.exit(2)
