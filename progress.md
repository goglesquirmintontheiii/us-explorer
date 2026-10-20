# Progress: Phase 3.1 (Hotfix: roads, lights and water), v0.3.1-hotfix

**Status:** in progress · previous phase: [Phase 3](progress/progress_phase_3.md) (released [v0.3.0](https://github.com/goglesquirmintontheiii/us-explorer/releases/tag/v0.3.0))

Goal: fix what testing of 0.3.0 turned up and add the requested road, sign, light and water features. Linux AppImage only for this release.

Open GitHub issues checked at the start of this phase: none.

## Tasks

### Loading
- [x] Road data in ~1 km tiles (zoom 15) instead of ~2 km, nearest first
- [x] Overpass servers raced against each other: the next one starts if the first hasn't answered in 8 s, and the first good answer wins
- [x] Falls back to the main OpenStreetMap API when every Overpass server fails
- [x] Water areas, freeway exits, mile markers, level crossings and power lines added to the map query (cache format v2)

### Roads
- [x] Roads are built from denser rows and several columns across, and the levelled corridor under them includes the sidewalks, so the terrain no longer pokes through
- [x] Curbs on city streets wherever there are sidewalks, also where sidewalks are mapped as separate ways (and `kerb=*`)
- [x] Smooth width tapers where a road gains or loses lanes
- [x] Junctions joined by very short links (divided roads crossing) merge into one intersection: one paved surface, one signal controller, phases by direction
- [x] Sidewalks sit level with the road edge on slopes; inferred only on town streets (not fast, rural or unpaved roads)
- [x] Raised roads in cities stand on retaining walls with at most a metre of fill, instead of raising the ground; cuts and underpasses unchanged
- [x] Bridge parapets open where ramps merge; guardrails stop near side roads and ramps
- [x] Construction-zone roads look like the road they connect to; cones and barrels stand on the road surface

### Markings and lights
- [x] Turn arrows drawn in metres at real proportions (5 m long)
- [x] Signal lamps show red, amber and green instead of white
- [x] Pedestrian signals with the raised hand and walking person symbols
- [x] Pools of light under inferred streetlights, visible at any distance; 24 real lights near you instead of 10, with a longer reach
- [x] Darker nights (much less ambient light); bloom around bright lights

### Bridges, signs and objects
- [x] Bridge types from `bridge:structure`: girder bridges on pier bents with girders under the deck, truss, through arch, suspension (towers, main cables, suspenders) and cable-stayed (pylons and stays); painted with the `colour` tag
- [x] Overhead guide signs about 250 m before freeway exits, on a gantry or on the side of an overpass when one crosses there
- [x] Mile markers: from mapped `highway=milestone`, or worked out from exit numbers where they run at a mile per mile
- [x] Street name sign poles end under the blades
- [x] Flock-style ALPR cameras (slim pole, solar panel, battery box, camera on an arm)
- [x] Utility poles with sagging wires along streets (and mapped power lines), fire hydrants at the curb, where the map has none
- [x] Streetlights, bus stops and other mapped objects face the street; pedestrian signals face the people crossing
- [x] Level crossings: rails flush with the road in concrete panels, crossbuck signs (with lights where tagged)

### Water
- [x] Lakes, reservoirs and river areas from OSM drawn as water with moving ripples, and you can swim in them

### Release
- [ ] Screenshots reviewed
- [ ] v0.3.1-hotfix released (AppImage)

## Screenshots

To come from the CI screenshot run.
