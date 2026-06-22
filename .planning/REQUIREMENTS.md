# Requirements

## Functional

1. **System-wide traffic capture via TUN device**
   - Create/manage tun0 interface
   - Set default route through TUN
   - Read/write raw IP packets

2. **Proxy protocol support**
   - HTTP CONNECT
   - SOCKS4
   - SOCKS5

3. **Proxy discovery (scraping)**
   - Built-in sources: free-proxy-list.net, sslproxies.org, etc.
   - Periodic refresh
   - User-customizable sources

4. **Health checking**
   - Latency measurement
   - Anonymity level detection
   - Protocol support probing
   - Automatic score/ranking

5. **Proxy switching**
   - Manual: user picks exact proxy
   - Auto-rotate: timer-based or on failure
   - Instant effect (next packet)

6. **Control interfaces**
   - Web UI dashboard (SPA)
   - CLI with commands: switch, list, status, rotate, add, sources
   - REST API for both

7. **DNS leak prevention**
   - DNS resolution through selected proxy
   - Optional IPv6 disable

## Non-functional

- No dependency on X11/Wayland
- Single binary deployment (daemon + embedded UI)
- Graceful error handling on proxy failure
- Minimal latency overhead
