#!/usr/bin/env python3
"""Verify a relay executable over real TLS, without publishing its invitation."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import sys
import re
import tomllib
from urllib.parse import urlsplit, parse_qs

ROOT = Path(__file__).resolve().parents[1]
APP_VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
PROTOCOL = int(re.search(r"PROTOCOL_VERSION: u32 = (\d+)", (ROOT / "crates/bomberman-net/src/protocol.rs").read_text())[1])

def parse_invitation(text):
    text = text.strip()
    if text.startswith("AB1."):
        value = text[4:]
        return json.loads(base64.urlsafe_b64decode(value + "=" * ((-len(value)) % 4)))
    url = urlsplit(text)
    if url.scheme != "atomic-bomberman" or not url.hostname or url.username or url.password or url.query or url.path != "/":
        raise ValueError("Invalid private server URL")
    fields = parse_qs(url.fragment, strict_parsing=True)
    if any(len(values) != 1 for values in fields.values()) or fields.get("v") != ["1"] or set(fields) - {"v","pin","token","room"}:
        raise ValueError("Invalid private server URL fields")
    host = f"[{url.hostname}]" if ":" in url.hostname else url.hostname
    fingerprint = bytes.fromhex(fields["pin"][0])
    if len(fingerprint) != 32: raise ValueError("Invalid certificate pin")
    return {"server":{"endpoint": f"wss://{host}:{url.port or 443}/v1/ws", "fingerprint":list(fingerprint), "token":fields["token"][0]}, "room":fields.get("room",[None])[0]}


class WebSocket:
    def __init__(self, port, fingerprint, host="127.0.0.1"):
        raw = socket.create_connection((host, port), timeout=10)
        self.socket = ssl._create_unverified_context().wrap_socket(raw, server_hostname='atomic-relay.invalid')
        if hashlib.sha256(self.socket.getpeercert(binary_form=True)).digest() != bytes(fingerprint):
            self.socket.close()
            raise ValueError('Certificate pin mismatch')
        self.sent = self.received = 0
        key = base64.b64encode(os.urandom(16)).decode()
        request = (f'GET /v1/ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n'
                   f'Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n')
        self.socket.sendall(request.encode())
        header = b''
        while not header.endswith(b'\r\n\r\n'):
            header += self.socket.recv(1)
            assert len(header) < 8192
        assert b'101 Switching Protocols' in header
        accept = base64.b64encode(hashlib.sha1((key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        assert accept in header

    def send_frame(self, opcode, payload):
        mask = os.urandom(4)
        length = len(payload)
        prefix = bytes([0x80 | opcode, 0x80 | (length if length < 126 else 126 if length < 65536 else 127)])
        if length >= 126:
            prefix += struct.pack('!H' if length < 65536 else '!Q', length)
        wire = prefix + mask + bytes(byte ^ mask[i % 4] for i, byte in enumerate(payload))
        self.socket.sendall(wire)
        self.sent += len(wire)

    def send(self, request):
        self.send_frame(2, json.dumps(request, separators=(',', ':')).encode())

    def read(self, count):
        result = b''
        while len(result) < count:
            chunk = self.socket.recv(count-len(result))
            if not chunk:
                raise EOFError('relay connection closed')
            result += chunk
        self.received += count
        return result

    def receive(self):
        while True:
            first, second = self.read(2)
            assert first & 0x80, 'fragmented server message'
            length = second & 127
            if length in (126, 127):
                length = struct.unpack('!H' if length == 126 else '!Q', self.read(2 if length == 126 else 8))[0]
            assert not second & 128 and length <= 512*1024
            payload = self.read(length)
            opcode = first & 15
            if opcode == 9:
                self.send_frame(10, payload)
            elif opcode == 10:
                continue
            elif opcode == 2:
                return json.loads(payload)
            else:
                raise EOFError('relay connection closed')

    def open(self, invitation, room=None, token=None):
        self.send({'Open': {'token': token or invitation['server']['token'], 'room': room,
                           'compatibility': {'protocol': PROTOCOL, 'application': APP_VERSION, 'assets': 'a'*64}}})
        return self.receive()

    def close(self):
        self.socket.close()


class LatencyProxy:
    """Inject a fixed delay in each TCP direction without changing TLS bytes."""
    def __init__(self, target, milliseconds):
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0)); self.listener.listen(32)
        self.port = self.listener.getsockname()[1]
        self.listener.settimeout(.2)
        self.target, self.delay = target, milliseconds/2000
        self.stopped = threading.Event()
        self.streams = []
        threading.Thread(target=self.accept, daemon=True).start()

    def forward(self, source, destination):
        try:
            while not self.stopped.is_set():
                data = source.recv(65536)
                if not data: break
                time.sleep(self.delay)
                destination.sendall(data)
        except OSError:
            pass
        finally:
            for stream in (source, destination):
                try: stream.shutdown(socket.SHUT_RDWR)
                except OSError: pass
                stream.close()

    def accept(self):
        while not self.stopped.is_set():
            try:
                incoming, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                outgoing = socket.create_connection(('127.0.0.1', self.target), timeout=5)
            except OSError:
                incoming.close()
                continue
            outgoing.settimeout(None)
            self.streams.extend([incoming,outgoing])
            threading.Thread(target=self.forward,args=(incoming,outgoing),daemon=True).start()
            threading.Thread(target=self.forward,args=(outgoing,incoming),daemon=True).start()

    def close(self):
        self.stopped.set(); self.listener.close()
        for stream in self.streams:
            try: stream.shutdown(socket.SHUT_RDWR)
            except OSError: pass
            stream.close()


def cpu_seconds(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().split()
    return (int(fields[13])+int(fields[14])) / os.sysconf('SC_CLK_TCK')


def memory_kib(pid):
    fields = Path(f'/proc/{pid}/status').read_text().splitlines()
    return int(next(line.split()[1] for line in fields if line.startswith('VmHWM:')))


def verify_live(invitation, args):
    endpoint = urlsplit(invitation['server']['endpoint'])
    clients = []
    def connect(pin=None):
        connection = WebSocket(endpoint.port or 443, pin or invitation['server']['fingerprint'], endpoint.hostname)
        clients.append(connection)
        return connection
    def directory(connection):
        connection.send({'Browse': {'token': invitation['server']['token'], 'compatibility': {'protocol':PROTOCOL,'application':APP_VERSION,'assets':'a'*64}}})
        response = connection.receive()
        assert 'Rooms' in response, 'Directory authentication failed'
        return response['Rooms']
    def rooms(connection):
        connection.send('ListRooms')
        return connection.receive()['Rooms']
    try:
        browser = connect(); before = directory(browser)
        rejected = connect()
        assert rejected.open(invitation, token=base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip('=')) == {'Error':'Unauthorized'}
        try:
            connect([0]*32)
        except ValueError:
            pass
        else:
            raise AssertionError('Wrong certificate pin was accepted')
        host = connect(); welcome = host.open(invitation)['Welcome']; code = welcome['room']
        listed = next(room for room in rooms(browser) if room['code'] == code)
        assert listed['computers'] == 1 and not listed['playing']
        friend = connect(); assert friend.open(invitation,code)['Welcome']['id'] == 2
        assert host.receive() == {'PeerJoined':2}
        isolated = connect(); other_code = isolated.open(invitation)['Welcome']['room']
        sample = {'direction':None,'direction_mask':0,'bomb':False,'action':False}
        latencies = []
        for tick in range(1,args.frames+1):
            started = time.perf_counter()
            friend.send({'Send':{'to':1,'message':{'Input':{'sequence':tick,'seats':[{'seat':1,'input':sample}]}}}})
            incoming = host.receive()['Message']
            assert incoming['from'] == 2 and incoming['message']['Input']['sequence'] == tick
            host.send({'Send':{'to':None,'message':{'Frame':{'tick':tick,'inputs':[sample]*2,'actions':[],'hash':None}}}})
            assert friend.receive()['Message']['message']['Frame']['tick'] == tick
            latencies.append((time.perf_counter()-started)*1000)
        isolated.send('Ping')
        assert isolated.receive() == 'Pong', 'Traffic crossed room boundaries'
        host.send({'Playing':True}); host.send('Ping'); assert host.receive() == 'Pong'
        assert next(room for room in rooms(browser) if room['code'] == code)['playing']
        late = connect(); assert late.open(invitation,code) == {'Error':'InProgress'}
        host.send('Leave'); assert friend.receive() == {'Error':'HostLeft'}
        isolated.send('Leave'); assert isolated.receive() == {'Error':'HostLeft'}
        after = rooms(browser)
        assert not {code,other_code} & {room['code'] for room in after}
        assert {room['code'] for room in before} <= {room['code'] for room in after}, 'Unrelated room disappeared'
        latencies.sort()
        report = {'passed':True,'version':APP_VERSION,'protocol':PROTOCOL,
                  'environment':'External TCP/TLS/WebSocket connection to deployed relay',
                  'port':endpoint.port or 443,'frames':args.frames,
                  'input_and_frame_round_trip_ms':{'median':latencies[len(latencies)//2],'p95':latencies[min(len(latencies)-1,int(len(latencies)*.95))]},
                  'checks':['TLS certificate pin','invalid pin rejected','invalid credentials rejected','authenticated directory',
                            'new hosts appear without invites','joining discovered room','client inputs reach host',
                            'ordered host frames reach client','two isolated rooms','playing status and late-join rejection',
                            'host departure','temporary rooms removed'],
                  'limitations':['Protocol smoke test; graphical Mac/Linux gameplay is verified separately.']}
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report,indent=2))
    finally:
        for connection in clients: connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true', help='Test an existing relay without restarting or rotating its credentials')
    parser.add_argument('--invite-stdin', action='store_true', help='Read the private server URL from stdin')
    parser.add_argument('--binary', type=Path, default=Path('target/release/bomberman-relay'))
    parser.add_argument('--emulator', type=Path)
    parser.add_argument('--sysroot', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--latency-ms', type=float, default=0)
    parser.add_argument('--frames', type=int, default=250)
    parser.add_argument('--clients-per-room', type=int, default=9, choices=range(1, 10))
    args = parser.parse_args()
    if args.live:
        if not args.invite_stdin: parser.error('--live requires --invite-stdin')
        verify_live(parse_invitation(sys.stdin.read()), args)
        return
    command = [str(args.binary.resolve())]
    if args.emulator:
        command = [str(args.emulator.resolve()), '-L', str(args.sysroot.resolve()), *command]
    with tempfile.TemporaryDirectory(prefix='atomic private relay ') as temporary:
        root = Path(temporary)
        state = root / 'state'
        with socket.socket() as reserve:
            reserve.bind(('127.0.0.1', 0))
            port = reserve.getsockname()[1]
        common = ['--state', str(state)]
        subprocess.run([*command, 'init', *common, '--advertise', f'127.0.0.1:{port}', '--listen', f'127.0.0.1:{port}'], check=True, capture_output=True)
        for file in state.iterdir():
            assert file.stat().st_mode & 0o777 == 0o600
        cert = (state / 'certificate.der').read_bytes()
        assert subprocess.run([*command, 'init', *common, '--advertise', '127.0.0.1:443'], capture_output=True).returncode != 0
        assert cert == (state / 'certificate.der').read_bytes()

        def invite():
            encoded = subprocess.check_output([*command, 'export-invite', *common], text=True).strip()
            return parse_invitation(encoded)

        invitation = invite()
        proxy = LatencyProxy(port, args.latency_ms) if args.latency_ms else None
        if proxy: port = proxy.port
        log = (root / 'relay.log').open('w+')
        process = None
        clients = []

        def start():
            process = subprocess.Popen([*command, 'serve', *common], stdout=log, stderr=log)
            deadline = time.monotonic()+10
            while time.monotonic() < deadline:
                assert process.poll() is None, 'relay exited before accepting TLS'
                try:
                    client = WebSocket(port, invitation['server']['fingerprint'])
                    client.close()
                    return process
                except (ConnectionRefusedError, ConnectionResetError, ssl.SSLEOFError, TimeoutError):
                    time.sleep(.05)
            raise AssertionError('relay failed to start')

        try:
            process = start()
            rooms = []
            for _ in range(2):
                host = WebSocket(port, invitation['server']['fingerprint']); clients.append(host)
                welcome = host.open(invitation)['Welcome']
                friends = []
                for peer in range(2, args.clients_per_room+2):
                    friend = WebSocket(port, invitation['server']['fingerprint']); clients.append(friend)
                    assert friend.open(invitation, welcome['room'])['Welcome']['id'] == peer
                    assert host.receive() == {'PeerJoined': peer}
                    friends.append(friend)
                rooms.append((host, friends, welcome['room']))
            if args.clients_per_room == 9:
                extra = WebSocket(port, invitation['server']['fingerprint']); clients.append(extra)
                assert extra.open(invitation, rooms[0][2]) == {'Error': 'Full'}
            sample = {'direction': 'Down', 'direction_mask': 4, 'bomb': True, 'action': False}
            latencies = []
            cpu_before = cpu_seconds(process.pid)
            started = time.perf_counter()
            for tick in range(1, args.frames+1):
                for host, friends, _ in rooms:
                    before = time.perf_counter()
                    frame = {'Frame': {'tick': tick, 'inputs': [sample]*10, 'actions': [], 'hash': None}}
                    host.send({'Send': {'to': None, 'message': frame}})
                    for friend in friends:
                        delivery = friend.receive()
                        assert 'Message' in delivery, delivery
                        event = delivery['Message']
                        assert event['from'] == 1 and event['message']['Frame']['tick'] == tick
                    latencies.append((time.perf_counter()-before)*1000)
                    # Nine remote computers each return their own seat's
                    # controls through an outbound-only connection.
                    for peer, friend in enumerate(friends, 2):
                        friend.send({'Send': {'to': 1, 'message': {'Input': {'sequence': tick, 'seats': [{'seat': peer-1, 'input': sample}]}}}})
                    received = set()
                    for _ in friends:
                        event = host.receive()['Message']
                        assert event['message']['Input']['sequence'] == tick
                        received.add(event['from'])
                    assert received == set(range(2, args.clients_per_room+2))
                time.sleep(max(0, started+tick*.020-time.perf_counter()))
            elapsed = time.perf_counter()-started
            cpu = cpu_seconds(process.pid)-cpu_before
            rss = memory_kib(process.pid)
            bandwidth = sum(c.sent+c.received for c in clients)/elapsed
            # A stalled peer resumes with intact ordered messages; no inbound
            # listening socket exists on any of these game connections.
            host, friends, _ = rooms[0]
            for tick in range(args.frames+1, args.frames+6):
                host.send({'Send': {'to': None, 'message': {'Frame': {'tick': tick, 'inputs': [sample]*10, 'actions': [], 'hash': None}}}})
            time.sleep(.08)
            for friend in friends:
                for tick in range(args.frames+1, args.frames+6):
                    assert friend.receive()['Message']['message']['Frame']['tick'] == tick
            friends[0].close()
            assert host.receive() == {'PeerLeft': 2}
            rooms[1][0].send('Leave')
            for friend in rooms[1][1]:
                assert friend.receive() == {'Error': 'HostLeft'}
            process.terminate(); process.wait(timeout=10); process = None
            subprocess.run([*command, 'rotate-token', *common], check=True, capture_output=True)
            renewed = invite()
            assert renewed['server']['fingerprint'] == invitation['server']['fingerprint']
            assert renewed['server']['token'] != invitation['server']['token']
            process = start()
            old = WebSocket(port, invitation['server']['fingerprint']); clients.append(old)
            assert old.open(invitation) == {'Error': 'Unauthorized'}
            new = WebSocket(port, renewed['server']['fingerprint']); clients.append(new)
            assert 'Welcome' in new.open(renewed)
            log.flush(); log.seek(0); logs = log.read()
            assert invitation['server']['token'] not in logs
            assert renewed['server']['token'] not in logs
            assert invitation['server']['endpoint'] not in logs
            latencies.sort()
            report = {'passed': True, 'version': APP_VERSION,
                      'environment': 'ARM64 QEMU user emulation on Ubuntu 24.04 x86-64' if args.emulator else 'Ubuntu 24.04 x86-64 native loopback',
                      'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                      'rooms': 2, 'outbound_connections': 2*(1+args.clients_per_room), 'remote_clients_per_room': args.clients_per_room, 'inputs_per_frame': 10, 'frames_per_room': args.frames, 'injected_round_trip_tcp_delay_ms': args.latency_ms,
                      'duration_seconds': elapsed, 'process_cpu_seconds': cpu, 'process_peak_rss_kib': rss,
                      'application_websocket_bytes_per_second': bandwidth,
                      'forward_latency_ms': {'median': latencies[len(latencies)//2], 'p95': latencies[int(len(latencies)*.95)], 'max': max(latencies)},
                      'checks': ['initialization and persistent TLS identity', 'private state permissions', 'real TLS and pinned certificate',
                                 'two concurrent isolated rooms', 'one server plus nine clients capacity (with default arguments)', 'client inputs forwarded to host', 'ordered authoritative frames with target 50 Hz', '80 ms receive interruption',
                                 'peer disconnection', 'host departure', 'token rotation and restart', 'credential-free logs'],
                      'limitations': ['Loopback measurements include client scheduling and TLS; not WAN latency.',
                                      'QEMU measurements include emulator overhead; not native ARM64 production capacity.',
                                      'Linux/macOS hardware interoperability and Hetzner deployment require those machines.']}
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2)+'\n')
            print(json.dumps(report, indent=2))
        finally:
            for client in clients:
                client.close()
            if process and process.poll() is None:
                process.terminate(); process.wait(timeout=10)
            log.close()
            if proxy: proxy.close()


if __name__ == '__main__':
    main()
