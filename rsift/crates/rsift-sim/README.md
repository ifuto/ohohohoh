# rsift-sim

Native simulation helpers (lighting planners, pathfinding, QUIC hints, region I/O).

## Wiring status

- **Used from:** `rsift-jvm` (`init_global_sim` / `tick_global_sim` via agent bridge).
- **Not used from:** `mods-official/*` crates (by design — sim is owned by the JVM agent layer).

This crate is a **library**, not a standalone official mod. Features that are not called from `rsift-jvm` remain available for tests/benches but are not claimed as live Minecraft tick replacements until explicitly hooked.
