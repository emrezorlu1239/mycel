# Testing with friends over the internet

The automated tests prove the logic on simulated networks. This guide is the
final Phase 0 check: real routers, real ISPs.

## 0. Build on every computer

Install [Rust](https://rustup.rs). On Windows, also install *Visual Studio
Build Tools* with the "Desktop development with C++" workload. Then:

```powershell
cd C:\Dev\mycel
cargo build --release
```

The program is `target\release\mycel.exe` (Windows) or `target/release/mycel`.

## 1. Same Wi-Fi (easiest, no setup)

Host:

```powershell
.\target\release\mycel.exe host --server "Test" --nick emre
```

Copy the line starting with `mycel1`. On the second computer:

```powershell
.\target\release\mycel.exe join mycel1... --nick ali
```

Expected: `[path] DIRECT`, then `[joined]`. Type messages on both sides.

## 2. Different homes, with a relay

Someone needs an address the internet can reach. Pick one:

- **A friend who can forward a port** (UDP and TCP 4001 to their computer), or
- **A small cloud server** (any cheap Linux VPS works; open UDP/TCP 4001).

On that machine:

```bash
mycel relay --port 4001 --external /ip4/<PUBLIC-IP>/udp/4001/quic-v1 --external /ip4/<PUBLIC-IP>/tcp/4001
```

It prints `[relay-addr] /ip4/<PUBLIC-IP>/udp/4001/quic-v1/p2p/12D3KooW...`.
Copy the QUIC one.

Host (in another home):

```powershell
.\target\release\mycel.exe host --server "Test" --nick emre --relay /ip4/<PUBLIC-IP>/udp/4001/quic-v1/p2p/12D3KooW...
```

Wait for `[relay] reservation accepted`, then share the **latest** `[invite]`
line (it now contains the relay). The member joins as in step 1.

## 3. What to look for

| Line                                   | Meaning |
|----------------------------------------|---------|
| `[path] DIRECT peer=<host>`            | straight connection |
| `[path] RELAYED peer=<host>`           | going through the relay |
| `[holepunch] OK`                       | upgraded from relayed to direct |
| `[path] relay released, now DIRECT only` | relay no longer carries your chat |
| `[holepunch] FAILED`                   | your routers do not allow it; chat stays on the relay and still works |

Try at least these combinations and note the result:

1. Both on home Wi-Fi in different homes
2. One on home Wi-Fi, one on a phone hotspot (mobile networks are usually CGNAT)
3. Both on phone hotspots

Please record what you see in [phase-0-report.md](phase-0-report.md) under
"Real-world results".

## Troubleshooting

- **Nothing connects across homes, no relay given:** expected. Without a relay
  or a forwarded port, two home routers cannot reach each other.
- **Windows Firewall prompt:** allow access on private and public networks.
- **More detail:** set `RUST_LOG=info` (PowerShell: `$env:RUST_LOG="info"`).
