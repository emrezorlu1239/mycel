# Phase 0 report

**Question:** can two computers in different homes, with no server in between,
find each other using only an invite code and chat over an encrypted connection?

**Answer:** yes. When routers allow it, the connection becomes direct. When
they do not (mobile networks, CGNAT), chat still works through a relay that
only ever sees encrypted bytes.

Test date: 2026-10-21 · rust-libp2p 0.57 · Rust 1.97 · Linux test machine.

## 1. Automated tests (`cargo test`)

24 tests, all passing.

| Area | Tests |
|------|-------|
| Identity | survives restart; separate folders give separate identities; concurrent first starts agree on one key; corrupted key file reported |
| Invites | round trip; copy-paste whitespace tolerated; garbage, truncated, future-version and empty codes rejected |
| Protocol | stable JSON shape; nickname sanitising |
| Node helpers | address classification; relay extraction prefers QUIC |
| End to end (real nodes on one machine) | host ↔ member chat both ways; fan-out to every other member without echo; messages typed before joining are delivered; member reaches host **only** through a relay; member reconnects when the host restarts; member **refuses an impostor** at the right address with the wrong key |

## 2. Network simulator (`scripts/netsim/netsim.sh`)

A fake internet built from Linux network namespaces, with routers doing real
NAT (iptables). "Cone" routers behave like typical home routers; "symmetric"
routers pick a new public port per destination like mobile carriers and CGNAT.

| # | Scenario | Expected | Result |
|---|----------|----------|--------|
| 1 | Same home network | direct | ✅ direct |
| 2 | Two homes, no relay, no port forwarding | cannot connect | ✅ cannot connect |
| 3 | Host forwarded a port | direct | ✅ direct |
| 4 | Two homes, ordinary routers, with relay | relayed → hole punch → direct, relay released | ✅ punched |
| 5 | Host at home, member on mobile/CGNAT | relayed | ✅ relayed |
| 6 | Both on mobile/CGNAT | relayed | ✅ relayed |
| 7 | Two homes whose routers lose the punch timing race | relayed | ✅ relayed |

The full suite ran **three times in a row: 21/21 passes.**

Evidence from scenario 4 (member's log, IDs shortened):

```
[path] RELAYED peer=<host> addr=/ip4/203.0.113.10/udp/4001/quic-v1/p2p/<relay>/p2p-circuit/p2p/<host>
[joined] "4-two-homes-holepunch" members: host
[path] DIRECT peer=<host> addr=/ip4/203.0.113.21/udp/33623/quic-v1
[holepunch] OK peer=<host> upgraded to a direct connection
[msg] host: hello from the host
```

Scenario 5 (CGNAT): `[path] RELAYED` → `[joined]` → messages both ways, no
direct path, as expected.

## 3. Problems found and fixed during testing

| Problem | Fix |
|---------|-----|
| Relay refused reservations: it did not consider its own address reachable | Relay treats its listen addresses as external unless told otherwise |
| Hole punching never tried QUIC: invites listed TCP first and members only learned their public TCP port | Invites order QUIC first; members connect to the invite's relays over QUIC before dialling the host |
| After a successful punch, half the messages still went through the relay (round-robin over connections) | Relayed connection is closed 2 s after a successful punch |
| Invite printed several times during start-up, early ones incomplete | Invite published once the address list settles (1.5 s) |
| Two processes starting with a fresh data folder could create two different identities | Atomic create; the loser reads the winner's key |
| Failed extra dials (e.g. a lost punch race) printed scary warnings while chat worked | Only reported when the host is actually unreachable |

## 4. What a zero-latency simulator gets wrong, and how we handled it

In a simulator, packets arrive instantly. A hole-punch packet from the other
side can reach the router **before** our own outgoing packet. Plain Linux NAT
then creates a mapping that forces a different public port, and the punch
fails every time. On a real network the other side's packet arrives
milliseconds later, after our own has opened the hole. The simulator's
`cone` mode reproduces this by dropping unsolicited packets before they create
NAT state, and scenario 7 keeps the unfavourable behaviour on purpose to prove
the relay fallback.

## 5. Not yet verified

- **Real routers and ISPs.** Follow [testing-with-friends.md](testing-with-friends.md)
  and fill in the table below.
- **Windows and macOS builds.** The code has no platform-specific networking,
  and CI builds and tests on all three systems, but the first CI run has not
  happened yet.

### Real-world results

| Date | Host network | Member network | Path | Notes |
|------|--------------|----------------|------|-------|
|      |              |                |      |       |
