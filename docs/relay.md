# Native private relay — 0.4.1

The native ARM64 package targets Ubuntu 26 on the existing CAX11 and works with
glibc 2.39 or newer. No Docker, domain, certificate authority, database or original
game assets are required. The service forwards bounded traffic between outbound
clients; the player's computer hosts simulation. Version 0.4.1 keeps protocol 5 and the 0.4.0 handshake identifier;
existing 0.4.0 clients and relays remain compatible.

The relay listens on the same port number for two transports: secure WebSockets
over **TCP 443** for authentication, browsing, lobby, setup, and requested
recovery; encrypted QUIC datagrams over **UDP 443** for gameplay. Both are
required. Gameplay never falls back to TCP.

The server detects its public IP **locally during setup**; `--advertise` overrides it. Keep state and invitations
outside the checkout and release artifacts. The example config intentionally has
an unconfigured placeholder. Clients have no default endpoint.

## Install

Copy the ARM64 package and its checksum to the server. Check the checksum before
extracting. Run the following from its extracted directory:

```sh
sha256sum -c atomic-bomberman-0.4.1-relay-linux-arm64.tar.gz.sha256
sudo useradd --system --home-dir /var/lib/bomberman-relay \
  --shell /usr/sbin/nologin bomberman-relay
sudo install -d -m755 /usr/local/lib/bomberman-relay
sudo install -m755 bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay
sudo install -m644 bomberman-relay.service /etc/systemd/system/
sudo install -d -m700 -o bomberman-relay -g bomberman-relay /var/lib/bomberman-relay
sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay init \
  --state /var/lib/bomberman-relay/private
sudo systemctl daemon-reload
sudo systemctl enable --now bomberman-relay
sudo systemctl status bomberman-relay
```

The administrator must allow these inbound rules in the firewall attached to the
server, including the Hetzner Cloud Firewall when used:

| Protocol | Destination port | Sources |
| --- | --- | --- |
| TCP | 443 | Players' addresses, or `0.0.0.0/0` and `::/0` |
| UDP | 443 | Players' addresses, or `0.0.0.0/0` and `::/0` |

Keep existing SSH access allowed. Apply the same rules to a local firewall if
one is enabled. Players connect outbound on both transports and do not need
inbound ports. The server's TCP and UDP listener ports follow the configured
`listen` address; invitations use that port number for both transports.

An existing service occupying either listener must be moved before starting this
service. The unit
uses a dedicated user and grants only `CAP_NET_BIND_SERVICE` for privileged
binding. State is mode 0700; keys, token and config are mode 0600. The process has
read-only system access, restricted address families, a 256 MiB memory limit,
64-task limit and 1024-descriptor limit.

Initialize once: `init` refuses to replace persistent state. It generates an
ECDSA TLS key, a self-signed certificate and a random 256-bit access token. The
certificate contains a generic name, independent of the advertised address.
The **private server link is the players' access key**. It contains the server
address, access secret, and TLS certificate fingerprint. The private TLS key
remains on the server. Export the link over your private SSH session:

```sh
sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay export-invite \
  --state /var/lib/bomberman-relay/private
```

From another computer, the same command can be run with
`ssh root@your-server 'sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay export-invite --state /var/lib/bomberman-relay/private'`.

Copy the complete generated URL. On each client open **Multiplayer → Internet →
Paste server link**, paste with **Ctrl-V** on Linux or **Command-V** on Mac, and
select **Connect**. The link imports the access secret and certificate fingerprint
together; no manual certificate installation is needed. Everyone can then host
games and find the group's rooms in
**Friends' games** without further invitations. **Copy server link** shares this
same group link with another friend. The URL contains the reachable IP, random
secret, and pinned certificate fingerprint. Automatic detection prefers a unique
public IPv4 address, then a unique public IPv6 address. Behind NAT or with
multiple public addresses, pass `--advertise <host:port>` explicitly. Do not include this output in logs,
commits, screenshots, support tickets or a public release. All invited clients
receive the destination address and can inspect it; privacy means excluding it
from published artifacts and defaults, not hiding it from invited friends.

## Updates and rotation

Verify each new package's checksum and provenance. Install a new binary to a
separate temporary filename, atomically rename it over the installed executable,
and restart the service. Keep the previous binary for rollback. Keep the private
state directory. Existing saved client profiles continue to work with the same
certificate and token. Protocol 5 accepts 0.4.0 and 0.4.1 clients; clients from older protocols
receive a version error. Preserve `/var/lib/bomberman-relay/private` and do not
run `init` again during an update.

```sh
sudo install -m755 bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay.new
sudo mv /usr/local/lib/bomberman-relay/bomberman-relay.new /usr/local/lib/bomberman-relay/bomberman-relay
sudo systemctl restart bomberman-relay
```

To revoke previously distributed invitations:

```sh
sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay rotate-token \
  --state /var/lib/bomberman-relay/private
sudo systemctl restart bomberman-relay
```

Export and distribute a new server invitation. Old full invitations and saved
profiles fail authentication after restart. Rotation preserves the TLS identity;
rotating it as well requires a newly initialized state directory and new full
invitations. Never delete working state accidentally. Back it up privately with
restrictive permissions. A changed IP can be updated in the private config's
`advertised_endpoint`, followed by restart and new invitations.

Rooms exist only in memory. A restart or host departure closes active rooms;
there is no host migration, persistent matchmaking or spectator role. Rooms
expire after twelve hours. New clients and seat changes are limited to the lobby.

## Diagnostics and resource bounds

```sh
sudo journalctl -u bomberman-relay --since '15 minutes ago'
sudo systemctl show bomberman-relay -p MemoryCurrent -p CPUUsageNSec
```

Logs contain startup status and room/connection counts each minute, without
credentials or advertised endpoints. A bind failure indicates a port conflict or
missing capability; a TLS load failure indicates state permissions or damaged
keys. If clients can browse but cannot start, verify inbound UDP 443 in the cloud
firewall and outbound UDP 443 on their networks. TCP 443 must remain open. A
successful TCP connection alone does not establish UDP reachability. Client
errors distinguish unavailable servers, blocked UDP, pin mismatch, revoked
credentials, expired/full rooms and incompatible versions/assets.

Defaults: 64 rooms, 128 control connections, ten player computers per room,
five-second TLS/authentication deadlines, 30-second control receive timeout,
512 KiB maximum control messages, 128-message outgoing control queues with a shared
16 MiB queued-data budget, 240 incoming control messages and
2 MiB per connection per second, and 60 WebSocket admissions per source IP per
minute. The admission map is bounded to 1024 IPs. Slow/invalid peers close rather
than growing queues. Host-only broadcasts and relay-assigned sender identities
prevent clients from impersonating a host or crossing rooms. Gameplay connections
are bound to an authenticated control session using a single-use token and are
revoked when that session closes. Gameplay packets are limited to 1,000 bytes or
the negotiated datagram maximum, whichever is smaller; repeated frames and
consumed-input acknowledgments handle loss without accumulating stale movement.
Tune the two configuration capacity limits only within their validated bounds.

TLS clients pin the SHA-256 of the certificate from the invitation. They also
verify handshake signatures using Rustls's cryptographic provider. QUIC uses TLS
1.3; the WebSocket control connection supports TLS 1.2/1.3. The exact pin replaces DNS-name,
public-CA and expiry checks. See the
[Rustls verifier interface](https://docs.rs/rustls/0.23.31/rustls/client/danger/trait.ServerCertVerifier.html).

## Build natively on Ubuntu 26 ARM64

Install Rust **1.89.0** (or newer) and `build-essential pkg-config`. The relay
uses Rustls/ring and does not depend on OpenSSL or any desktop library.

```sh
cargo build --locked --release -p bomberman-relay
cargo test --locked -p bomberman-net -p bomberman-relay
python3 tools/package_relay.py --release
```

`--release` requires a clean committed checkout. The package includes the binary,
systemd unit, unconfigured example, instructions, licenses, SHA-256 and build
provenance. Cross-builds can use cargo-zigbuild 0.20.1, Zig 0.14.1 and Rust's
`aarch64-unknown-linux-gnu` target:

```sh
cargo zigbuild --locked --release --target aarch64-unknown-linux-gnu.2.39 -p bomberman-relay
python3 tools/package_relay.py --binary target/aarch64-unknown-linux-gnu/release/bomberman-relay
```

The release workflow builds and tests on a native ARM64 runner. Local ARM64
emulation establishes executable startup/operation, not Hetzner production
capacity. Deployment and latency across real Linux/macOS peers require the
private server address and machines. Do not describe unperformed production or
cross-platform tests as passing; record the environment with each measurement.

## Verify a deployed server

Build the production coordinator verifier on a client computer:

```sh
cargo build --locked -p bomberman --example network_latency
```

The live verifier opens a temporary room with one host and one client. It checks
encrypted QUIC gameplay, automatic preparation, predicted movement, shared pause,
and matching canonical simulation state. It records application probe RTT and
input-to-presentation latency at a 60 Hz sampling rate. This headless check reads
the same predicted `render_position` used by the renderer; it opens no graphics
window and does not measure a physical keyboard or display.

It does not restart the service, rotate its secret, or change firewalls. Pass the
link on stdin; do not put credentials in command-line arguments or reports:

```sh
ssh root@your-server 'sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay export-invite --state /var/lib/bomberman-relay/private' \
  | python3 tools/verify_datagram.py --live --invite-stdin --report /tmp/relay-quic-live.json
```

For a disposable local relay, build it and run the same verifier without the live
options:

```sh
cargo build --locked -p bomberman-relay
python3 tools/verify_datagram.py --relay target/debug/bomberman-relay \
  --report /tmp/relay-quic-loopback.json
```

The default local matrix injects 0, 40, 80, and 160 ms of gameplay round-trip
delay, plus a case with 80 ms delay, jitter, and 5% UDP packet loss. The report
separates injected delay from observed application RTT. Add `--strict-latency`
to make the 40 ms p95 presentation target a required pass; otherwise timing is
reported separately from functional correctness.
Rust relay integration tests cover WebSocket authentication, room isolation,
limits, and QUIC negotiation with disposable state.
Directory connections consume no room player slots.

Record gameplay latency using the production coordinator and QUIC datagrams.
TCP/WebSocket forwarding latency alone does not establish input-to-visible
response or UDP reachability. Report the tested client platforms, actual game
round-trip delay, packet impairment, and host/client replay checks separately.
