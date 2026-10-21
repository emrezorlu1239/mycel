# Roadmap

Each phase ends with something people can try. Phases are checked off only
when their tests pass.

## Phase 0 – Proof of concept ✅

Can two computers in different homes, with no server in between, find each
other with an invite code and chat over an encrypted connection?

- [x] Persistent device identity (Ed25519)
- [x] Invite codes (`mycel1…`), validated and tamper-evident
- [x] QUIC + TCP transports, always encrypted
- [x] LAN discovery (mDNS)
- [x] Relay mode, relay reservations, relayed connections
- [x] Hole punching (DCUtR), with the relay released afterwards
- [x] Host-relayed chat with join, fan-out and offline queueing
- [x] Automatic reconnect when the host comes back
- [x] `mycel` CLI with `host`, `join`, `relay`, `id`
- [x] 24 unit + integration tests, NAT simulator with 7 scenarios (3 × 7/7 passing)
- [x] CI green on Linux, Windows and macOS
- [ ] Real-world test across two homes (see [testing-with-friends.md](testing-with-friends.md))

## Phase 1 – A usable desktop app

- Tauri desktop app (Windows first, then macOS/Linux), English UI with Turkish
- Servers with text channels, message history saved on the host
- Admin: kick, ban, invite revocation, **server transfer** to another member
- Data folder per user: keep or bulk-delete messages, images, voice notes
- Automatic relay discovery: Kademlia DHT bootstrap; reachable members act as
  relays for their group
- Identity backup/restore
- Installer and auto-update from GitHub Releases, plus mirror download links

## Phase 2 – Rich messaging

- File and image sharing (chunked, resumable)
- Voice messages
- Reactions, replies, mentions, unread markers
- Member list with presence

## Phase 3 – Voice

- Voice channels over WebRTC media through the existing P2P connections
- Push-to-talk, mute/deafen, per-user volume
- Live audio is never recorded

## Phase 4 – Everywhere

- Screen sharing and video
- Mobile apps (Android first)
- Pluggable transports for networks that block QUIC/UDP or fingerprint traffic
- Optional local AI bot in channels, running on the host's machine
