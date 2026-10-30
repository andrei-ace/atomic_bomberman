# Multiplayer responsiveness

The prepared **0.5.1** candidate uses protocol **7**, `ABG7`, QUIC ALPN
`atomic-bomberman/7` and compatibility family `0.5.1`. Clients and relay must
upgrade together; 0.5.0 and earlier are incompatible. Existing server links, certificates
and access tokens remain valid.

Each computer advances its own avatars immediately. The host accepts owner
motion and resolves shared interactions within a provisional tail bounded to
100 ms. Late facts are evaluated forward; committed history stays immutable.
Button transitions retain ordered identities, and confirmed control sounds do
not repeat. See [rationale, conflict rules and measurements](owner-authority.md).

### Historical 0.4.1 prediction timing and corrections

This section records the earlier control-prediction model for comparison with
the 0.5.1 owner-motion design above.

Movement ACKs identify the newest state selected by the host. They do not count
how many 20 ms movement steps the host simulated: several samples can coalesce
into one host step, and one held sample can drive several host steps. Previously,
ACKing sample 8 could collapse eight predicted steps to one confirmed step. An
empty-arena reproduction showed 30 pixels of movement falling back to 4 pixels.

Prediction now advances an independent 20 ms clock and retains a bounded history
of 256 local samples, including acknowledged samples. It rebuilds the recent
input window up to that clock instead of using the unacknowledged queue length
as elapsed time. Sample capture times estimate the prediction lead, with a 1/8
EWMA and clock alignment limited to 2 ms per local step. Existing RTT probes can
seed the lead during loading. Receive batches reconcile once; host catch-up
consumes one local sample per simulated step. Pending button edges retain their
separate acknowledgment and sound identities without adding movement time.

Local artwork blends small reconciliation offsets linearly over 80 ms. Normal
input movement does not pass through that blend. Physics and canonical hashes
never contain the offsets. Obstructed paths, corrections of a tile or more,
transit, death, pause, round changes and checkpoint recovery apply immediately.
Prediction and new sample generation stop after 500 ms without canonical tick
progress; duplicate traffic does not extend that deadline.

The previous implementation displayed only host frames. A client control
therefore crossed the client-to-host-to-client route before becoming visible.
Its whole-input FIFO also consumed only one changed sample per 20 ms step;
128 queued samples could represent 2.56 seconds of obsolete movement.

## Original binary observations

REA MCP with Ghidra 12.1.4 inspected the supplied `BM95.EXE`, SHA-256
`d1ff5f6fba03f20b4a161af1c75165b841e536cd9398ed2692202cee6d6b3857`.
These are static code observations, not measurements of the original game
running over a network. The focused investigation covered local controls,
movement encoding/decoding, bomb/action sending, packet queues, polling,
batching, acknowledgment, duplicate handling, and retransmission.

| Original behavior | Evidence | Reconstruction |
| --- | --- | --- |
| Local physics runs before movement is sent; type 4 carries six 16-bit values, only when state changes. | `0x41f29b`, `0x40ef4e`; `ev_273516e2065dff21946a00cd96227693aee89fc5c7a73cf8f54ec97dec66adc6`, `ev_328cb6c3f2e8f696d52bcd8bf87213781e0ae62920e78c3687437f460a19a754` | Local prediction; compact datagrams; latest held movement replaces older samples. |
| Bomb placement and sound occur before the network event. Trigger actions also mutate locally before sending. | `0x41eb13`, `0x420cbc`, `0x40f096`; `ev_450e3e8f636b4360d5b503b929525642bb4224c2cea9db46ca29185086c18d39`, `ev_216fa3a42b0b86269523d4812b3b23dde2f41f13e626e6ccc5d552f3b7c4d38c` | Predict local action feedback; keep outcomes authoritative; deduplicate confirmed sounds. |
| Polling occurs before world updates and after rendering; each receive pass drains available packets. | `0x42a20f`, `0x42a308`, `0x40e765`; `ev_3283b320e4f782ba85a779a6beda3fe109cfd862faabe5aaa91bce4d7ba1e902` | A background session clock and prompt receive draining avoid render stalls. |
| Movement bypasses critical-event reliability. Tokens 32 and above retain separate acknowledgment/retry state; duplicates are acknowledged but dispatched once. | `0x40ce27`, `0x40e765`, `0x40fbc2`; `ev_349b20ee52965b05c81c0669bbc2da8f28dba806bbe3578168f4095363a9d5b8`, `ev_e922a982e5a2bf08b26ba642a0161dec4957b94e9f087594e74d5581b7b8ec2d` | Movement, ordered button edges, and authoritative frame recovery have separate identities. Lost data does not hold newer datagrams behind a TCP stream. |
| Tokens share a compact batch, flushed near 512 bytes and at network-tick completion. No separate critical-priority queue was established. | `0x40cd1c`, `0x40ccc1`; `ev_fab4790f7d6e77e5091a813dffa33ce47a1d2fd7338246b99de541af11973f86` | Publish every 20 ms, include recent complete frames when they fit, and never wait for a packet to fill. |
| Critical retries scan 500 retained events without blocking movement. Resource 1100 + protocol selects IPX 200 ms, modem 750 ms, serial 400 ms; retries end after 25 attempts. | `0x40ea1e`; `ev_21be108153f671877bc518e5b180c16899be024e8edd9c6e20aa07eef5cbc051` | Use modern bounded recovery independently of current movement; do not import legacy modem timeout values. |
| Remote players read cached coordinates, direction, and animation directly. No interpolation buffer was observed in that path. The 30-entry direction history belongs to arena ice behavior. | `0x41e61e`, `0x40f3e9`; `ev_72859a860fb0f132b010bd3db213bf4d8b249fcb58fb5236fe7017c27d9c6fb9` | Preserve the core's ice rules; use generation/sequence validation and bounded display prediction for modern Internet play. |

The new encrypted transport, host authority, sequence validation, prediction
reconciliation, and frame checkpoints are modern design choices. They do not
claim wire compatibility with the original IPX/modem/serial protocol.

### Focused Ghidra follow-up for 0.4.1

The same executable and Ghidra version were inspected again without executing
the original game. Sender `0x40ef4e` builds exactly 12 bytes for token 4: a seat
index and five signed 16-bit values. The caller at `0x420cfe–0x420d40` supplies
active state, x/y, direction and animation state. Initialization at
`0x40e515–0x40e51f` registers receiver `0x40d519`; instruction inspection through
`0x40d5ed` shows validation and direct replacement of five cache arrays. That
handler contains no movement-duration field, command-history integration, or
sample-ACK comparison. Remote actor code at `0x41e8ad–0x41e8ef` copies the cached
coordinates, direction and animation directly. Input kind 4 is excluded from
resending movement at `0x420cf5–0x420cfc`.

Retained REA evidence: registration
`ev_a87504d24559b519fc43c3fc81ea3750b9eb65748a2ad1f4892f8854c6c339a4`,
receiver bytes
`ev_6431f0210e22687d33181f79f7ff19eec31b42368a3b511398ff89c47c4e1b5b`,
cache write
`ev_82df6a4bf6f791b86b2cf188155e4344574eb25eb2a91e376a0be608d427a968`,
remote actor path
`ev_166c5b58396daffc120fcc5a5e44e4adcae26d63fce9f3d876e35ebc7babe0a7`,
and critical retry/composition batch
`ev_43f063814550e0f97f8058d42369f77f749cbcc13ee9a7dba081ca2224f5ad59`.
The receiver callback lacks a Ghidra function definition, so its exact decoded
instructions were examined instead of assuming decompiler function coverage.

These static findings support keeping state supersession separate from critical
event identity. The independent clock and 80 ms blend are modern design choices.
This pass does not measure original runtime latency or establish the complete
packet envelope's ordering, authority or security policy; modern sequence
rejection remains necessary.

## Transport and recovery

Internet play uses pinned QUIC datagrams on UDP 443 and existing TLS WebSocket
control on TCP 443. LAN uses TCP 1997, discovery UDP 1997, and gameplay UDP 1998.
Every player must complete a bidirectional gameplay probe before the host begins.
There is no TCP gameplay fallback.

Gameplay datagrams fit within 1,000 application bytes. Each authoritative
packet includes its newest frame and up to two earlier complete frames. A
256-frame reorder/history window and requested reliable gap repair preserve
deterministic replay. Older gaps recover through a verified full checkpoint.
Prediction stops after 500 ms without canonical tick progress; a failed UDP path ends
the affected connection instead of silently switching gameplay to TCP.

Owner movement uses the newest motion facts; late facts are applied forward.
Committed outcomes cannot be revised. Bomb/action transitions retain ordered
IDs and repeat until consumed; each seat has a maximum of 16 pending edges.
Controls release after 250 ms without fresh observations, and disconnected
players become bots after five seconds.

See [online setup](online-play.md), [relay setup](relay.md), and
[release validation](releases.md). The datagram verifier reports injected
gameplay RTT separately from measured round-trip and input-to-presentation
latency. Headless presentation checks do not establish physical display or
audio-device latency.

Protocol 7 reliable `Repair.frames` is a bounded base64 string containing a
little-endian u16 frame count, then a u16 byte length and the gameplay frame
encoding for each frame. The limit remains 256 frames, each at most 1,000 bytes;
normal host replies chunk at 32 frames. All frames must decode completely with
no trailing bytes. This preserves the existing control rate and byte limits
under simultaneous recovery.
