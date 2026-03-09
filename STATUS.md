# Anno 1602 RE — Project Status

## Reverse Engineering

| Subsystem | Functions | Coverage |
|-----------|-----------|----------|
| Graphics engine | ~50 key functions mapped | Full architecture understood: DirectDraw/GDI dual backend, BSH sprite format, isometric renderer, palette system |
| Sound engine | 70/70 functions | Fully analyzed: wave slots, streaming, ADPCM codec, 3D positioning |
| Game simulation | 12 subsystem tickers identified | Core formulas extracted: production efficiency, population happiness, tax/bankruptcy, entity state machine |
| AI controller | Entry point identified | Not yet deeply analyzed |
| Networking (Maxnet.dll) | 77/77 functions | Fully decompiled: DirectPlay wrapper, 13 exported STDCALL functions, message protocol (12 command IDs), confirmed/unconfirmed send modes, 4-player session management, pause/run sync, message fragmentation |
| Save/load | Chunk format decoded | SZS chunk structure: 16-byte name + 4-byte size + data. INSEL5 (island metadata) + INSELHAUS (8-byte tile records) parsed. |

## Rust Implementation

| Crate | Status | What works |
|-------|--------|------------|
| `anno-formats` | All parsers verified | BSH: 5964 sprites. COL: 256-color palette. SZS: island chunks. COD: 500 building defs with sub-object properties (ProdKind, Ware, Rohstoff, Interval, Maxlager, construction costs). |
| `anno-audio` | Integrated into game | ADPCM codec tested (encode/decode roundtrip). AudioEngine with rodio backend: WaveManager (256 slots, spatial audio), StreamManager (8 slots, auto-detect finish). Plays 21 MUSIC8 tracks + SPEECH8 sound effects in game binary. |
| `anno-render` | SpriteManager tested | Framebuffer blitter, isometric camera, tile renderer. SpriteManager loads all 33 BSH files (47,679 sprites) across 3 zoom levels × 11 categories with case-insensitive path matching. |
| `anno-sim` | Full simulation engine | Data bridge, production ticker, A* carrier dispatch, warehouse delivery, population/economy model, AI controller (3 personalities), combat system (10 unit types, diplomacy), trade routes (multi-stop, cargo management), marketplace coverage (radius from COD, public buildings), ocean pathfinding (world map A*). 30 good types. 11 subsystem timers all active. |
| `anno-net` | Protocol implemented | Message protocol (12 command IDs: GameData, Pause, Resume, Ack, PlayerSync, SessionInfo, ChatMessage, FragContinuation, PlayerDisconnect). Session management (4 player slots, pause bitmask). TCP transport replacing DirectPlay (host/client, non-blocking I/O, message fragmentation). 8 tests passing. |
| `anno-game` | Four working binaries | `sprite-viewer`: browse sprites. `island-viewer`: isometric rendering with world map, 3 zoom levels, screenshots. `sim-test`: runs production simulation on loaded scenarios. `game`: live render+simulation with entity overlay (carriers, ships, military units), pause/speed controls. |

### What's working now

1. **Palette loading** — STADTFLD.COL parsed (256-entry RGBX format)
2. **SDL2 windowing** — sprite-viewer and island-viewer binaries run with vsync
3. **Sprite rendering** — STADTFLD.BSH sprites decoded with correct colors
4. **Scenario loading** — SZS chunk parser extracts islands with tile data
5. **Isometric rendering** — island tiles rendered in diamond projection using building_id → sprite index mapping
6. **All sprite sets loaded** — 33 BSH files across GFX/MGFX/SGFX (47,679 sprites total)
7. **World map mode** — all islands rendered together with isometric projection (W key)
8. **Multi-zoom sprites** — switch between GFX/MGFX/SGFX zoom levels (1/2/3 keys)
9. **Production simulation** — COD building defs → sim engine, production ticking with efficiency, input consumption, output accumulation
10. **Data bridge** — COD/SZS data loads into simulation BuildingDef/BuildingInstance types; 27 good types mapped
11. **Carrier dispatch** — carriers spawn when output > half capacity, walk to nearest warehouse, deposit goods, return and despawn
12. **Warehouse inventory** — per-island warehouses track goods with deposit/withdraw/capacity
13. **Population model** — 5-tier demand system, per-capita consumption from warehouses, satisfaction tracking with fulfillment-based blending, economy integration
14. **A* pathfinding** — 8-directional A* with octile heuristic, island walkability grids from building categories, diagonal corner-cutting prevention, fallback to direct movement
15. **Trade routes** — multi-stop routes with per-stop buy/sell config, ship movement, cargo load/unload, free trader AI for finding profitable trades
16. **Live game viewer** — combined render+simulation binary, real-time entity overlay (carriers=yellow, ships=cyan, military=green/red, warehouses=blue), pause/speed controls, title-bar HUD
17. **Marketplace coverage** — per-island coverage maps with Manhattan-distance radius from COD data, warehouse base radius 22, marketplace radius 30, public building overlay (churches, schools, taverns etc.), periodic recomputation
18. **Ocean pathfinding** — world ocean map from scenario data, 8-directional A* on ocean tiles (100k node limit), ships navigate around islands, path computed per route stop with direct-movement fallback
19. **Interactive building placement** — B key toggles build mode, 1-9 selects from paginated building list, click places with green/red validity preview, reverse isometric projection (screen→tile), footprint validation against island walkability grid, gold cost deduction, placed buildings join simulation with seeded inputs
20. **Sprite Y-offset fix** — building sprites (up to 286px tall) now drawn with base aligned to isometric tile diamond via `sy - (sprite_height - tile_height)` offset
21. **Sound integration** — AudioEngine (rodio backend) integrated into game binary, 21 music tracks auto-discovered from MUSIC8/, background music with auto-advance, M=toggle N=next V=volume controls, building placement sound effects, per-frame audio cleanup
22. **Multiplayer protocol** — Maxnet.dll fully decompiled (77 functions), DirectPlay-based protocol reverse-engineered: 12 message types (GameData/Pause/Resume/Ack/PlayerSync/SessionInfo/Chat/FragContinuation/Disconnect), confirmed send with ACK flow control, 4-player sessions, pause bitmask sync. `anno-net` crate implements TCP replacement with host/client architecture, non-blocking I/O, message fragmentation
23. **Sprite animation** — buildings with multiple animation frames (windmills, workshops, etc.) now cycle through frames in real-time using COD AnimAnz/AnimAdd/AnimTime parameters, ~100ms redraw interval, binary-search sprite→building lookup for efficient frame computation
24. **Minimap** — downscaled terrain overview in bottom-right corner with white viewport rectangle, click-to-navigate (clicking minimap scrolls main view to that location), semi-transparent dark background, auto-scales to fit 200×150px bounds while preserving aspect ratio

### What's missing / next

1. ~~Marketplace radius extension~~ ✓
2. ~~Ship pathfinding~~ ✓
3. ~~Interactive building placement~~ ✓
4. ~~Sound integration~~ ✓
5. ~~Multiplayer protocol~~ ✓ — Maxnet.dll fully decompiled (77 functions), protocol reverse-engineered, `anno-net` crate implemented with TCP transport

## Next Steps

### Phase 2: Game data pipeline ✓
5. ~~Fix COD parser~~ ✓ — Byte-negation encryption, 500 building defs parsed
6. ~~Parse save files~~ ✓ — SZS parser implemented
7. ~~Load all sprite sets~~ ✓ — GFX/MGFX/SGFX loaded (33 BSH files, 47,679 sprites)
8. ~~Verify building_id → sprite index mapping~~ ✓ — INSELHAUS building_id is direct sprite index (includes rotation/animation variants)
9. ~~Multi-tile buildings~~ ✓ — Each tile has own INSELHAUS record with independent sprite index; rendering works as-is

### Phase 3: Simulation (in progress)
10. ~~Production chains~~ ✓ — Data bridge wires COD/SZS to simulation; production ticker tested with real scenarios
11. ~~Carrier dispatching~~ ✓ — Carriers spawn, walk to warehouses, deposit goods, return, despawn. Direct movement (A* pathfinding TODO)
12. ~~Population model~~ ✓ — 5-tier demand system (Pioneer→Aristocrat), per-capita consumption, warehouse supply withdrawal, satisfaction tracking, economy integration (tax income, bankruptcy)
13. ~~A* pathfinding~~ ✓ — 8-directional A* with octile heuristic, island walkability grids from COD building categories, corner-cutting prevention

### Phase 4: Deeper RE (in progress)
13. ~~AI analysis~~ ✓ — AI controller with 3 personalities (Economic/Military/Balanced), building priority system, tax adjustment, difficulty scaling, cooldown-based decision pacing
14. ~~Combat formulas~~ ✓ — 10 unit types (6 land + 4 naval), per-type stats (HP/damage/speed/range), diplomacy matrix, engagement detection, per-tick damage, Lanchester battle prediction
15. ~~Trade routes~~ ✓ — multi-stop routes, cargo management, ship movement, free trader AI
16. ~~Multiplayer protocol~~ ✓ — Maxnet.dll decompiled (77 functions via Ghidra), DirectPlay protocol mapped (13 exports, 12 message types), `anno-net` crate with TCP replacement
