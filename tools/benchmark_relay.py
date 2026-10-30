#!/usr/bin/env python3
"""Benchmark real production TLS/QUIC gameplay clients with correctness gates.

Build: cargo build --locked --release -p bomberman --example relay_load -p
bomberman-relay --bin bomberman-relay. Live invitations arrive only on stdin.
"""
import argparse
import hashlib
import gzip
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit, urlunsplit

ROOT = Path(__file__).resolve().parents[1]


def percentiles(values):
    values = sorted(values)
    if not values:
        return dict(samples=0, p50=None, p95=None, p99=None, max=None)
    import math
    return dict(samples=len(values), p50=statistics.median(values),
                p95=values[math.ceil(.95 * len(values)) - 1],
                p99=values[math.ceil(.99 * len(values)) - 1], max=values[-1])


def stop(process):
    if process and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def resources(pid):
    proc = Path(f"/proc/{pid}/stat")
    if proc.exists():
        try:
            # Linux ps rounds process time to whole seconds. /proc retains
            # scheduler ticks, so short CPU samples do not become 400% spikes.
            fields = proc.read_text().rsplit(")", 1)[1].split()
            ticks = os.sysconf("SC_CLK_TCK")
            seconds = (int(fields[11])+int(fields[12]))/ticks
            rss = int(fields[21])*os.sysconf("SC_PAGE_SIZE")//1024
            return seconds, rss
        except (OSError, ValueError, IndexError):
            return None
    result = subprocess.run(["ps", "-p", str(pid), "-o", "time=,rss="],
                            capture_output=True, text=True, check=False)
    fields = result.stdout.split()
    if len(fields) != 2:
        return None
    seconds = sum(float(part) * 60**i for i, part in enumerate(reversed(fields[0].split(":"))))
    return seconds, int(fields[1])


def relay_metrics(path, start_ms, end_ms, short_run=False):
    records = []
    if path.exists():
        for line in path.read_text().splitlines():
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if record.get("event") == "relay_gameplay_summary":
                records.append(record)
    # Summaries are cumulative per connection, so keep the last one. Never
    # add periodic cumulative counters to disconnect counters.
    final = {}
    for record in records:
        d = record["diagnostics"]
        final[(d["room_tag"], d["peer"])] = d
    buckets = [0] * 1001
    maximum = 0
    selected = [r for r in records if short_run or start_ms <= r.get("interval_start_unix_ms", 0) < r.get("interval_end_unix_ms", 0) <= end_ms]
    for record in selected:
        h = record["diagnostics"]["forwarding"]
        if h["buckets"]:
            buckets = [a+b for a, b in zip(buckets, h["buckets"])]
        maximum = max(maximum, h["max_us"])
    count = sum(buckets)
    def percentile(p):
        if not count:
            return None
        import math
        threshold = math.ceil(p * count)
        total = 0
        for i, value in enumerate(buckets):
            total += value
            if total >= threshold:
                return maximum / 1000 if i == 1000 else min(maximum / 1000, (i+1)/10)
        raise AssertionError("histogram count")
    return dict(summary_records=len(records), measured_intervals=len(selected), connections=len(final),
                forwarding_ms=dict(samples=count, p50=percentile(.5), p95=percentile(.95),
                                   p99=percentile(.99), max=maximum/1000 if count else None),
                forwarding_failures=sum(d["traffic"]["send_failures"] for d in final.values()),
                send_buffer_pressure=sum(d["traffic"]["send_buffer_pressure"] for d in final.values()),
                sender_loss_estimate=sum(d["transport"]["sender_lost_packets"] for d in final.values()),
                congestion_events=sum(d["transport"]["congestion_events"] for d in final.values()))


def combine_rooms(cases):
    """Pool measured histograms; each process drives one independent room."""
    result = {key: cases[0][key] for key in ("version", "protocol", "players_per_room", "warmup_seconds", "measurement_seconds", "seed", "limitations")}
    result.update(rooms=len(cases), passed=all(c["passed"] for c in cases),
        first_failure=next((c["first_failure"] for c in cases if c["first_failure"]), None),
        failure_trace=next((c["failure_trace"] for c in cases if c["failure_trace"]), None),
        measurement_start_unix_ms=max(c["measurement_start_unix_ms"] for c in cases),
        measurement_end_unix_ms=min(c["measurement_end_unix_ms"] for c in cases),
        client_diagnostics=[d for c in cases for d in c["client_diagnostics"]],
        measurement_window_scope="client rates/histograms cover each room's full measurement; relay and resources use the common overlapping window")
    for key in ("canonical_ticks_compared", "sampled_render_frames", "render_frames_checked", "explained_render_boundaries", "rematches", "queue_evictions", "send_failures_and_queue_rejections", "intentional_supersessions", "lifetime_application_failures", "received_packets_per_second", "sent_packets_per_second", "achieved_committed_ticks_per_second"):
        result[key] = sum(c[key] for c in cases)
    result["shared_outcomes"] = {}
    for case in cases:
        for kind, count in case["shared_outcomes"].items():
            result["shared_outcomes"][kind] = result["shared_outcomes"].get(kind, 0)+count
    result["latency_distributions"] = {}
    for name in cases[0]["latency_distributions"]:
        buckets = {}
        samples = maximum = 0
        for case in cases:
            distribution = case["latency_distributions"][name]
            samples += distribution["samples"]
            maximum = max(maximum, distribution["max"])
            for index, count in distribution["buckets"]:
                buckets[index] = buckets.get(index, 0)+count
        def percentile(percent):
            if not samples: return None
            threshold = (samples*percent+99)//100
            total = 0
            for index, count in sorted(buckets.items()):
                total += count
                if total >= threshold: return maximum if index == 4096 else index
            raise AssertionError("timing histogram count")
        result[name] = dict(samples=samples, p50=percentile(50), p95=percentile(95), p99=percentile(99), max=maximum if samples else None)
        result["latency_distributions"][name] = dict(samples=samples, max=maximum, buckets=sorted(buckets.items()))
    return result


def run_case(args, rooms, players, repetition, link):
    with tempfile.TemporaryDirectory(prefix="atomic-gameplay-load-") as temporary:
        private = Path(temporary)
        relay = proxy = None
        generators, child_reports, progress_paths, generator_logs = [], [], [], []
        relay_log = private / "relay.jsonl"
        tag = f"{rooms}x{players}-r{repetition+1}"
        case_report = args.report.parent / (args.report.stem + "-" + tag + ".json")
        case_report.unlink(missing_ok=True)
        progress_path = case_report.with_suffix(".progress.json")
        progress_path.unlink(missing_ok=True)
        last_resources = {}
        measurement = None
        cpu, rss, generator_cpu, generator_rss = [], [], [], []
        try:
            if not args.live:
                with socket.socket() as s:
                    s.bind(("127.0.0.1", 0))
                    port = s.getsockname()[1]
                state = private / "state"
                initialized = subprocess.run([str(args.relay), "init", "--state", str(state),
                    "--listen", f"127.0.0.1:{port}", "--advertise", f"127.0.0.1:{port}"],
                    capture_output=True, text=True, check=True)
                links = [s.strip() for s in initialized.stdout.splitlines() if s.startswith("atomic-bomberman://")]
                if len(links) != 1:
                    raise RuntimeError("relay did not supply one private invitation")
                link = links[0]
                with relay_log.open("w") as log:
                    relay = subprocess.Popen([str(args.relay), "serve", "--state", str(state)],
                                             stdout=subprocess.DEVNULL, stderr=log)
                deadline = time.monotonic()+10
                while True:
                    if relay.poll() is not None:
                        raise RuntimeError("disposable relay exited")
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=.1):
                            break
                    except OSError:
                        if time.monotonic() > deadline:
                            raise RuntimeError("relay listener timed out")
                        time.sleep(.05)
            if not args.live and (args.rtt_ms or args.jitter_ms or args.loss_percent):
                from verify_datagram import ImpairmentProxy
                proxy = ImpairmentProxy(port, seed=args.seed+repetition, rtt_ms=args.rtt_ms,
                                        jitter_ms=args.jitter_ms, loss_percent=args.loss_percent)
                parsed = urlsplit(link)
                link = urlunsplit((parsed.scheme, f"[::1]:{port}", parsed.path, parsed.query, parsed.fragment))
            # Independent production room processes prevent the generator's
            # serial simulation loop from becoming the forty-client bottleneck.
            for room in range(rooms):
                child = case_report if rooms == 1 else case_report.with_name(case_report.stem+f".room{room+1}.json")
                child.unlink(missing_ok=True)
                progress = child.with_suffix(".progress.json")
                progress.unlink(missing_ok=True)
                child_reports.append(child)
                progress_paths.append(progress)
                generator_logs.append(private / f"generator{room}.log")
                with generator_logs[-1].open("w") as log:
                    generator = subprocess.Popen([str(args.client), "--rooms", "1", "--players", str(players),
                        "--warmup", str(args.warmup), "--duration", str(args.duration), "--seed", str(args.seed+repetition+room),
                        "--report", str(child)], stdin=subprocess.PIPE, stdout=log, stderr=log, text=True)
                generators.append(generator)
                generator.stdin.write(link)
                generator.stdin.close()
            deadline = time.monotonic()+args.warmup+args.duration+55
            while any(g.poll() is None for g in generators):
                if time.monotonic() > deadline:
                    raise RuntimeError("gameplay generator timed out")
                if measurement is None and all(p.exists() for p in progress_paths):
                    windows = [json.loads(p.read_text()) for p in progress_paths]
                    measurement = dict(measurement_start_unix_ms=max(w["measurement_start_unix_ms"] for w in windows),
                        measurement_end_unix_ms=min(w["measurement_end_unix_ms"] for w in windows))
                sample_time = time.time()
                measuring = measurement is not None and measurement["measurement_start_unix_ms"] <= sample_time*1000 <= measurement["measurement_end_unix_ms"]
                for key, processes, cpus, memories in [("relay", [relay] if relay else [], cpu, rss), ("generator", generators, generator_cpu, generator_rss)]:
                    values = [resources(process.pid) for process in processes]
                    if values and all(v is not None for v in values):
                        sample = (sum(v[0] for v in values), sum(v[1] for v in values))
                        previous = last_resources.get(key)
                        if measuring or (measurement is None and args.live):
                            if previous and sample_time > previous[0] and (measurement is None or previous[0]*1000 >= measurement["measurement_start_unix_ms"]):
                                cpus.append(max(0, (sample[0]-previous[1])*100/(sample_time-previous[0])))
                            memories.append(sample[1])
                        last_resources[key] = (sample_time, sample[0])
                time.sleep(.25)
            time.sleep(.25)  # Let authenticated disconnects emit final summaries.
            if not all(p.exists() for p in child_reports):
                errors = []
                for child, log in zip(child_reports, generator_logs):
                    log_text = log.read_text()
                    child.with_suffix(".generator.jsonl").write_text(log_text)
                    errors.extend(line for line in log_text.splitlines() if not line.startswith("{"))
                if relay_log.exists():
                    case_report.with_suffix(".relay.jsonl.gz").write_bytes(gzip.compress(
                        ("\n".join(line for line in relay_log.read_text().splitlines() if line.startswith('{'))+"\n").encode(), mtime=0))
                raise RuntimeError("generator failed before producing results: " + " ".join(errors)[-1800:])
            cases = [json.loads(p.read_text()) for p in child_reports]
            for case, generator in zip(cases, generators):
                case["passed"] &= generator.returncode == 0
            result = cases[0] if rooms == 1 else combine_rooms(cases)
            if rooms > 1:
                result["room_reports"] = [p.name for p in child_reports]
                result["generator_processes"] = rooms
            result["impairment"] = dict(rtt_ms=args.rtt_ms, jitter_ms=args.jitter_ms, loss_percent=args.loss_percent,
                udp_forwarded=proxy.forwarded if proxy else None, udp_dropped=proxy.dropped if proxy else None)
            result["generator_exit_codes"] = [g.returncode for g in generators]
            result["generator_exit_code"] = next((g.returncode for g in generators if g.returncode != 0), 0)
            result["passed"] &= generator.returncode == 0
            result["resource_scope"] = "measurement interval; CPU from process-time deltas (100 percent = one core)" if measurement else "whole run including setup and warmup (baseline without progress markers)"
            result["relay_cpu_percent"] = percentiles(cpu)
            result["relay_memory_kib"] = percentiles(rss)
            result["generator_cpu_percent"] = percentiles(generator_cpu)
            result["generator_memory_kib"] = percentiles(generator_rss)
            metrics = relay_metrics(relay_log, result.get("measurement_start_unix_ms", 0), result.get("measurement_end_unix_ms", 0), args.duration < 20)
            result["relay"] = metrics if not args.live else None
            result["relay_forwarding_scope"] = ("whole connection (short smoke run)" if args.duration < 20 else "complete ten-second intervals inside measurement; partial boundary intervals excluded") if not args.live else "collected separately from deployed relay diagnostics"
            result["gates"] = dict(gameplay_correct=result["passed"], zero_application_evictions=result["queue_evictions"] == 0,
                zero_send_failures=result["send_failures_and_queue_rejections"] == 0)
            if not args.live:
                result["gates"].update(zero_forwarding_failures=metrics["forwarding_failures"] == 0,
                    forwarding_p99_below_20_ms=metrics["forwarding_ms"]["p99"] is not None and metrics["forwarding_ms"]["p99"] < 20,
                    all_relay_connections_observed=metrics["connections"] == rooms*players)
                # Persist only the allowlisted diagnostic records, never stderr
                # from the setup command that contains the private invitation.
                safe_log = case_report.with_suffix(".relay.jsonl.gz")
                safe_bytes = gzip.compress(("\n".join(line for line in relay_log.read_text().splitlines() if line.startswith('{'))+"\n").encode(), mtime=0)
                safe_log.write_bytes(safe_bytes)
                result["relay_diagnostics_artifact"] = safe_log.name
                result["relay_diagnostics_sha256"] = hashlib.sha256(safe_bytes).hexdigest()
            result["passed"] = all(result["gates"].values())
            case_report.write_text(json.dumps(result, indent=2)+"\n")
            return result
        finally:
            progress_path.unlink(missing_ok=True)
            for path in progress_paths:
                path.unlink(missing_ok=True)
            for generator in generators:
                stop(generator)
            stop(relay)
            if proxy:
                proxy.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client", type=Path, default=ROOT/"target/release/examples/relay_load")
    parser.add_argument("--relay", type=Path, default=ROOT/"target/release/bomberman-relay")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--workloads", nargs="+", choices=["1x2","1x10","4x10"], default=["1x2","1x10","4x10"])
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--duration", type=int, default=60)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--seed", type=int, default=1997)
    parser.add_argument("--rtt-ms", type=float, default=0)
    parser.add_argument("--jitter-ms", type=float, default=0)
    parser.add_argument("--loss-percent", type=float, default=0)
    parser.add_argument("--live", action="store_true", help="deployed relay invitation on stdin; one room at a time")
    args = parser.parse_args()
    if args.duration < 1 or args.warmup < 0 or args.repetitions < 1:
        parser.error("duration/repetitions must be positive and warmup nonnegative")
    import math
    if any(not math.isfinite(v) or v < 0 for v in (args.rtt_ms,args.jitter_ms,args.loss_percent)) or args.loss_percent > 100:
        parser.error("invalid impairment parameters")
    if args.live and (args.rtt_ms or args.jitter_ms or args.loss_percent):
        parser.error("live WAN measurements do not use local impairment")
    if args.live and "4x10" in args.workloads:
        parser.error("live baseline is limited to one room at a time")
    args.client, args.relay = args.client.resolve(), args.relay.resolve()
    link = sys.stdin.read().strip() if args.live else None
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, complete=False, release_gate_passed=False, environment=f"{platform.system()} {platform.machine()}",
        transport="deployed WAN" if args.live else "local TLS/QUIC", warmup_seconds=args.warmup, measurement_seconds=args.duration,
        repetitions=args.repetitions, client_sha256=hashlib.sha256(args.client.read_bytes()).hexdigest(),
        relay_sha256=None if args.live else hashlib.sha256(args.relay.read_bytes()).hexdigest(), cases=[],
        limitations=["Relay forwarding and server resources are outside this client runner; collect matching deployed relay diagnostics separately."] if args.live else [])
    def save():
        args.report.write_text(json.dumps(report, indent=2)+"\n")
    save()
    try:
        for workload in args.workloads:
            rooms, players = map(int, workload.split("x"))
            for repetition in range(args.repetitions):
                case = run_case(args, rooms, players, repetition, link)
                report["cases"].append(case)
                print(json.dumps(dict(workload=workload, repetition=repetition+1, passed=case["passed"],
                    canonical_ticks=case["canonical_ticks_compared"], application_rtt_ms=case["application_rtt_ms"],
                    relay_forwarding_ms=case["relay"]["forwarding_ms"] if case["relay"] else None)), flush=True)
                save()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        report["error"] = f"Benchmark subprocess failed ({type(error).__name__}); no command or endpoint recorded" if isinstance(error, subprocess.SubprocessError) else str(error)
        save()
        return 1
    report["complete"] = True
    report["passed"] = all(c["passed"] for c in report["cases"])
    report["release_gate_passed"] = report["passed"] and not args.live and not (args.rtt_ms or args.jitter_ms or args.loss_percent) and args.duration >= 60 and args.warmup >= 10 and args.repetitions >= 3 and set(args.workloads) == {"1x2","1x10","4x10"}
    save()
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
