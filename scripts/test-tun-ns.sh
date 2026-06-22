#!/usr/bin/env bash
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
source "$HOME/.cargo/env" 2>/dev/null || true

NS="proxy-test-ns"
VETH_HOST="veth-proxy-h"
VETH_NS="veth-proxy-ns"
NS_SUBNET="10.200.0"
NS_IP="${NS_SUBNET}.2"
HOST_IP="${NS_SUBNET}.1"

cleanup() {
    echo "=== CLEANUP ==="
    sudo ip netns del "$NS" 2>/dev/null || true
    sudo iptables -D FORWARD -i "$VETH_HOST" -j ACCEPT 2>/dev/null || true
    sudo iptables -D FORWARD -o "$VETH_HOST" -j ACCEPT 2>/dev/null || true
    sudo iptables -t nat -D POSTROUTING -s "${NS_SUBNET}.0/24" -j MASQUERADE 2>/dev/null || true
    echo "Cleanup done"
}
trap cleanup EXIT

echo "=== Step 1: Create namespace ==="
sudo ip netns add "$NS"

echo "=== Step 2: Create veth pair for internet access ==="
sudo ip link add "$VETH_HOST" type veth peer name "$VETH_NS"
sudo ip link set "$VETH_NS" netns "$NS"

echo "=== Step 3: Assign IPs and bring up ==="
sudo ip addr add "${HOST_IP}/24" dev "$VETH_HOST"
sudo ip link set "$VETH_HOST" up

sudo ip netns exec "$NS" ip addr add "${NS_IP}/24" dev "$VETH_NS"
sudo ip netns exec "$NS" ip link set "$VETH_NS" up
sudo ip netns exec "$NS" ip link set lo up

echo "=== Step 4: Set default route in namespace ==="
sudo ip netns exec "$NS" ip route add default via "$HOST_IP"

echo "=== Step 5: Enable IP forwarding + NAT ==="
sudo bash -c "echo 1 > /proc/sys/net/ipv4/ip_forward"
sudo iptables -A FORWARD -i "$VETH_HOST" -j ACCEPT
sudo iptables -A FORWARD -o "$VETH_HOST" -j ACCEPT
sudo iptables -t nat -A POSTROUTING -s "${NS_SUBNET}.0/24" -j MASQUERADE

echo "=== Step 6: Verify internet access ==="
sudo ip netns exec "$NS" ping -c1 -W3 8.8.8.8 >/dev/null 2>&1 && echo "Internet OK" || echo "WARNING: No internet (may affect tests)"

echo "=== Step 7: Build TUN test binary ==="
cargo build -p proxy-tun-test 2>&1
BINARY="$(pwd)/target/debug/proxy-tun-test"

echo "=== Step 8: Run TUN test in namespace ==="
sudo ip netns exec "$NS" env \
    PATH="$HOME/.cargo/bin:$PATH" \
    HOME="$HOME" \
    RUST_LOG=debug \
    "$BINARY" 2>&1

echo "=== Test complete ==="
