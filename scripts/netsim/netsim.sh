#!/usr/bin/env bash
# Mycel network simulator.
#
# Builds a fake internet out of Linux network namespaces, with home routers
# that do NAT exactly like real ones, then runs Mycel scenarios across it.
#
#   sudo scripts/netsim/netsim.sh up          # build the topology
#   sudo scripts/netsim/netsim.sh test        # run every scenario
#   sudo scripts/netsim/netsim.sh down        # remove everything
#   sudo scripts/netsim/netsim.sh all         # up + test + down
#
# Requires Linux, root, iproute2, iptables, ipset and conntrack. Uses target/release/mycel
# unless MYCEL_BIN points elsewhere.
#
# Topology ("internet" is 203.0.113.0/24, a range reserved for examples):
#
#                          +----------------- internet ------------------+
#                          |                                              |
#   relay 203.0.113.10     rA .21            rB .22           rC .23          rD .24
#                          |  cone NAT       | cone NAT       | symmetric NAT | symmetric NAT
#                          |  (see nat_mode for what each NAT type does)
#                       192.168.1.0/24    192.168.2.0/24   192.168.3.0/24  192.168.4.0/24
#                        hA .10, hA2 .11    hB .10           hC .10          hD .10
#
# "Cone" routers keep the same public port for a socket (most home routers).
# "Symmetric" routers pick a random public port per destination (typical of
# mobile carriers and CGNAT), which defeats hole punching.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${MYCEL_BIN:-$ROOT/target/release/mycel}"
WORK="${NETSIM_WORK:-/tmp/mycel-netsim}"
NSES=(inet relay rA hA hA2 rB hB rC hC rD hD)

ns() { ip netns exec "$@"; }

up() {
    down >/dev/null 2>&1 || true
    for n in "${NSES[@]}"; do
        ip netns add "$n"
        ns "$n" ip link set lo up
    done

    # The internet: one bridge everyone's WAN side plugs into.
    ns inet ip link add br0 type bridge
    ns inet ip link set br0 up

    attach_wan relay 203.0.113.10
    for r in A B C D; do
        local idx=$(( $(printf '%d' "'$r") - 64 ))      # A=1, B=2 ...
        attach_wan "r$r" "203.0.113.$((20 + idx))"
        make_lan "r$r" "$idx"
    done
    attach_lan rA hA 192.168.1.10 1
    attach_lan rA hA2 192.168.1.11 1
    attach_lan rB hB 192.168.2.10 2
    attach_lan rC hC 192.168.3.10 3
    attach_lan rD hD 192.168.4.10 4

    nat_mode rA cone
    nat_mode rB cone
    nat_mode rC symmetric
    nat_mode rD symmetric
    echo "topology up"
}

attach_wan() { # <ns> <ip>
    local n=$1 ip=$2
    ip link add "w-$n" type veth peer name "p-$n"
    ip link set "w-$n" netns "$n"
    ip link set "p-$n" netns inet
    ns inet ip link set "p-$n" master br0 up
    ns "$n" ip link set "w-$n" name wan
    ns "$n" ip addr add "$ip/24" dev wan
    ns "$n" ip link set wan up
}

make_lan() { # <router-ns> <index>
    local r=$1 i=$2
    ns "$r" ip link add brlan type bridge
    ns "$r" ip addr add "192.168.$i.1/24" dev brlan
    ns "$r" ip link set brlan up
    ns "$r" sysctl -qw net.ipv4.ip_forward=1
}

attach_lan() { # <router-ns> <host-ns> <ip> <index>
    local r=$1 h=$2 ip=$3 i=$4
    ip link add "l-$h" type veth peer name "q-$h"
    ip link set "l-$h" netns "$h"
    ip link set "q-$h" netns "$r"
    ns "$r" ip link set "q-$h" master brlan up
    ns "$h" ip link set "l-$h" name eth0
    ns "$h" ip addr add "$ip/24" dev eth0
    ns "$h" ip link set eth0 up
    ns "$h" ip route add default via "192.168.$i.1"
}

# NAT modes for a router namespace.
#
#   cone       Keeps a socket's public port (like most home routers) and only
#              lets in packets from addresses we have already sent to
#              ("port-restricted cone"). Unsolicited packets are dropped before
#              they create NAT state, which is what happens on a real network
#              where the other side's packets arrive a few milliseconds later.
#   naive      Plain Linux MASQUERADE with zero latency. A hole-punch packet
#              that arrives before our own outgoing one creates NAT state that
#              forces a new public port, so punching loses the race.
#   symmetric  Random public port per destination (mobile carriers, CGNAT).
nat_mode() { # <router-ns> <mode>
    local r=$1 mode=$2
    for t in raw mangle nat filter; do ns "$r" iptables -t "$t" -F; done
    ns "$r" ipset destroy seen 2>/dev/null || true
    case "$mode" in
        cone)
            ns "$r" ipset create seen hash:ip,port timeout 120
            ns "$r" iptables -t mangle -A POSTROUTING -o wan -p udp \
                -j SET --add-set seen dst,dst --exist
            ns "$r" iptables -t raw -A PREROUTING -i wan -p udp \
                -m set ! --match-set seen src,src -j DROP
            ns "$r" iptables -t nat -A POSTROUTING -o wan -j MASQUERADE
            ;;
        naive)
            ns "$r" iptables -t nat -A POSTROUTING -o wan -j MASQUERADE
            ;;
        symmetric)
            ns "$r" iptables -t nat -A POSTROUTING -o wan -j MASQUERADE --random-fully
            ;;
    esac
    # Like any home router: nothing gets in unless it answers something we sent.
    ns "$r" iptables -A FORWARD -i wan -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    ns "$r" iptables -A FORWARD -i wan -j DROP
    ns "$r" conntrack -F >/dev/null 2>&1 || true
}

port_forward() { # <router-ns> <lan-ip> <port>
    local r=$1 ip=$2 p=$3
    ns "$r" iptables -t raw -I PREROUTING -i wan -p udp --dport "$p" -j ACCEPT
    for proto in udp tcp; do
        ns "$r" iptables -t nat -A PREROUTING -i wan -p "$proto" --dport "$p" -j DNAT --to-destination "$ip:$p"
        ns "$r" iptables -I FORWARD -i wan -p "$proto" -d "$ip" --dport "$p" -j ACCEPT
    done
}

down() {
    for n in "${NSES[@]}"; do
        # Stop anything still running inside before removing the namespace.
        ip netns pids "$n" 2>/dev/null | xargs -r kill 2>/dev/null || true
        ip netns del "$n" 2>/dev/null || true
    done
    echo "topology down"
}

# ----- scenario helpers -----------------------------------------------------

PASS=0
FAIL=0
RESULTS=()

start_relay() {
    rm -rf "$WORK/relay"
    mkdir -p "$WORK/relay"
    ip netns pids relay 2>/dev/null | xargs -r kill 2>/dev/null || true
    # Create the identity first so we know the relay's id before it starts.
    RELAY_ID=$(ns relay "$BIN" id --data-dir "$WORK/relay/data" | awk 'NR==1{print $2}')
    RELAY_ADDR="/ip4/203.0.113.10/udp/4001/quic-v1/p2p/$RELAY_ID"
    # `ip netns exec` directly (not through a function) so $! is the relay itself.
    ip netns exec relay "$BIN" relay --data-dir "$WORK/relay/data" --port 4001 \
        --external /ip4/203.0.113.10/udp/4001/quic-v1 \
        --external /ip4/203.0.113.10/tcp/4001 \
        --exit-after 600 >"$WORK/relay/log" 2>&1 &
    RELAY_PID=$!
    sleep 1
}

# run_scenario <name> <host-ns> <member-ns> <expect> [host extra args...]
#   expect: direct | punched | relayed | unreachable
run_scenario() {
    local name=$1 hns=$2 mns=$3 expect=$4
    shift 4
    local dir="$WORK/$name"
    rm -rf "$dir"
    mkdir -p "$dir"

    ( sleep 14; echo "hello from the host" ) | ns "$hns" "$BIN" host \
        --server "$name" --nick host --data-dir "$dir/host" --no-mdns \
        --invite-file "$dir/invite" --exit-after 30 "$@" >"$dir/host.log" 2>&1 &
    local hpid=$!

    # Wait for an invite. With a relay, wait until it includes the circuit.
    local want='invite-addr'
    [[ " $* " == *" --relay "* ]] && want='invite-addr.*p2p-circuit'
    for _ in $(seq 1 60); do
        grep -q "$want" "$dir/host.log" 2>/dev/null && [ -s "$dir/invite" ] && break
        sleep 0.25
    done
    sleep 0.5

    ( sleep 10; echo "hello from the member" ) | ns "$mns" "$BIN" join "@$dir/invite" \
        --nick member --data-dir "$dir/member" --no-mdns --exit-after 22 >"$dir/member.log" 2>&1 || true
    wait "$hpid" || true

    local got
    got=$(classify "$dir")
    local ok=fail
    [ "$got" = "$expect" ] && ok=pass
    if [ "$ok" = pass ]; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); fi
    RESULTS+=("$(printf '%-28s expected=%-11s got=%-11s %s' "$name" "$expect" "$got" "${ok^^}")")
    printf '%-28s expected=%-11s got=%-11s %s\n' "$name" "$expect" "$got" "${ok^^}"
}

# Works out what actually happened from the logs.
classify() {
    local dir=$1 m="$1/member.log" h="$1/host.log"
    local both_ways=no
    grep -q '^\[msg\] member: hello from the member' "$h" &&
        grep -q '^\[msg\] host: hello from the host' "$m" && both_ways=yes
    if [ "$both_ways" != yes ]; then
        grep -q '^\[joined\]' "$m" && echo "partial" || echo "unreachable"
        return
    fi
    # Only connections to the host count, not the member's link to the relay.
    local host_id paths
    host_id=$(awk '/^\[id\]/{print $2; exit}' "$h")
    paths=$(grep "^\[path\] .* peer=$host_id " "$m" | awk '{print $2}' | sort -u | tr '\n' ' ')
    if grep -q '^\[holepunch\] OK' "$h" "$m"; then
        # A punch only counts if the relayed connection was then let go.
        if grep -q '^\[path\] relay released' "$h" "$m"; then echo "punched"; else echo "punched-kept-relay"; fi
    elif [ "$paths" = "RELAYED " ]; then
        echo "relayed"
    elif [ "$paths" = "DIRECT " ]; then
        echo "direct"
    else
        echo "mixed"
    fi
}

test_all() {
    [ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 1; }
    mkdir -p "$WORK"
    start_relay

    # 1. Same home network: straight connection, no help needed.
    run_scenario 1-same-lan hA hA2 direct

    # 2. Different homes, no relay, no port forwarding: impossible by design
    #    of NAT. Proves why relays exist.
    run_scenario 2-no-relay-blocked hA hB unreachable

    # 3. Host forwarded a port on its router: friends connect directly.
    port_forward rA 192.168.1.10 4001
    run_scenario 3-port-forwarded hA hB direct --port 4001 \
        --external /ip4/203.0.113.21/udp/4001/quic-v1 --external /ip4/203.0.113.21/tcp/4001
    nat_mode rA cone

    # 4. Different homes behind ordinary routers: relay first, then the hole
    #    punch upgrades to a direct connection.
    run_scenario 4-two-homes-holepunch hA hB punched --relay "$RELAY_ADDR"

    # 5. Member on a mobile/CGNAT-style network: hole punching cannot work,
    #    chat continues through the relay.
    run_scenario 5-member-symmetric-nat hA hC relayed --relay "$RELAY_ADDR"

    # 6. Both sides on symmetric NAT: relay only.
    run_scenario 6-both-symmetric-nat hD hC relayed --relay "$RELAY_ADDR"

    # 7. Two homes behind routers that lose the timing race (see nat_mode):
    #    the hole punch fails and the relay must carry the chat.
    nat_mode rA naive
    nat_mode rB naive
    run_scenario 7-two-homes-race-lost hA hB relayed --relay "$RELAY_ADDR"
    nat_mode rA cone
    nat_mode rB cone

    kill "$RELAY_PID" 2>/dev/null || true

    echo
    echo "summary: $PASS passed, $FAIL failed"
    {
        echo "# netsim results ($(date -u +%Y-%m-%dT%H:%MZ))"
        printf '%s\n' "${RESULTS[@]}"
        echo "summary: $PASS passed, $FAIL failed"
    } >"$WORK/summary.txt"
    [ "$FAIL" -eq 0 ]
}

case "${1:-all}" in
    up) up ;;
    down) down ;;
    test) test_all ;;
    all)
        up
        trap down EXIT
        test_all
        ;;
    *) echo "usage: $0 {up|test|down|all}"; exit 2 ;;
esac
