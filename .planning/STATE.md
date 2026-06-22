# Project State

## Active Phase: 1
## Status: Design Complete
## Current Task: Project scaffolding

## Decisions
- Workspace layout with 3 crates + frontend
- React for Web UI (ecosystem, community)
- Axum over Actix (simpler, modern tokio-native)
- SQLite for persistence (no daemon dependency)
- TUN device, not TAP (L3, no ARP needed)
- Default route injection via ip route replace
