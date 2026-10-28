# Native private relay — 0.2.0

The native ARM64 package targets Ubuntu 26 on the existing CAX11 and works with
glibc 2.39 or newer. No Docker, domain, certificate authority, database or original
game assets are required. TLS terminates inside the Rust service. It forwards
bounded messages between outbound clients; the player's computer hosts simulation.

The server address is entered **locally during setup**. Keep state and invitations
outside the checkout and release artifacts. The example config intentionally has
an unconfigured placeholder. Clients have no default endpoint.

## Install

Copy the ARM64 package and its checksum to the server. Check the checksum before
extracting. Run the following from its extracted directory:

```sh
sha256sum -c atomic-bomberman-0.2.0-relay-linux-arm64.tar.gz.sha256
sudo useradd --system --home-dir /var/lib/bomberman-relay \
  --shell /usr/sbin/nologin bomberman-relay
sudo install -d -m755 /usr/local/lib/bomberman-relay
sudo install -m755 bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay
sudo install -m644 bomberman-relay.service /etc/systemd/system/
sudo install -d -m700 -o bomberman-relay -g bomberman-relay /var/lib/bomberman-relay
read -r -p 'Private advertised address (IP:443): ' relay_address
sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay init \
  --state /var/lib/bomberman-relay/private --advertise "$relay_address"
unset relay_address
sudo systemctl daemon-reload
sudo systemctl enable --now bomberman-relay
sudo systemctl status bomberman-relay
```

Permit **inbound TCP 443** in the Hetzner firewall and Ubuntu firewall. Keep your
SSH access allowed. No UDP port or game port is needed for online relay play.
An existing service on 443 must be moved before starting this service. The unit
uses a dedicated user and grants only `CAP_NET_BIND_SERVICE` for privileged
binding. State is mode 0700; keys, token and config are mode 0600. The process has
read-only system access, restricted address families, a 256 MiB memory limit,
64-task limit and 1024-descriptor limit.

Initialize once: `init` refuses to replace persistent state. It generates an
ECDSA TLS key, a self-signed certificate and a random 256-bit access token. The
certificate contains a generic name, independent of the advertised address.
Export the **private server invitation** over your private SSH session:

```sh
sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay export-invite \
  --state /var/lib/bomberman-relay/private
```

Paste the single output string into **Create Online Game**. The host's **Copy
Invite** includes the room code for friends. Do not include this output in logs,
commits, screenshots, support tickets or a public release. All invited clients
receive the destination address and can inspect it; privacy means excluding it
from published artifacts and defaults, not hiding it from invited friends.

## Updates and rotation

Verify each new package's checksum and provenance. Install a new binary to a
separate temporary filename, atomically rename it over the installed executable,
and restart the service. Keep the previous binary for rollback. Keep the private
state directory. Existing saved client profiles continue to work with the same
certificate and token; app/protocol versions must match within a room.

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
keys. Client errors distinguish unavailable servers, pin mismatch, revoked
credentials, expired/full rooms and incompatible versions/assets.

Defaults: 64 rooms, 128 sockets (limited **before TLS**), ten connections per
room, five-second TLS/authentication deadlines, 30-second receive timeout,
512 KiB maximum messages, 128-message outgoing queues with a shared 16 MiB queued-data budget, 240 incoming messages and
2 MiB per connection per second, and 60 WebSocket admissions per source IP per
minute. The admission map is bounded to 1024 IPs. Slow/invalid peers close rather
than growing queues. Host-only broadcasts and relay-assigned sender identities
prevent clients from impersonating a host or crossing rooms. Tune the two
configuration capacity limits only within their validated bounds.

TLS clients pin the SHA-256 of the certificate from the invitation. They also
verify TLS 1.2/1.3 handshake signatures using Rustls's cryptographic provider;
this is not an accept-all certificate verifier. The exact pin replaces DNS-name,
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
