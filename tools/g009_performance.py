"""Exact, non-tunable G009 performance calculations."""
from __future__ import annotations
from fractions import Fraction
from typing import Any
from g009_common import QualificationError, fail, fraction_json, fraction_value

WARMUPS = 5
SAMPLES = 30
LATENCY_FACTOR = Fraction(6, 5)
THROUGHPUT_FACTOR = Fraction(4, 5)
MAD_LIMIT = Fraction(1, 10)

def _samples(values: Any, label: str) -> list[Fraction]:
    if not isinstance(values, list) or len(values) != SAMPLES:
        fail(f"{label} must contain exactly {SAMPLES} samples")
    samples = [fraction_value(item, f"{label}[{index}]") for index, item in enumerate(values)]
    if any(item < 0 for item in samples): fail(f"{label} has negative sample")
    return samples

def nearest_rank(values: list[Fraction], numerator: int, denominator: int) -> Fraction:
    if not values or not (0 < numerator <= denominator): fail("invalid nearest-rank inputs")
    ordered = sorted(values); rank = (numerator * len(ordered) + denominator - 1) // denominator
    return ordered[rank - 1]

def median(values: list[Fraction]) -> Fraction:
    if not values: fail("median of empty samples")
    ordered = sorted(values); count = len(ordered); middle = count // 2
    return ordered[middle] if count % 2 else (ordered[middle - 1] + ordered[middle]) / 2

def statistics(values: Any, label: str) -> dict[str, dict[str, int]]:
    samples = _samples(values, label); center = median(samples)
    if center <= 0: fail(f"{label} has zero or undefined median")
    mad = median([abs(sample - center) for sample in samples])
    if mad / center > MAD_LIMIT: fail(f"{label} MAD/median exceeds 1/10")
    return {"median": fraction_json(center), "p5": fraction_json(nearest_rank(samples, 5, 100)),
            "p95": fraction_json(nearest_rank(samples, 95, 100)), "mad": fraction_json(mad)}

def characterize(workloads: Any) -> dict[str, Any]:
    if not isinstance(workloads, dict) or not workloads: fail("workloads must be a nonempty object")
    output: dict[str, Any] = {}
    for workload, record in workloads.items():
        if not isinstance(record, dict): fail(f"workload {workload} must be an object")
        warmups = record.get("warmups")
        if not isinstance(warmups, list) or len(warmups) != WARMUPS: fail(f"{workload} needs exactly {WARMUPS} warmups")
        kind = record.get("kind")
        if kind not in ("latency", "throughput"): fail(f"{workload} has invalid kind")
        output[workload] = {"kind": kind, "statistics": statistics(record.get("samples"), workload),
                            "resource": record.get("resource")}
    return output

def thresholds(baseline: dict[str, Any]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for workload, record in baseline.items():
        kind, stats = record.get("kind"), record.get("statistics")
        if kind == "latency":
            result[workload] = {"kind": kind, "median_max": fraction_json(fraction_value(stats["median"], "median") * LATENCY_FACTOR),
                                "p95_max": fraction_json(fraction_value(stats["p95"], "p95") * LATENCY_FACTOR)}
        elif kind == "throughput":
            result[workload] = {"kind": kind, "median_min": fraction_json(fraction_value(stats["median"], "median") * THROUGHPUT_FACTOR),
                                "p5_min": fraction_json(fraction_value(stats["p5"], "p5") * THROUGHPUT_FACTOR)}
        else: fail(f"invalid baseline kind for {workload}")
    return result

def evaluate_holdout(holdout: Any, baseline: dict[str, Any], approved: dict[str, Any]) -> dict[str, Any]:
    measured = characterize(holdout)
    if set(measured) != set(baseline) or set(measured) != set(approved): fail("holdout workload set differs from baseline")
    results: dict[str, Any] = {}
    for workload, record in measured.items():
        limits, stats = approved[workload], record["statistics"]
        if record["kind"] != limits.get("kind"): fail(f"holdout kind drift for {workload}")
        if record["kind"] == "latency":
            passed = fraction_value(stats["median"], "median") <= fraction_value(limits.get("median_max"), "median_max") and fraction_value(stats["p95"], "p95") <= fraction_value(limits.get("p95_max"), "p95_max")
        else:
            passed = fraction_value(stats["median"], "median") >= fraction_value(limits.get("median_min"), "median_min") and fraction_value(stats["p5"], "p5") >= fraction_value(limits.get("p5_min"), "p5_min")
        results[workload] = {"passed": passed, "statistics": stats}
    return {"passed": all(item["passed"] for item in results.values()), "workloads": results}
