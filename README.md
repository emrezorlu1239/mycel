# Mycel

**Peer-to-peer chat for friend groups that nobody in between can switch off.**

[Türkçe](README.tr.md)

Mycel is a Discord-style chat app with no company server. One friend's computer
hosts the group, everyone else connects straight to it, and all traffic is
encrypted end to end on the wire. There is no account to ban, no central server
to block and no data stored anywhere except on your own devices.

The name comes from *mycelium*, the underground fungal network that links
trees in a forest: no centre, and cutting one strand does not stop the rest.

> **Status: Phase 0 (proof of concept).** The networking core works and is
> tested; there is no graphical app yet. See the [roadmap](docs/roadmap.md).

## Why

- **Unblockable by design.** No central server, domain or account system that
  can be taken down. Peers find each other with invite codes and connect
  directly, falling back to relays only when routers leave no other way.
- **Your group, your rules.** The host is the admin: they decide who joins and
  can hand the server over to someone else.
- **Your data stays home.** Messages and files live on the host's machine and
  on each member's device, in a folder they control and can wipe at any time.
  Live voice is never recorded.
- **Made for everyone.** The goal is an app your non-technical friends can
  install and use without touching a router setting.

## How it works

```
   Emre (host, admin)                       Ali (member)
   ┌──────────────┐   1. direct, if routers allow   ┌──────────────┐
   │ server data  │◄───────────────────────────────►│ local cache  │
   │ identity key │                                 │ identity key │
   └──────┬───────┘                                 └──────┬───────┘
          │ 2. otherwise through a relay ...              │
          └──────────────►  relay  ◄──────────────────────┘
                 3. ... then a hole punch upgrades it to direct
```

- **Identity:** each device generates an Ed25519 key. Its public half is the
  device's ID. No phone numbers, emails or passwords.
- **Invites:** a host shares a code like `mycel1…` that contains its ID and the
  addresses to try. Codes are self-contained and work without any lookup service.
- **Connectivity:** built on [libp2p](https://libp2p.io): QUIC and TCP
  transports, Noise/TLS encryption, circuit relays and DCUtR hole punching.
- **Chat:** members send to the host, the host fans messages out. The host is
  the single source of truth for the server, which keeps admin control simple.

Details: [docs/architecture.md](docs/architecture.md).

## Try it (Phase 0 CLI)

You need [Rust](https://rustup.rs) 1.88 or newer. On Windows, also install the
*Visual Studio Build Tools* with the "Desktop development with C++" workload.

```bash
cargo build --release
```

**On the host's computer:**

```bash
./target/release/mycel host --server "Friday Night" --nick emre
```

It prints an invite line starting with `mycel1`. Send it to your friend.

**On a friend's computer:**

```bash
./target/release/mycel join mycel1... --nick ali
```

Type a message and press Enter. Type `/quit` to leave.

On the same Wi-Fi this works as is. Across different homes, the host needs one
of these (Phase 0 does not yet find public relays automatically):

- a forwarded port: `--port 4001 --external /ip4/<your-public-ip>/udp/4001/quic-v1`, or
- a relay that someone with a reachable address runs with `mycel relay`, passed
  to the host as `--relay /ip4/<relay-ip>/udp/4001/quic-v1/p2p/<relay-id>`.

Step-by-step instructions for testing with a friend over the internet:
[docs/testing-with-friends.md](docs/testing-with-friends.md).

## Repository layout

```
crates/
  mycel-core/     networking core: identity, invites, protocol, libp2p node
  mycel-cli/      `mycel` command-line client (Phase 0)
apps/
  desktop/        Tauri desktop app (Phase 1, not started)
scripts/
  netsim/         simulated internet with real NAT behaviour for testing
docs/             architecture, roadmap, test reports
```

## Testing

```bash
cargo test                               # unit tests + nodes talking on this machine
sudo scripts/netsim/netsim.sh all        # Linux only: NAT scenarios across a fake internet
```

The network simulator builds home routers out of Linux network namespaces
(cone NAT, symmetric/CGNAT-style NAT, port forwarding) and checks which path
each connection takes. Latest results: [docs/phase-0-report.md](docs/phase-0-report.md).

## License

[MIT](LICENSE)
