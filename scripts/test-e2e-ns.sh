#!/usr/bin/env bash
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

NS="proxy-e2e-ns"
VETH_H="veth-e2e-h"
VETH_N="veth-e2e-n"
SUBNET="10.200.0"
NS_IP="${SUBNET}.2"
HOST_IP="${SUBNET}.1"

cleanup() {
    echo "=== CLEANUP ==="
    sudo ip netns del "$NS" 2>/dev/null || true
    sudo iptables -D FORWARD -i "$VETH_H" -j ACCEPT 2>/dev/null || true
    sudo iptables -D FORWARD -o "$VETH_H" -j ACCEPT 2>/dev/null || true
    sudo iptables -t nat -D POSTROUTING -s "${SUBNET}.0/24" -j MASQUERADE 2>/dev/null || true
    echo "Done"
}
trap cleanup EXIT

echo "=== 1. Namespace ==="
sudo ip netns add "$NS"

echo "=== 2. Veth pair ==="
sudo ip link add "$VETH_H" type veth peer name "$VETH_N"
sudo ip link set "$VETH_N" netns "$NS"

echo "=== 3. IPs ==="
sudo ip addr add "${HOST_IP}/24" dev "$VETH_H"
sudo ip link set "$VETH_H" up
sudo ip netns exec "$NS" ip addr add "${NS_IP}/24" dev "$VETH_N"
sudo ip netns exec "$NS" ip link set "$VETH_N" up
sudo ip netns exec "$NS" ip link set lo up

echo "=== 4. Route + NAT ==="
sudo ip netns exec "$NS" ip route add default via "$HOST_IP"
sudo bash -c "echo 1 > /proc/sys/net/ipv4/ip_forward"
sudo iptables -A FORWARD -i "$VETH_H" -j ACCEPT
sudo iptables -A FORWARD -o "$VETH_H" -j ACCEPT
sudo iptables -t nat -A POSTROUTING -s "${SUBNET}.0/24" -j MASQUERADE

echo "=== 5. Establish ARP (ping gateway) ==="
sudo ip netns exec "$NS" ping -c1 -W2 10.200.0.1 >/dev/null 2>&1 || true

echo "=== 6. Verify internet ==="
sudo ip netns exec "$NS" ping -c1 -W5 8.8.8.8 >/dev/null 2>&1 && echo "OK" || { echo "No internet"; exit 1; }

echo "=== 7. Build ==="
cargo build -p proxy-tun-test 2>&1

echo "=== 8. E2E test ==="
BINARY="$(pwd)/target/debug/proxy-tun-test"
sudo ip netns exec "$NS" env \
    PATH="$HOME/.cargo/bin:$PATH" \
    HOME="$HOME" \
    RUST_LOG=proxy=debug \
    "$BINARY" 2>&1 || true

echo "=== DONE ==="
