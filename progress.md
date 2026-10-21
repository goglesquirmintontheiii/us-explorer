# Progress: Phase 5 (Nature, water and the sky), v0.5.0

**Status:** complete · released as [v0.5.0](https://github.com/goglesquirmintontheiii/us-explorer/releases/tag/v0.5.0) on 2026-10-21 · previous phase: [Phase 4](progress/progress_phase_4.md) (released [v0.4.0](https://github.com/goglesquirmintontheiii/us-explorer/releases/tag/v0.4.0))

Goal: the land between the roads and buildings: trees, grass and ground cover that match the region and the season, lakes, rivers and the sea with real waves, beaches, parks, fences, power lines, wind turbines and buoys; and the sky: the real sun and moon, stars, planets, the Milky Way and the aurora, with night lighting and ambient sound. Also the fixes and requests from testing 0.4.0. Linux AppImage and Windows build.

Open GitHub issues checked at the start of this phase: none.

## Tasks

### 0.4.0 fixes and requests
- [x] Crash a few minutes after entering a house at 100% GPU: the bathroom mirror camera now renders at a third of the resolution with a 25 m far plane, interiors don't cast sun shadows, and a crash log is written (shown on the title screen next time)
- [x] Freeze loading a large building's interior: tangents are computed with a fast per-triangle method on the worker thread (MikkTSpace on the main thread took seconds)
- [x] Stores look different: 19 store styles (grocery, convenience, clothing, department, sports, shoes, electronics, books, hardware, furniture, pharmacy, salon, jewelry, bakery, liquor, florist, pet, toys, general) with their own fixtures, goods, floors and paint
- [x] Title screen always shown and lightweight: the world isn't drawn behind it and it runs at 30 fps
- [x] Parking lots: built in small pieces per tile so no corner is lifted, laid flat (the ground is smoothed under them), stripes no longer sink into the surface, wheel stops and accessible-parking signs appear
- [x] Downtown streets lifted 5-12 m (San Antonio over the River Walk): footpaths and underground ways no longer push the streets above them up, and a crossing only lifts the deck near it
- [x] Rivers drawn at street level: inland water sits on the lowest natural ground around it
- [x] Divided roads no longer get sidewalks and curbs in their median overlapping the other carriageway
- [x] Pedestrian signals face the people waiting to cross; bus stop signs face approaching traffic
- [x] Night is much darker (moonlight and street lights carry it)

### Vegetation
- [x] 51 tree and plant species with regional mixes in 20 biomes (palms in South Florida and on warm beaches, live oaks and cedar in the Hill Country, conifers in the West and North, saguaros and Joshua trees in the deserts, and so on), placed by setting (street, woods, riverside, coast)
- [x] Procedural trees at four growth stages with real branching, 3D leaf cards, bark; impostor cards far away; 2D cards for every tree below the default preset
- [x] Land cover from the aerial imagery and OpenStreetMap: canopy, grass, dry grass, shrubs, crops in rows, wetlands, sand, rock, dunes
- [x] 3D grass blades from the default preset up (tufts below), coloured from the imagery and the season
- [x] Seasons from NASA MODIS satellite greenness (leaf-out, autumn colour, bare winter trees, dry summers) with a phenology model offline

### Water
- [x] Ocean with swell and wind waves from the Open-Meteo marine model and the nearest NOAA buoy, shoaling, whitecaps and surf on the beach, wet sand and swash
- [x] Lakes ripple with the real wind; rivers flow along their mapped centrelines; streams and small rivers drawn as water ribbons
- [x] Beaches: wet and dry sand, foam, dune grass, piers, lifeguard towers (mapped or set along warm beaches)
- [x] Buoys and beacons from OpenStreetMap sea marks, in IALA-B colours and shapes, bobbing on the waves, with flashing lights by their mapped character

### Details
- [x] Fences (chain-link, wood, metal), walls, hedges, retaining walls; power lines with lattice, steel and wood towers; wind turbines turning into the real wind at a speed that follows it; park furniture
- [x] Parked cars in lots (head-in, some backed in, fuller near the buildings) and along residential curbs (except where parking is mapped as banned), in five body styles and the colours US cars come in

### Sky and light
- [x] The real sun and moon (low-precision ephemerides, parallax included), the moon's phase lit by the sun, moonlight
- [x] 7,000 stars from the Yale Bright Star Catalogue and the five naked-eye planets, twinkling; the Milky Way in its real place
- [x] Light pollution from NASA's VIIRS Black Marble washes out the faint stars and the Milky Way and lights the sky over towns; cloud cover hides them
- [x] Aurora from NOAA's OVATION forecast and Kp index, towards the magnetic pole
- [x] Local time with time zones and US daylight saving in the overlay
- [x] Ambient sound mixed by where you are: city, highway, wind in the trees, birds by day, crickets on warm nights, surf, wind higher up (synthesised, no sound files); volume in Settings

### Release
- [x] Settings: nature, details, sound and volume; F3 shows nature, conditions, moon and light pollution; credits for the new sources; offline download includes the nature layer
- [x] v0.5.0 released (AppImage and Windows .exe); screenshots skipped as asked, CI and local runs used for checking

## Bug sweep

Found and fixed while checking the release (CI picture runs of San Francisco, Miami, San Antonio, a wind farm, a forest, a farm and a river, plus local runs):

- A new land-cover class overflowed a tally and crashed the vegetation worker near beaches
- Trees grew in shallow turquoise sea water (it read as green): the sea is now taken from the elevation model's bathymetry, and clearly blue-green water from its colour
- Surf foam covered wide shallow flats; breakers now form only where the bottom shelves up to a shore
- Far ocean waves aliased into rings; waves shorter than a few pixels fade out
- Waterways mapped across the sea drew as ribbons over it; stream ribbons stop at the sea, and rivers and canals mapped as water areas aren't drawn twice
- Dune thickets were planted with palms; they're low scrub now. Shrubs were a few sticks with floating leaves; they're dense bushes
- Palm fronds read as solid paddles; leaflets are separate now
- A two-node bridge over a street sagged to half clearance (the deck is straight between its nodes); both ends lift until the line clears
- The planets' mesh was rebuilt in a way the renderer rejected (it vanished)
