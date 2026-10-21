# Architecture

This document describes Mycel as it stands after Phase 0 and the decisions
behind it. Later phases extend it; see [roadmap.md](roadmap.md).

## Goals and non-goals

**Goals**

1. No central point that can be blocked or shut down: no company server, no
   account system, no required domain name.
2. Usable by people who have never opened a terminal or a router page.
3. The host owns the server: membership, data and the ability to hand it over.
4. Data lives only on participants' devices, under their control.

**Non-goals**

- Moderation tooling or content rules. Groups are people who know each other;
  the host decides.
- Anonymity against a global observer. Mycel hides *what* you say, not the fact
  that two IP addresses talk. Users who need that can run it over Tor or a VPN.

## Roles

| Role       | Who                         | Does                                                  |
|------------|-----------------------------|-------------------------------------------------------|
| **Host**   | one member, the admin       | owns the server, accepts members, relays chat         |
| **Member** | everyone else in the group  | joins with an invite, sends and receives messages     |
| **Relay**  | any node with a reachable address | forwards encrypted traffic when two peers cannot connect directly |

A relay sees only encrypted bytes. It cannot read messages or pretend to be
the host, because every connection is authenticated with the peers' keys.

## Identity

`mycel-core/src/identity.rs`

- Each device generates an Ed25519 keypair on first start and stores it as
  `identity.key` in its data directory (file mode `0600` on Unix).
- The public key's hash is the device's `PeerId`, e.g.
  `12D3KooW…`. It is the only identifier Mycel uses.
- Losing the key file means losing that identity; Phase 1 adds export/backup.

## Invites

`mycel-core/src/invite.rs`

An invite is `mycel1` + Base58(JSON):

```json
{ "v": 1, "server": "Friday Night", "host": "12D3KooW…",
  "addrs": ["/ip4/192.168.1.10/udp/50451/quic-v1", "/ip4/203.0.113.10/udp/4001/quic-v1/p2p/<relay>/p2p-circuit/p2p/<host>"] }
```

- Self-contained: no lookup service is needed to use it.
- Contains the host's `PeerId`, so a member refuses to talk to anyone else at
  those addresses (tested: `member_refuses_an_impostor_host`).
- Address order: confirmed public addresses, then LAN addresses, then relayed
  circuit addresses; QUIC before TCP inside each group because only QUIC
  connections can be hole-punched.
- The host waits ~1.5 s for its address list to settle before publishing an
  invite, so nobody copies a half-ready code.

## Connectivity

`mycel-core/src/node.rs`, built on rust-libp2p.

| Layer          | Choice                                   | Why |
|----------------|------------------------------------------|-----|
| Transports     | QUIC v1 (UDP) and TCP                    | QUIC punches through NATs; TCP works where UDP is blocked |
| Encryption     | TLS 1.3 (QUIC), Noise XX (TCP)           | always on, no configuration |
| Multiplexing   | QUIC streams / Yamux                     | many conversations over one connection |
| LAN discovery  | mDNS                                     | same Wi-Fi works with zero setup |
| NAT traversal  | Circuit Relay v2 + DCUtR                 | relayed first, then upgraded to direct |
| Liveness       | Ping every 15 s, identify                | detect dead links, learn observed addresses |

Connection strategy for a member:

1. Dial every address in the invite at once, plus each relay named in it
   (preferring QUIC). The relay connection also tells the member its public UDP
   address, which hole punching needs.
2. Whichever connection succeeds first is used to join.
3. If the connection is relayed, DCUtR tries to open a direct one. On success
   the relayed connection is closed after a 2 s drain, so the relay stops
   carrying traffic.
4. If the host disappears, the member redials every 3 s and re-joins
   automatically when it comes back.

Why the relay connection is closed after a punch: libp2p's request-response
spreads requests round-robin over all open connections, so leaving the relayed
one open would keep sending half the messages through the relay.

### What NATs allow (from the simulator)

| Host's network       | Member's network       | Result |
|----------------------|------------------------|--------|
| same LAN             | same LAN               | direct |
| port forwarded       | anything               | direct |
| home router (cone)   | home router (cone)     | relayed, then hole-punched to direct |
| home router          | mobile / CGNAT (symmetric) | relayed |
| symmetric            | symmetric              | relayed |
| home router, no relay | other home, no relay  | **cannot connect** – why relays exist |

## Chat protocol

`mycel-core/src/protocol.rs`, libp2p request-response with JSON, protocol id
`/mycel/chat/0.1.0`.

| Request                 | Direction       | Response                   |
|-------------------------|-----------------|----------------------------|
| `join { nick }`         | member → host   | `welcome { server, members }` or `denied` |
| `post { text }`         | member → host   | `ack` or `denied`          |
| `deliver { from, text }`| host → member   | `ack`                      |

- The host fans each post out to every other member; the sender does not get an
  echo.
- Messages typed before the member has joined are queued and sent after the
  welcome (tested).
- Limits: messages ≤ 4096 bytes, nicknames ≤ 32 characters with control
  characters stripped.

The host is the single authority for its server. This makes admin actions
(kick, ban, transfer) a local decision on one machine instead of a distributed
consensus problem. The trade-off, accepted by design: when the host is offline,
the server is offline.

## Code layout

```
crates/mycel-core/src/
  identity.rs   key generation and storage
  invite.rs     invite encoding/decoding and validation
  protocol.rs   request/response types, limits, nickname sanitising
  node.rs       libp2p swarm, event loop, host/member/relay logic
crates/mycel-cli/src/main.rs   `mycel` command; prints tagged lines like [path], [msg]
```

`mycel-core` exposes `start(NodeConfig) -> NodeHandle`. The caller sends
`Command`s (`Send`, `Shutdown`) and receives `NodeEvent`s. The CLI and the
future Tauri app are both thin layers over this API.

## Known limits of Phase 0

- **No automatic relay discovery.** Relays are given explicitly. Phase 1 adds a
  Kademlia DHT bootstrap (including the public libp2p/IPFS network) and lets any
  reachable member act as a relay for its group.
- **No persistence.** Messages exist only in memory and on screen.
- **No admin commands** (kick, ban, transfer) yet; the protocol is ready for them.
- **TCP cannot be hole-punched** (rust-libp2p no longer supports TCP port reuse);
  QUIC carries the punched connections.
- **Our relays allow 1 h / 1 GiB per circuit.** Public libp2p relays allow far
  less (2 min / 128 KiB), so long relayed sessions need our own relay mode.
