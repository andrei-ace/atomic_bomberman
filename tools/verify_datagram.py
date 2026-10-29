#!/usr/bin/env python3
"""Verify production QUIC sessions and predicted presentation without a window.

Live mode accepts the private invitation only on stdin. Local mode creates a
disposable relay and injects impairment into encrypted UDP packets, leaving TLS
and QUIC payloads opaque. No firewall settings are read or changed.
"""
import argparse
import heapq
import itertools
import json
import math
import platform
import random
import selectors
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, urlsplit, urlunsplit


ROOT = Path(__file__).resolve().parents[1]


class ImpairmentProxy:
    """Transparent TCP control and independently impaired UDP gameplay.

    Both listeners use the relay's port on a second loopback address, so the
    unmodified GameplayOffer remains valid. A client-to-host-to-client probe
    crosses four UDP legs: client->relay->host->relay->client.
    """

    def __init__(self, port, rtt_ms, jitter_ms, loss_percent):
        self.port = port
        self.delay = rtt_ms / 4000
        self.jitter = jitter_ms / 4000
        self.loss = loss_percent / 100
        self.random = random.Random(1997)
        self.stopped = threading.Event()
        self.streams = []
        self.backends = {}
        self.pending = []
        self.serial = itertools.count()
        self.received = self.forwarded = self.dropped = 0
        self.tcp = socket.socket()
        self.tcp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.tcp.bind(("127.0.0.2", port))
        self.tcp.listen(16)
        self.tcp.settimeout(.1)
        self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.udp.bind(("127.0.0.2", port))
        self.udp.setblocking(False)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.udp, selectors.EVENT_READ, None)
        self.threads = [
            threading.Thread(target=self.accept, daemon=True),
            threading.Thread(target=self.forward_udp, daemon=True),
        ]
        for thread in self.threads:
            thread.start()

    def accept(self):
        while not self.stopped.is_set():
            try:
                incoming, _ = self.tcp.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                outgoing = socket.create_connection(("127.0.0.1", self.port), timeout=5)
            except OSError:
                incoming.close()
                continue
            incoming.settimeout(None)
            outgoing.settimeout(None)
            self.streams.extend([incoming, outgoing])
            for source, destination in [(incoming, outgoing), (outgoing, incoming)]:
                threading.Thread(
                    target=self.forward_tcp, args=(source, destination), daemon=True
                ).start()

    def forward_tcp(self, source, destination):
        try:
            while not self.stopped.is_set():
                data = source.recv(65536)
                if not data:
                    break
                destination.sendall(data)
        except OSError:
            pass
        finally:
            for stream in (source, destination):
                try:
                    stream.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                stream.close()

    def schedule(self, data, destination, address):
        self.received += 1
        if self.random.random() < self.loss:
            self.dropped += 1
            return
        delay = max(0, self.delay + self.random.uniform(-self.jitter, self.jitter))
        heapq.heappush(
            self.pending,
            (time.monotonic() + delay, next(self.serial), destination, address, data),
        )

    def forward_udp(self):
        while not self.stopped.is_set():
            now = time.monotonic()
            while self.pending and self.pending[0][0] <= now:
                _, _, destination, address, data = heapq.heappop(self.pending)
                try:
                    if address is None:
                        destination.send(data)
                    else:
                        destination.sendto(data, address)
                    self.forwarded += 1
                except OSError:
                    pass
            wait = min(.01, max(0, self.pending[0][0] - now)) if self.pending else .01
            try:
                readable = self.selector.select(wait)
            except (OSError, ValueError):
                break
            for key, _ in readable:
                try:
                    data, address = key.fileobj.recvfrom(65536)
                except (BlockingIOError, OSError):
                    continue
                if key.data is None:
                    backend = self.backends.get(address)
                    if backend is None:
                        backend = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                        backend.connect(("127.0.0.1", self.port))
                        backend.setblocking(False)
                        self.backends[address] = backend
                        self.selector.register(backend, selectors.EVENT_READ, address)
                    self.schedule(data, backend, None)
                else:
                    self.schedule(data, self.udp, key.data)

    def close(self):
        self.stopped.set()
        self.tcp.close()
        for stream in self.streams:
            try:
                stream.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            stream.close()
        for thread in self.threads:
            thread.join(timeout=1)
        self.selector.close()
        self.udp.close()
        for backend in self.backends.values():
            backend.close()


def unused_port():
    for _ in range(20):
        with socket.socket() as tcp:
            tcp.bind(("127.0.0.1", 0))
            port = tcp.getsockname()[1]
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
                try:
                    udp.bind(("127.0.0.1", port))
                    return port
                except OSError:
                    pass
    raise RuntimeError("Could not reserve a local test port")


def stop(process):
    if process is None:
        return
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def launch(example, role, report, link, room=None):
    arguments = [str(example), role, "online", str(report)]
    if room is not None:
        arguments.append(room)
    process = subprocess.Popen(
        arguments, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    process.stdin.write(link.encode())
    process.stdin.close()
    process.stdin = None
    return process


def output_error(process, link):
    stdout, stderr = process.communicate(timeout=5)
    text = (stderr + stdout).decode(errors="replace")
    # Invitations never belong in diagnostics, even when a future binary adds logs.
    text = text.replace(link, "[private server link]")
    for values in parse_qs(urlsplit(link).fragment).values():
        for value in values:
            if len(value) >= 16:
                text = text.replace(value, "[private]")
    return text[-2000:]


def run_pair(example, directory, link):
    host_report = directory / "host.json"
    client_report = directory / "client.json"
    host = client = None
    try:
        host = launch(example, "host", host_report, link)
        ready = host_report.with_suffix(".ready.json")
        deadline = time.monotonic() + 25
        while not ready.exists():
            if host.poll() is not None:
                raise RuntimeError("Host failed: " + output_error(host, link))
            if time.monotonic() > deadline:
                raise RuntimeError("Host did not announce its temporary room")
            time.sleep(.02)
        room = json.loads(ready.read_text())["room"]
        client = launch(example, "join", client_report, link, room)
        deadline = time.monotonic() + 65
        while client.poll() is None or host.poll() is None:
            if client.poll() not in (None, 0):
                raise RuntimeError("Client failed: " + output_error(client, link))
            if host.poll() not in (None, 0):
                raise RuntimeError("Host failed: " + output_error(host, link))
            if time.monotonic() > deadline:
                raise RuntimeError("Temporary coordinator session timed out")
            time.sleep(.02)
        host_data = json.loads(host_report.read_text())
        client_data = json.loads(client_report.read_text())
        assert host_data["passed"] and client_data["passed"]
        assert host_data["state_hash"] == client_data["state_hash"], "Canonical state differs"
        assert client_data["gameplay_rtt_ms"], "No gameplay probe completed"
        samples = client_data["samples"]
        assert len(samples) == 10 and all(s["sample_to_visible_ms"] is not None for s in samples)
        latency = sorted(s["sample_to_visible_ms"] for s in samples)
        p95 = latency[math.ceil(.95 * len(latency)) - 1]
        return {
            "passed": True,
            "presentation": client_data["presentation"],
            "host_platform": host_data["platform"],
            "client_platform": client_data["platform"],
            "canonical_state_hash": host_data["state_hash"],
            "host_tick": host_data["tick"],
            "client_tick": client_data["tick"],
            "observed_gameplay_rtt_ms": client_data["gameplay_rtt_ms"],
            "sample_to_visible_ms": {
                "median": statistics.median(latency), "p95": p95, "max": max(latency)
            },
            "responsiveness_target_ms": 40,
            "responsiveness_target_met": p95 <= 40,
            "visible_before_confirmation_samples": sum(s["visible_before_confirmation"] for s in samples),
            "samples": samples,
        }
    finally:
        stop(client)
        stop(host)


def local_case(args, impairment):
    with tempfile.TemporaryDirectory(prefix="atomic-quic-verification-") as temporary:
        directory = Path(temporary)
        port = unused_port()
        state = directory / "private"
        initialized = subprocess.run(
            [str(args.relay), "init", "--state", str(state),
             "--listen", f"127.0.0.1:{port}", "--advertise", f"127.0.0.1:{port}"],
            check=True, capture_output=True, text=True,
        )
        links = [line.strip() for line in initialized.stdout.splitlines()
                 if line.strip().startswith("atomic-bomberman://")]
        if len(links) != 1:
            raise RuntimeError("Relay did not produce a private server link")
        link = links[0]
        relay = proxy = None
        try:
            relay = subprocess.Popen(
                [str(args.relay), "serve", "--state", str(state)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            deadline = time.monotonic() + 10
            while True:
                if relay.poll() is not None:
                    raise RuntimeError("Disposable relay failed to start")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=.1):
                        break
                except OSError:
                    if time.monotonic() > deadline:
                        raise RuntimeError("Disposable relay listener timed out")
                    time.sleep(.02)
            proxy = ImpairmentProxy(port, **impairment)
            parsed = urlsplit(link)
            link = urlunsplit((parsed.scheme, f"127.0.0.2:{port}", parsed.path, parsed.query, parsed.fragment))
            result = run_pair(args.example, directory, link)
            assert proxy.forwarded > 0, "No encrypted UDP traffic crossed the proxy"
            if impairment["loss_percent"]:
                assert proxy.dropped > 0, "Configured UDP loss was not exercised"
            if impairment["rtt_ms"] >= 40:
                assert result["visible_before_confirmation_samples"] >= 5, "Delayed controls did not respond before confirmation"
            result.update({
                "environment": "Disposable TLS/QUIC relay, native loopback through opaque packet proxy",
                "injected_gameplay_rtt_ms": impairment["rtt_ms"],
                "jitter_ms": impairment["jitter_ms"],
                "udp_loss_percent": impairment["loss_percent"],
                "udp_packets_received": proxy.received,
                "udp_packets_forwarded": proxy.forwarded,
                "udp_packets_dropped": proxy.dropped,
            })
            return result
        finally:
            if proxy is not None:
                proxy.close()
            stop(relay)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--example", type=Path, default=ROOT / "target/debug/examples/network_latency")
    parser.add_argument("--relay", type=Path, default=ROOT / "target/release/bomberman-relay")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--invite-stdin", action="store_true")
    parser.add_argument("--rtt-ms", nargs="+", type=float, default=[0, 40, 80, 160])
    parser.add_argument("--skip-loss-case", action="store_true", help="Skip the additional 80ms RTT/20ms jitter/5%% UDP loss case")
    parser.add_argument("--strict-latency", action="store_true", help="Fail if p95 sampled presentation latency exceeds40ms")
    args = parser.parse_args()
    args.example = args.example.resolve()
    args.relay = args.relay.resolve()
    if not args.example.is_file():
        parser.error("Build the production network_latency example first")
    if args.live != args.invite_stdin:
        parser.error("Live verification requires both --live and --invite-stdin")
    if any(not math.isfinite(rtt) or rtt < 0 for rtt in args.rtt_ms):
        parser.error("RTT values must be finite and nonnegative")
    if args.live:
        link = sys.stdin.read().strip()
        if not link:
            parser.error("Private server link must arrive on stdin")
        with tempfile.TemporaryDirectory(prefix="atomic-quic-live-") as temporary:
            case = run_pair(args.example, Path(temporary), link)
            case.update({"environment": "Deployed relay; real network; no injected impairment", "injected_gameplay_rtt_ms": None})
            cases = [case]
    else:
        if not args.relay.is_file():
            parser.error("Build bomberman-relay first")
        scenarios = [{"rtt_ms": rtt, "jitter_ms": 0, "loss_percent": 0} for rtt in args.rtt_ms]
        if not args.skip_loss_case:
            scenarios.append({"rtt_ms": 80, "jitter_ms": 20, "loss_percent": 5})
        cases = []
        for impairment in scenarios:
            case = local_case(args, impairment)
            cases.append(case)
            print(f"QUIC RTT={impairment['rtt_ms']:g}ms loss={impairment['loss_percent']:g}%: p95 visible={case['sample_to_visible_ms']['p95']:.2f}ms; matching canonical state", flush=True)
    value = {
        "passed": all(case["passed"] for case in cases),
        "environment": f"{platform.system()} {platform.machine()}",
        "responsiveness_target_met": all(case["responsiveness_target_met"] for case in cases),
        "cases": cases,
        "limitations": [
            "Measures production predicted render_position sampled at60Hz, without an actual graphics window or physical input device.",
            "Injected RTT is the configured four-leg gameplay path delay; observed application probe RTT includes endpoint scheduling.",
            "Loopback impairment does not establish physical LAN or Linux/macOS hardware interoperability.",
        ],
    }
    serialized = json.dumps(value, indent=2) + "\n"
    assert "atomic-bomberman://" not in serialized and '"token"' not in serialized
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(serialized)
    print(json.dumps({"passed": value["passed"], "responsiveness_target_met": value["responsiveness_target_met"], "cases": len(cases)}))
    return 0 if value["passed"] and (not args.strict_latency or value["responsiveness_target_met"]) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, RuntimeError, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"QUIC verification failed: {error}", file=sys.stderr)
        raise SystemExit(1)
