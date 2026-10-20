# Progress: Phase 1 (Terrain, imagery and streaming), v0.1.0

**Status:** in progress · started 2026-10-20 · previous phase: [Phase 0](progress/progress_phase_0.md) (released [v0.0.0](https://github.com/goglesquirmintontheiii/us-explorer/releases/tag/v0.0.0))

Goal: real ground shape and real imagery everywhere in the US, loading gracefully on slow hardware or connections.

Open GitHub issues checked at the start of this phase: none.

## Tasks

- [x] Elevation tiles: USGS 3DEP lidar (1 m class) from zoom 16 up, 10 m terrain tiles below, sea level fallback offline
- [x] Full coverage: lower 48, Alaska, Hawaii and Puerto Rico (Web Mercator quadtree)
- [x] Rock on steep slopes so cliffs don't show stretched imagery
- [x] Esri World Imagery with a USGS/NAIP switch in Settings
- [x] Level of detail by on-screen texel size, full detail around you, coarse to the horizon
- [x] Skirts on every tile edge to hide cracks between zoom levels
- [x] Ground detail near the camera (grass, asphalt, dirt, rock) with small bumps
- [x] Collision matches the drawn triangles exactly
- [x] Lazy loading: nearest tiles first, parents stay until children are ready, never blocks a frame
- [x] Adaptive streaming: detail backs off when downloads or frames fall behind
- [x] Prefetch along the direction of travel; optional slow-down while loading
- [x] Loading indicator
- [x] Arrival timing in the overlay; visited areas load from the disk cache with no network
- [x] Shadows baked into imagery lifted slightly
- [x] Offline area download (Esc → Offline download)
- [x] BC1-compressed imagery textures (about 8x less video memory) when the GPU supports it
- [x] Teleport holds you above the ground until detailed terrain arrives
- [ ] Verify street-level and canyon quality in fresh screenshots
- [ ] Release v0.1.0

Deferred on purpose:
- Real lake and coastal depths are already in the data but stay hidden until water exists (Phase 5); today the ground is clamped at sea level.
- Snow on high peaks follows the season and weather, so it belongs to Phase 7.

## Screenshots

_Coming from the next CI run._
