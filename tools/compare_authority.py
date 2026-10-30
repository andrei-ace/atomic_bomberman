#!/usr/bin/env python3
"""Compare the same production-session fixture over LAN and disposable relay.

Both binaries must be built with the same network_latency instrumentation.
The baseline network code is pinned to c6076f4 (0.4.1). No live relay is used.
"""
import argparse
import hashlib
import json
import platform
import re
import statistics
import tempfile
from pathlib import Path
from types import SimpleNamespace

from verify_datagram import ImpairmentProxy, local_case, run_pair

ROOT = Path(__file__).resolve().parents[1]
APP_VERSION = re.search(r'version = "([^"\n]+)"', (ROOT / "Cargo.toml").read_text())[1]
PROTOCOL = int(re.search(r"PROTOCOL_VERSION: u32 = (\d+)", (ROOT / "crates/bomberman-net/src/protocol.rs").read_text())[1])
PROFILES = [
    ("lan-0", "lan", 0, 0, 0),
    ("lan-10", "lan", 10, 0, 0),
    ("lan-20", "lan", 20, 0, 0),
    ("relay-40", "relay", 40, 0, 0),
    ("relay-80", "relay", 80, 0, 0),
    ("relay-160", "relay", 160, 0, 0),
    ("relay-loss", "relay", 80, 20, 5),
]


def run_case(example, relay, profile, seed, gameplay_port=1997):
    name, transport, rtt, jitter, loss = profile
    impairment = dict(rtt_ms=rtt, jitter_ms=jitter, loss_percent=loss)
    if transport == "relay":
        return local_case(SimpleNamespace(example=example, relay=relay, seed=seed), impairment)
    with tempfile.TemporaryDirectory(prefix="atomic-lan-comparison-") as directory:
        proxy = ImpairmentProxy(1997, **impairment, gameplay_port=gameplay_port, legs=2, seed=seed)
        try:
            result = run_pair(example, Path(directory), "", mode="lan", address="[::1]:1997")
            assert proxy.forwarded, "LAN packets bypassed the impairment proxy"
            result.update(environment="Production LAN TCP/QUIC; IPv6 loopback proxy to IPv4 host",
                          injected_gameplay_rtt_ms=rtt, jitter_ms=jitter,
                          udp_loss_percent=loss, udp_packets_forwarded=proxy.forwarded)
            return result
        finally:
            proxy.close()


def metrics(cases):
    return {
        "p95_first_visible_ms": statistics.median(c["sample_to_visible_ms"]["p95"] for c in cases),
        "backward_correction_pixels": statistics.median(c["movement_continuity"]["total_backward_correction_pixels"] for c in cases),
        "last_movement_after_release_ms": statistics.median(c["movement_continuity"]["max_last_movement_after_release_ms"] for c in cases),
        "all_trials_settled": all(c["movement_continuity"]["settled_samples"] == len(c["samples"]) for c in cases),
        "canonical_states_match": all(c["passed"] for c in cases),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-example", type=Path, required=True)
    parser.add_argument("--baseline-relay", type=Path, required=True)
    parser.add_argument("--candidate-example", type=Path, default=ROOT / "target/debug/examples/network_latency")
    parser.add_argument("--candidate-relay", type=Path, default=ROOT / "target/debug/bomberman-relay")
    parser.add_argument("--baseline-lan-gameplay-port", type=int, default=1998)
    parser.add_argument("--candidate-lan-gameplay-port", type=int, default=1997)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--profiles", nargs="+", choices=[p[0] for p in PROFILES])
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if any(not 1 <= port <= 65535 for port in [args.baseline_lan_gameplay_port, args.candidate_lan_gameplay_port]):
        parser.error("LAN gameplay ports must be between 1 and 65535")
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    binaries = {name: getattr(args, name).resolve() for name in
                ("baseline_example", "baseline_relay", "candidate_example", "candidate_relay")}
    for path in binaries.values():
        if not path.is_file():
            parser.error(f"Missing binary: {path}")
    report = dict(baseline_commit="c6076f4c4143b9210e8b3f7a4205f08e3ee5e90b",
                  environment=f"{platform.system()} {platform.machine()}",
                  repetitions=args.repetitions,
                  complete=False, passed=False, release_gate_passed=False,
                  binary_sha256={name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in binaries.items()},
                  profiles=[],
                  limitations=["Headless production display-state sampling at 60 Hz; no physical display or input device.",
                               "A disposable loopback relay models injected delay; this is not deployed WAN acceptance."])
    # An interrupted rerun must never leave an older successful report active.
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    selected = [profile for profile in PROFILES if not args.profiles or profile[0] in args.profiles]
    for profile in PROFILES:
        if args.profiles and profile[0] not in args.profiles:
            continue
        entry = dict(name=profile[0], baseline=[], candidate=[])
        for repetition in range(args.repetitions):
            # Alternate order to avoid consistently favoring the second build.
            order = ("baseline", "candidate") if repetition % 2 == 0 else ("candidate", "baseline")
            for build in order:
                try:
                    case = run_case(binaries[build + "_example"], binaries[build + "_relay"], profile, 1997 + repetition, gameplay_port=getattr(args, build + "_lan_gameplay_port"))
                except (AssertionError, RuntimeError, OSError, ValueError) as error:
                    entry["error"] = str(error)
                    entry["passed"] = False
                    report["profiles"].append(entry)
                    report.update(passed=False, complete=False, release_gate_passed=False)
                    args.report.parent.mkdir(parents=True, exist_ok=True)
                    args.report.write_text(json.dumps(report, indent=2) + "\n")
                    raise
                entry[build].append(case)
                print(f"{profile[0]} {build} {repetition + 1}: visible p95={case['sample_to_visible_ms']['p95']:.2f}ms; "
                      f"correction={case['movement_continuity']['total_backward_correction_pixels']:.2f}px; "
                      f"post-release={case['movement_continuity']['max_last_movement_after_release_ms']:.2f}ms", flush=True)
        before, after = metrics(entry["baseline"]), metrics(entry["candidate"])
        assert all(c["build"]["version"] == "0.4.1" and c["build"]["protocol"] == 5 for c in entry["baseline"])
        assert all(c["build"]["version"] == APP_VERSION and c["build"]["protocol"] == PROTOCOL for c in entry["candidate"])
        assert {c["build"]["fixture_sha256"] for c in entry["baseline"] + entry["candidate"]} == {entry["baseline"][0]["build"]["fixture_sha256"]}, "Fixtures differ between builds"
        entry["baseline_metrics"], entry["candidate_metrics"] = before, after
        entry["gates"] = {
            "correction_reduced_30_percent": after["backward_correction_pixels"] <= before["backward_correction_pixels"] * .7 + 1e-9,
            "settling_reduced_30_percent": after["last_movement_after_release_ms"] <= before["last_movement_after_release_ms"] * .7 + 1e-9,
            "first_visible_p95_at_most_40_ms": after["p95_first_visible_ms"] <= 40,
            "all_trials_settled": after["all_trials_settled"],
            "canonical_states_match": after["canonical_states_match"],
        }
        entry["passed"] = all(entry["gates"].values())
        report["profiles"].append(entry)
        report["complete"] = len(report["profiles"]) == len(selected)
        report["passed"] = report["complete"] and all(p["passed"] for p in report["profiles"])
        report["release_gate_passed"] = report["passed"] and len(selected) == len(PROFILES) and args.repetitions >= 5
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report.get("passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
