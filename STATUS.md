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
| `anno-formats` | All parsers verified | BSH: 5964 sprites. COL: 256-color palette. SZS: island chunks. COD: 500 building defs with sub-object properties (ProdKind, Ware, Rohstoff, Interval, Maxlager, construction costs). figuren.cod: figure (unit/ship) defs with resolved Gfx, Rotate, and per-figure ANIM blocks (AnimOffs/AnimAdd/AnimAnz/AnimSpeed). |
| `anno-audio` | Integrated into game | ADPCM codec tested (encode/decode roundtrip). AudioEngine with rodio backend: WaveManager (256 slots, spatial audio), StreamManager (8 slots, auto-detect finish). Plays 21 MUSIC8 tracks + SPEECH8 sound effects in game binary. |
| `anno-render` | SpriteManager tested | Framebuffer blitter, isometric camera, tile renderer. SpriteManager loads all 33 BSH files (47,679 sprites) across 3 zoom levels × 11 categories with case-insensitive path matching. |
| `anno-sim` | Full simulation engine | Data bridge, production ticker, A* carrier dispatch, warehouse delivery, population/economy model, AI controller (3 personalities), combat system (10 unit types, diplomacy), trade routes (multi-stop, cargo management), marketplace coverage (radius from COD, public buildings), ocean pathfinding (world map A*). 30 good types. 11 subsystem timers all active. |
| `anno-net` | Protocol implemented | Message protocol (12 command IDs: GameData, Pause, Resume, Ack, PlayerSync, SessionInfo, ChatMessage, FragContinuation, PlayerDisconnect). Session management (4 player slots, pause bitmask). TCP transport replacing DirectPlay (host/client, non-blocking I/O, message fragmentation). 8 tests passing. |
| `anno-game` | Four working binaries | `sprite-viewer`: browse sprites. `island-viewer`: isometric rendering with world map, 3 zoom levels, screenshots. `sim-test`: runs production simulation on loaded scenarios. `game`: live render+simulation with sprite-based entity overlay (carriers, ships, military units), pause/speed controls, building inspection (right-click), demolish mode, economy HUD. |

### What's working now

1. **Palette loading** — STADTFLD.COL parsed (256-entry RGBX format)
2. **SDL2 windowing** — sprite-viewer and island-viewer binaries run with vsync
3. **Sprite rendering** — STADTFLD.BSH sprites decoded with correct colors
4. **Scenario loading** — SZS chunk parser extracts islands with tile data
5. **Isometric rendering** — island tiles rendered in diamond projection using building_id → sprite index mapping
6. **All sprite sets loaded** — 33 BSH files across GFX/MGFX/SGFX (47,679 sprites total)
7. **World map mode** — all islands can still render together with isometric projection, but the former W shortcut has been removed because Appendix D assigns W to ship surrender
8. **Multi-zoom sprites** — switch between GFX/MGFX/SGFX zoom levels with the original F4/F3/F2 detailed/normal/bird-eye keys
9. **Production simulation** — COD building defs → sim engine, production ticking with efficiency, input consumption, output accumulation
10. **Data bridge** — COD/SZS data loads into simulation BuildingDef/BuildingInstance types; 27 good types mapped
11. **Carrier dispatch** — carriers spawn when output > half capacity, initialize their per-trip load cap from TRAEGER's `figuren.cod` `Maxtrag:4`, walk to the nearest warehouse, deposit goods, return, and despawn; any excess output remains in the source building for a later trip
12. **Warehouse inventory** — per-island warehouses track goods with deposit/withdraw/capacity
13. **Population model** — 5-tier demand system, per-capita consumption from warehouses, satisfaction tracking with fulfillment-based blending, economy integration
14. **A* pathfinding** — 8-directional A* with octile heuristic, island walkability grids from building categories, diagonal corner-cutting prevention, and carrier road-cost weighting from the decoded `Wegspeed` quad (empty: 145 off-road / 170 road; loaded: 120 off-road / 100 road)
15. **Trade routes** — multi-stop routes with per-stop buy/sell config, ship movement, cargo load/unload, per-ship cargo capacity from `figuren.cod` `Maxware × 10` (HANDEL1 40t, HANDEL2/HANDLER 60t), free trader AI for finding profitable trades
16. **Live game viewer** — combined render+simulation binary, real-time entity overlay (carriers=yellow, ships=cyan, military=green/red, warehouses=blue), pause/speed controls, title-bar HUD
17. **Marketplace coverage** — per-island coverage maps with Manhattan-distance radius from COD data, warehouse base radius 22, marketplace radius 30, public building overlay (churches, schools, taverns etc.), periodic recomputation
18. **Ocean pathfinding** — world ocean map from scenario data, 8-directional A* on ocean tiles (100k node limit), ships navigate around islands, and route-stop paths are recomputed from nearest navigable docking tiles. When an ocean map is present, a missing navigable route stalls the ship instead of falling back to land-cutting direct movement
19. **Interactive building placement** — B key toggles build mode, 1-9 selects from paginated building list, click places with green/red validity preview, reverse isometric projection (screen→tile), footprint validation against island walkability grid, gold cost deduction, and placed buildings join simulation without source-free input stock
20. **Sprite Y-offset fix** — building sprites (up to 286px tall) now drawn with base aligned to isometric tile diamond via `sy - (sprite_height - tile_height)` offset
21. **Sound integration** — AudioEngine (rodio backend) integrated into game binary, 21 music tracks auto-discovered from MUSIC8/, background music with auto-advance, music on/off and volume controlled from the O options menu, building placement sound effects, F opens the manual video/speech menu and the speech row gates routed announcement WAV playback, per-frame audio cleanup
22. **Multiplayer protocol** — Maxnet.dll fully decompiled (77 functions), DirectPlay-based protocol reverse-engineered: 12 message types (GameData/Pause/Resume/Ack/PlayerSync/SessionInfo/Chat/FragContinuation/Disconnect), confirmed send with ACK flow control, 4-player sessions, pause bitmask sync. `anno-net` crate implements TCP replacement with host/client architecture, non-blocking I/O, message fragmentation
23. **Sprite animation** — buildings with multiple animation frames (windmills, workshops, etc.) now cycle through frames in real-time using COD AnimAnz/AnimAdd/AnimTime parameters, ~100ms redraw interval, binary-search sprite→building lookup for efficient frame computation
24. **Minimap** — downscaled terrain overview in bottom-right corner with white viewport rectangle, click-to-navigate (clicking minimap scrolls main view to that location), semi-transparent dark background, auto-scales to fit 200×150px bounds while preserving aspect ratio
25. **Building inspection / info mode** — I opens the original-style info/status mode; left-clicking a building there opens the object card with name, construction/production state, stocks, service radius preview, and footprint highlight. Right-click any tile still opens the detailed inspection panel with production output/input stocks, efficiency percentage, warehouse inventory (top 8 goods), yellow diamond footprint highlight, and title-bar summary. Escape closes the active info/inspection surface
26. **Economy HUD** — on-screen overlay (top-left) showing population per tier with satisfaction/tax percentages, total population count, and gold balance. Pixel-art bitmap font renderer (4x5 glyphs, 2x scale). It auto-hides in build mode; H is reserved for the original warehouse-cycling control
27. **Building demolition** — Demolish mode removes player-owned buildings, with red highlight on the hovered footprint and a title-bar refund preview. Removes building tiles from island, restores walkability grid, removes building instance from simulation. The former D shortcut has been removed because Appendix D assigns D to diplomacy
28. **Entity sprite rendering** — carriers, ships, and military units now rendered using actual game sprites (TRAEGER.BSH, SHIP.BSH, SOLDAT.BSH) instead of colored dots. Direction-aware sprite selection (8 compass directions), walking animation frame cycling, zoom-level-matched sprite sets. Land military units render from SOLDAT.BSH using the four source variant ladders for SOLDAT/KAVALERIE/KANONIER/MUSKETIER and animate their moving walk frame from each family's decoded `AnimSpeed`; naval military units render from SHIP.BSH using KRIEG1/KRIEG2/PIRAT `figuren.cod` bases. Fallback to colored markers when sprites unavailable
29. **Tax rate panel removed** — the former T-key centered tax adjustment overlay changed population-tier tax rates with Up/Down and Left/Right. That global shortcut panel has been removed from the live keyboard surface because Appendix D does not assign T; tax rates and satisfaction still remain in the economy simulation and HUD, and faithful adjustment should later be reached through the original-style economy/city menu path rather than a standalone overlay
30. **Service coverage maps** — Coverage data tracks market/warehouse reach on each island (warehouse base 22, marketplace radius from COD) plus stacked public-building services (church/school/tavern/etc.). `Simulation.coverage_maps` is recomputed each market tick and feeds carrier dispatch; the former live C overlay has been removed because Appendix D assigns C to the cities list
31. **Manual speed keys restored** — F5/F6/F7 now set normal, double, and quadruple game speed, matching the original keyboard appendix. The removed quicksave/quickload shortcut no longer owns F5/F9; save/load remains on the L slot picker until the original menu shell is matched
32. **Military command / combat mode** — K opens the original-style combat mode; left-click on a player-owned military unit selects it there (Shift+click adds to selection), and left-click on an owned trade ship selects that ship. Right-click while units are selected issues a move-to order. Selection drawn as a yellow ring around the unit/ship, with a yellow target marker at the unit destination. Ctrl+1-9 stores the current living player-owned military selection as a troop assembly; 1-9 recalls a stored assembly. W makes the selected ship hoist the white flag and surrender to pirate slot 6; trade ships are detached from their route and pirate diplomacy is set to WAR with player 0. Sim adds military order movement every entity step while no combat target is set: naval units are clamped by the ocean map, and land units use island A* paths when a walkability map exists, stalling rather than crossing blocked terrain when no route exists. Combat takes priority. Escape closes combat mode and clears selection. Save format bumped to v2 to include the `move_timer_ms` field
33. **Diplomacy panel** — D key opens a centered overlay listing player 0's relation to each other slot (1-6). Up/Down selects counterpart, Left/Right cycles through Allied → Neutral → War (right) or the reverse (left). Edits go through `Command::SetDiplomacy`: War declarations remain unilateral and symmetric through `DiplomacyMatrix::set`, while Allied/Neutral proposals wait for the source diplomacy acceptance path instead of changing relations through a score shortcut. Empty/defeated player slots are tagged "(no player)". Color coding: green=allied, gray=neutral, red=war, yellow=selected row. The former fixed G tribute shortcut and panel-local T shortcut that gifted Tools have been removed; tribute waits for the source-shaped thumb/slider/hand flow from manual section 7.4. Goods transfers remain in the simulation command layer for faithful warehouse/ship-mediated flows
34. **Construction phases** — `BuildingInstance` now carries `construction_ms_remaining`/`construction_ms_total`; placed buildings start at 2000ms × footprint-tiles (min 2s) of build time. `is_built()` gates `production::tick_building` and `needs_carrier`, so a half-built mine produces nothing and dispatches no carriers. The render loop applies a blue tint over the footprint and draws a green progress bar at the back-top of the building until it finishes. Save format bumped to v3 for the new fields. 3 sim tests cover the gating + progress math
35. **Trade route editor removed** — the former R-key route-draft editor let the player click warehouses, draw numbered yellow stop markers, press L/U/B for stop modes, and press Enter to spawn a generated all-goods route plus ship. That global editor has been removed from the live keyboard surface because Appendix D does not assign R and the original route workflow should be reached through ship/warehouse menus rather than a map-wide debug-style draft mode. The underlying `TradeRoute`/`TradeShip` simulation and scenario ship loading remain
36. **Multiplayer wired into the game binary** — `--host PORT` binds an `anno-net` `NetHost` and broadcasts a bincoded `SaveState` snapshot to all peers every 1s; `--join HOST:PORT` connects a `NetClient`, deserializes incoming `GameData` messages, and replaces the local sim via `apply_snapshot`. Client mode skips local `sim.tick()` (host-authoritative) but still ticks animations so visuals don't freeze; title bar shows `[HOST :PORT (N peers)]` or `[CLIENT → addr]`. Loopback integration test in anno-net verifies a 4KB GameData payload round-trips host → client through the TCP transport
37. **Ship direction sprites + heading** — `TradeShip` carries `heading: u8` (0-7 compass) updated in `tick_trade_ship` from each step's movement vector via `trade::compass_heading`, plus a saved `TradeShipClass` so SHIP4-authored HANDEL1/HANDEL2 hull identity survives scenario import. Ship rendering now picks the live SHIP.BSH sprite as `figuren.cod Gfx + heading`: HANDEL1 starts at 0, HANDLER at 16, HANDEL2 at 32, KRIEG2 at 48, KRIEG1 at 64, PIRAT at 80, with dead/sinking hulls at `Gfx + 8`. Save format bumped to v20 for the class field. Tests cover compass heading, SHIP4 trader class handoff, and renderer sprite-index wrapping
38. **figuren.cod parser** — new `anno_formats::figuren::FiguresFile` decrypts the byte-negation encoding and parses figure definitions (carriers, ships, soldiers, animals): resolves `GFXTRAEGER`/`GFXSHIP`/etc. constants so `Gfx: GFXSHIP+32` lands as `gfx = 32`; recognizes `Objekt: ANIM` blocks (with their own `Nummer`, `AnimOffs`, `AnimAdd`, `AnimAnz`, `AnimSpeed`); skips top-level `Objekt: FIGUR`/`FORMATION` containers; supports `ObjFill: TEMPLATE`, `ObjFill: BASE` after `BASE = Nummer`, and `@Gfx: +N` to bump inherited offsets. Game binary loads it at startup and feeds the real per-figure layout into `overlay_entities`: carriers use TRAEGER's empty/loaded `AnimOffs` blocks, land units use SOLDAT1..4/KAVALERIE1..4/KANONIER1..4/MUSKETIER1..4 with decoded walk `AnimSpeed`, and live ships use the source `Gfx` base for HANDEL1/HANDEL2/HANDLER/KRIEG1/KRIEG2/PIRAT. Parser accessors now expose `Maxware`; the real-figuren tests verify TRAEGER `Speed:220`/`Maxtrag:4`, land soldier variant `Gfx` ladders and walk speeds, HANDEL1/HANDEL2/HANDLER/KRIEG/PIRAT cargo slots, all live ship `Gfx` bases, and TRAEGER/HANDEL1 rotate/anim counts
39. **Economy history graphs** — new `anno_sim::history::EconomyHistory` ring buffer (120 samples) sampled every population tick (~10s game time) with the human player's gold, total population, population-weighted average satisfaction, income, and costs. Game binary's G key opens a centered translucent panel showing three stacked bands: gold (yellow), population (green), avg satisfaction (blue, fixed 0-128 axis), with current value + max in each label. History snapshots are persisted in the save format (bumped to v5, `#[serde(default)]` keeps it forward-tolerant). 3 sim tests cover ring-buffer wrap-around and weighted-satisfaction math
40. **Production overview** — P key opens a panel that scans the human player's buildings and warehouses to aggregate, per output Good: producer count, average production efficiency (% of 128), and total stock pooled across warehouses. Goods we hold but don't produce (e.g. tradeable imports) appear as `n=0` rows so chronic gaps are obvious. Sorted by stock descending, with up to 20 rows. Uses the existing `tiny_font` renderer; no new sim state — pure read-only diagnostics
41. **Options menu** — O opens the original options menu surface. The centered panel exposes live music on/off, music volume, video-sequence, and speech-announcement rows; Up/Down selects a row, Left/Right/Enter changes it, and O/Esc closes. It reuses the same runtime state as the F video/speech menu instead of reviving the removed persistent F10 settings system; the former direct M/N/V audio shortcuts and Shift+V evaluation overlay are no longer bound
42. **In-game chat overlay** — Enter opens a single-line chat input (SDL2 TextInput-based), Enter sends, Esc cancels. Sent messages travel as `NetMessage::chat()` over `anno-net`: client → host, and the host re-broadcasts to all peers so everyone sees both sides. Bottom-left overlay renders the last 8 lines for 10s each, with green for self ("you:") and white for remote ("p<id>:"). Solo runs still log locally so the same UI works without a network. Earlier diagnostic graph/roster panel rebindings were later removed by the authenticity pass, so Enter and Tab keep their original commit / next-island duties
43. **AI military production** — `Simulation::tick_ai`'s `RequestMilitary` handler is no longer a stub: it spawns Swordsmen near the AI's first active warehouse, costing 100 gold per unit and capped by `player.gold` (gracefully stops mid-batch if the AI can't afford the full request). Units are placed in a 3×3 ring offset around the warehouse so they don't all overlap, and `military_maintenance` is bumped to keep the economy honest. With this wired up, the AI controller's existing `Military` personality (3/6/12 units on Med/Hard/Expert) actually fields an army, the player roster's `units` column and diplomacy panel become meaningful, and `tick_combat` / `tick_unit_orders` finally have something to chew on. New `ai_request_military_spawns_units` test in `simulation::tests` drives a one-shot AI tick and asserts unit creation + gold deduction
44. **AI building placement** — `Simulation::tick_ai`'s `RequestBuild` handler now actually builds: matches the cheapest `BuildingDef` whose `output_good` equals the requested good (and is within `player.gold`), anchors near the AI's first active warehouse, and spirals outward (max radius 12 tiles) for a footprint that fits via the new `IslandMap::find_open_spot`. On placement: deducts `cost_gold`, marks footprint tiles non-walkable in the `IslandMap`, pushes a `BuildingInstance` with `construction_ms_total = 2000ms × footprint_tiles` so the AI is also subject to construction phases. Two new island-map helpers (`can_fit`, `find_open_spot`) with their own tests, plus `ai_request_build_places_building` in `simulation::tests` covering the full happy path
45. **Live scenario picker removed** — the former F2 overlay for scanning `Szenes/` and re-execing the game has been audited out. Scenario files still load via the startup path argument, and F2 is again the original bird-eye zoom key
46. **Per-good price model** — `anno_sim::prices` now carries the original `DAT_0049ae50` ceiling table from `1602.exe`, indexed through an explicit `Good → text.cod [WARE]` mapping instead of enum discriminants; the matching floor uses the executable UI formula `ceiling * 300 / 1024`. The true WARE goods are source-priced (iron ore 90/26, gold 700/205, tools 140/41, wood 30/8, bricks 45/13, etc.), while local-only intermediates such as Stone, SugarCane, Fish, Meat, Grapes, Cotton, Silk, and WildGame remain named fallbacks because they have no player-facing WARE price slot. Replaces the flat 8-sell / 6-buy constants in `tick_trade_ship` and the AI's `SellExcess` 5-gold-per-unit shortcut, so trade-route profits and AI gold income now scale with the source goods being moved. Production overview panel grows a `buy/sell` column so the player can read each good's value alongside producer count and stock. 6 sim tests (5 in `prices::tests`, 1 `trade_uses_per_good_prices` end-to-end through `tick_trade_ship`)
47. **AI auto trade routes** — new `AiAction::EstablishTradeRoute` action emitted by `AiController::tick_trade` when (a) `trade_cooldown` has elapsed (8/12/20/30 ticks on Expert/Hard/Medium/Easy), (b) the AI has 2+ active warehouses on distinct islands, (c) gold ≥ 2000. Dispatcher in `tick_ai` deduplicates the AI's warehouses by island, takes up to 4 of them, builds a multi-stop `TradeRoute` with all 30 goods on every stop's load/unload list (mirrors the human-side editor), activates it, spawns a `TradeShip` at the first stop, and deducts a 1000-gold ship cost. Two new sim tests cover the eligibility rule (`ai_establishes_trade_route_when_eligible`, `ai_skips_trade_route_with_one_island`)
48. **Population growth & migration** — new `population::update_population_growth` runs every economy tick after `update_population_demands`/`tick_economy`. Each tier's per-tick delta = (sat - 64) × pop / 1280, clamped to ±50 people (sat=128 → +5%, sat=64 → 0, sat=0 → -5%). Fully-satisfied tiers (sat ≥ 96) promote 2% upward (Pioneer → Settler → … → Aristocrat); starving tiers (sat < 32) lose another 1% to emigration on top of the natural decay. `total_population` recomputed each tick from the per-tier vector. Six new tests in `population::tests` cover the par/full/starve growth-delta cases plus end-to-end promotion + emigration + empty-player-no-op behaviour
49. **Per-building maintenance aggregation** — every economy tick `tick_population` now sweeps active built `BuildingInstance`s, sums each one's `BuildingDef::maintenance_cost`, and writes the total into `Player::building_maintenance`. Was previously dead — `tick_economy` already factored `building_maintenance` into income/costs but nothing was writing it. Buildings still under construction (`!is_built()`) don't contribute, so the AI's freshly-placed sites don't hemorrhage gold before they're operational. Two new sim tests (`building_maintenance_aggregates_per_player`, `unfinished_buildings_do_not_pay_maintenance`) cover the multi-player aggregation and the in-construction exemption
50. **Player market panel removed** — the former A-key centered market overlay listed all 30 goods beside the player's first active warehouse and allowed direct buy/sell plus slider edits. That global shortcut panel has been removed from the live keyboard surface because Appendix D does not assign A and the original trading flow should be reached through warehouse/ship interaction rather than a standalone table. The underlying warehouse stock, price, and buy/sell command logic remains available to the simulation
51. **AI diplomacy reactions** — `Simulation::tick_diplomacy` no longer creates or clears diplomatic relations from a scalar strength score. The remaining AI code reacts tactically only to already-established `Diplomacy::War`: defenders can spawn near threatened AI warehouses and warships can escort AI trade ships, but score superiority no longer dispatches offensive raids. Source offensive AI still waits for the original player-slot state machine and cooldown fields. Sim tests include `ai_score_does_not_create_war`, `ai_score_does_not_end_war`, `economic_ai_does_not_declare_war`, `ai_score_does_not_dispatch_offensive_raid`, and the wartime defense/escort checks
52. **Building palette categories** — placer's flat list is now bucketed into 5 tabs (`PROD` / `RES` / `SVC` / `MIL` / `SPC`) via a new `BuildCategory::from_def` classifier that reads each `BuildingDef`'s `kind` + `prod_kind` (HANDWERK/ROHSTOFF/PLANTAGE/etc → Production; KIRCHE/MARKT/SCHULE/WIRT/etc → Service; MILITAR → Military; KONTOR/HQ → Special; WOHN → Residence). `BuildPlacer::next_category`/`prev_category` rotate the active tab, `[`/`]` are rebound from page-flip to category switch, and `PageUp`/`PageDown` retain the page navigation. Pagination, `1-9` selection, and the title-bar build list now operate on the filtered subset, so a "WOHN" residence and a "HANDWERK" workshop never share a page
53. **Save-slot picker** — L opens a centered overlay listing 10 named slots (`saves/<scenario>.slot{N}.bin`). Up/Down navigates, **S** writes the current snapshot to the highlighted slot, **L** loads from it, Esc closes. Each row shows the on-disk size in KiB or `(empty)` when no file exists; existing slots render in green, the selected row in yellow, empty slots in dim gray. The previous F5/F9 quicksave shortcuts have since been audited out; this picker is the remaining live save surface. Saves go through the same bincode round-trip as the rest of `anno_sim::save` (current SAVE_VERSION = 22)
54. **Building rotation in placer** — Z/X rotate `placer.orientation` in opposite directions through the selected building's `Rotate` count (read from haeuser.cod), matching Appendix D's counter-clockwise/clockwise pair. At placement, sprite indices skip ahead by `orientation × (anim_anz × anim_add)` so each rotated tile picks the correct slice in the BSH layout, and `IslandTile::orientation` is finally populated instead of always 0. Title bar reads `BUILD MODE [PROD] — … rot:2/4 — … Z/X=rot` while a multi-rotation building is selected; non-rotatable buildings hide the indicator. Orientation resets to 0 on selection change so the previous index can't overflow a building with fewer rotations
55. **Marketplace adjacency and load caps for carriers** — `carrier::try_spawn_carrier` consults the island's `CoverageMap` before dispatching: if the island has a coverage map and *none* of the production building's footprint tiles are inside the warehouse/marketplace service area, the carrier is not spawned. When an island map exists, dispatch also requires an actual loaded-carrier A* route to the warehouse; blocked routes leave output in the source building instead of creating a straight-line carrier. Returning carriers likewise avoid a direct fallback if the source route becomes blocked after delivery. When a carrier does dispatch, it uses `CarrierConfig`, initialized by the game binary from TRAEGER's `figuren.cod` `Maxtrag:4`, so output above the source cap remains in the building. Islands without coverage/path maps keep the legacy compatibility behavior
56. **AI building variety** — `RequestBuild` dispatcher used to deterministically pick the cheapest matching def, so an AI requesting "Food" 10 times built 10 copies of the same workshop. Now the dispatcher counts how many of each candidate def the AI already owns and minimises by `(existing_count, cost_gold)` — variety wins the primary tie-break, cost is the secondary. The AI's first cloth mill is still its cheapest option, but the second build of the same Good will diversify into the alternative. New `ai_request_build_prefers_variety` test seeds two equally-eligible Food defs (cost 500 vs 800) and asserts the AI builds one of each on back-to-back ticks instead of two of the cheapest
57. **Shortage banner removed** — the former persistent bottom-center `SHORTAGE: …` overlay and its `population::severe_shortages` helper were removed from the live game path. Population demand/supply still drives satisfaction, growth, and emigration, but missing goods are no longer surfaced through a modern global banner
58. **Sim events separated from chat overlay** — the game binary no longer diffs diplomacy/building state into `[diplo]` / `[build]` chat lines, and sim-originated `event_log` lines no longer render in the multiplayer chat feed. The chat overlay is again reserved for Enter/chat-protocol messages; drained sim events still drive voice-announcement routing where available
59. **Inspection detail panel** — right-click inspection used to cram everything into a single title-bar line. Now it also renders a multi-line top-right panel: building name (header), tile coords + footprint + owner + Kind, construction progress when unfinished, output good with stock/capacity, both inputs with stocks, efficiency %, upkeep/tick, and (for warehouses) the top 8 goods with stock/capacity plus an "N more…" footer when capped. Color-coded — yellow headers, green output line, blue construction status, gray detail rows. Reads straight from the existing `inspection` state so it stays in sync without any caching
60. **Path-debug overlay removed** — the former F6 carrier/ship path-dot overlay has been audited out of the live key loop. F6 is again the original double-speed key
61. **Per-island warehouse table** — U opens a centered grid where rows are goods and columns are the player's warehouses on the active island (one column per Kontor with its `(tile_x, tile_y)` header). Each cell shows `stock/capacity`, color-coded: green when stocked, red-orange when below 25% capacity, dim gray when empty. The good list is the union of every non-zero entry across the visible warehouses, sorted alphabetically. World mode dropping the island filter shows every player warehouse instead. Reuses the existing tiny_font + alpha-blended panel pattern
62. **AI defends warehouses** — `tick_diplomacy` now scans each AI's warehouses for hostile (`Diplomacy::War`) military units within 8 tiles and reactively spawns up to 2 Swordsman defenders per warehouse when the AI has gold (100/unit). Skips spawning when there's already enough nearby muscle so a long siege doesn't bankrupt the AI in a single tick. Threats are gated on actual war state, so neutral patrols passing by don't trigger a panic-spawn. Two new sim tests: `ai_defends_warehouse_when_threatened` (war + nearby enemy → defenders + gold spent) and `ai_does_not_defend_against_neutral` (neutral → no spawn)
63. **Own ships list / active-object jump** — S opens a centered panel listing the player's active ships, matching Appendix D. Trade ships and owned naval units show authored/fallback name, hull/class, and status instead of route ids, cargo internals, coordinates, health, or cannon debug. The former J freight diagnostic is replaced by Appendix D's active-object jump: J centers the camera on the selected trade ship, selected unit, selected building card, or inspected warehouse/building/tile
64. **Animated placer preview** — placer's hover preview no longer just shows a green/red footprint; it also blits each tile's actual building sprite at 50% alpha, run through `anim_state.animate()` so animated buildings (windmills, workshops) cycle their frames live before placement. The current orientation is honoured via `bb.sprite_idx + orientation × (anim_anz × anim_add) + dy×w + dx`, so rotating with **Z/X** flips the preview pose immediately. Validity overlay is drawn on top so unbuildable tiles still read as red even when the building sprite is opaque
65. **AI offensive raid score gate removed** — `tick_diplomacy` no longer issues attack orders from a scalar "winning" comparison. The previous placeholder picked an enemy warehouse during `Diplomacy::War` and sent roughly half of an aggressive AI's idle units at it when the AI's score exceeded the enemy's; that source-free offensive dispatch has been removed until the original player-slot offensive state and cooldowns are ported. Regression `ai_score_does_not_dispatch_offensive_raid` seeds 4 idle AI units, declares war, gives the AI a gold advantage, and asserts no unit targets the enemy warehouse from score alone
66. **Carrier loaded/empty animation** — the former per-good letter chip overlay was removed. Carriers now use the source-defined TRAEGER animations from `figuren.cod`: anim 0 (`AnimOffs:0`) for empty/returning carriers and anim 1 (`AnimOffs:64`) for loaded carriers. `FiguresFile::anim(n)` exposes the animation slots, the real-figuren parser test pins the TRAEGER offsets, and `game.rs` passes those parsed offsets into `overlay_entities` instead of recomputing the loaded base from a layout heuristic
67. **No panel auto-pause** — the live loop keeps simulation time running while panels are open, matching the original expectation that the player controls pause explicitly with the Pause key. The former `prev_modal_open` / `auto_paused` overlay pause layer has been removed, and Esc/right-click resume a manually paused game
68. **AI gold reinvestment threshold removed** — the former `AiController::tick` shortcut that halved build / military / trade cooldowns whenever AI gold exceeded 15 000 has been removed. AI cooldowns now decrement once per tick regardless of treasury size until the original player-slot pacing fields are decoded. Sim regressions cover rich and poor AI cooldown behavior.
69. **Per-good warehouse stock sparklines** — `EconomyHistory` gains a per-Good ring buffer (31 × 120 samples) populated by the new `record_full(player, warehouses, owner)` method which sums every active warehouse's stock per `Good`. `tick_population` calls it for player 0. Production overview panel now widens to ~460px and renders a self-normalised 90×8 sparkline at the right of each row, matching the row's color (green for actively-produced goods). Each row is independently scaled so a Wood spike doesn't flatten the Jewelry trace. Save format bumped to v6 for the new field; old saves get a backfill via `Vec::resize` on the `stocks` field. Two new tests (`record_full_captures_warehouse_stocks`, `record_full_sums_across_warehouses`)
70. **AI naval escorts** — `MilitaryUnit` gains `escort_ship: i32` (-1 = none). When at war, `tick_diplomacy` assigns each AI trade ship an idle naval unit as escort, or spawns a fresh `SmallWarship` at the ship's coords for 500 gold. New `combat::tick_escort_targets` runs every entity step, refreshing each escort's `target_x/target_y` from the shadowed ship — `tick_unit_orders` then walks the warship after the ship; `tick_combat` engages anything in range. Escort link clears automatically if the ship goes inactive. Save format bumped to v7 for the new field. Three new sim tests covering spawn, target tracking, and inactive-ship cleanup
71. **Manual ship-construction shortcut removed** — the former F4 instant trade-ship purchase has been audited out of the live key loop. F4 is again the original detailed zoom key; ship construction still needs the shipyard/buy flow
72. **Colony founding shortcut removed** — the former F7 instant-Kontor shortcut has been audited out of the live key loop. F7 is again the original quadruple-speed key; colony founding still needs the ship-delivered warehouse flow
73. **Buildings take combat damage** — `BuildingInstance` gains a `health: u16` (default 100). New `combat::tick_building_damage` runs each military tick: hostile non-naval units within 2 tiles of a building's footprint drain 5 hp/tick each (so 5 swordsmen flatten a 100-hp building in ~4 military ticks). When health reaches 0, the building is removed, walkability restored, and a `(island_id, x, y, w, h)` event is pushed onto `Simulation::tile_clears`. The game binary drains it each frame to clear `Island::tiles` (so the static renderer stops painting the dead building) and plays the destruction cue if available; no combat line is inserted into chat. Save format bumped to v8. 2 new sim tests cover the WAR-vs-NEUTRAL gating and IslandMap restoration after destruction
74. **Housing capacity gating** — `update_population_growth` now takes a `housing_cap: u32` (0 = uncapped). `tick_population` computes per-player capacity by summing 8 occupants per built `WOHN` residence and passes it through. After growth/promotion/emigration, if total population exceeds capacity, every tier is scaled proportionally so the surplus simply doesn't materialise that tick — preserves tier ratios while preventing unbounded growth. New `housing_cap_clamps_total` test seeds 600 population with cap 300 and asserts both the total clamp and the proportional scale-down
75. **Scenario objectives** — new `anno_sim::objectives` module with an `Objective` enum (ReachPopulation / ReachTotalPopulation / Build prod_kind / AccumulateGold / StockGood / Monopolies / SupportFellowPlayer) and `ObjectiveSet`. `tick_population` re-evaluates each unfulfilled objective against the human player; objectives only flip true (never revert), so spending past a gold target keeps the checkmark. Objectives are now empty by default and only populated from AUFTRAG4 mission goals when the scenario supplies flagged goals; the former generated tutorial checklist is no longer injected into continuous-play or goal-free scenarios. Completions are drained for the completion cue, not inserted into chat. The former `?`/`/` global objectives panel has been removed from the live keyboard surface because Appendix D does not assign those keys. Save format bumped to v9; ObjectiveSet derives Default + Serde with `#[serde(default)]` for forward-compat.
76. **Sim event SFX** — game binary loads three additional WAV slots from SPEECH8 (1010/1020/1030, with fallbacks) and triggers them on milestone events: building destruction (drained from `tile_clears`), objective completion (drained from `objective_completions`), and any new diplomacy WAR transition. Plays via existing `WaveManager::play_once` at screen center. Routed announcement playback now obeys the F video/speech menu's speech switch. Slots fall back to the existing placement-sfx file if the dedicated WAV isn't shipped, so the integration degrades gracefully on stripped data installs
77. **Per-island fertility** — INSEL5 fertility bytes are decoded through `anno_formats::szs::Fertility` and carried into `IslandMap`. Building defs derive `required_fertility` from haeuser.cod `Rohstoff`, and the AI + player placement paths reject plantations unless the island carries the matching fertility byte. The former y-position north/south live placer gate has been replaced by this source-data lookup; the player banner now says "build FAILED: needs Tobacco fertility", and the inspection panel lists the island's active fertilities instead of the old north/south badge. The obsolete y-split climate module/export has been removed. Tests cover the INSEL5 gate on barren vs fertile islands and the live label path
78. **Residence promotion** — `BuildingInstance` gains `house_tier: u8` (0=Pioneer, 4=Aristocrat). `tick_population` runs a promotion pass before maintenance/cap calc: any built `WOHN` whose current tier is fully satisfied (sat ≥ 100) bumps `house_tier += 1`. Housing capacity now scales with tier via `HOUSING_BY_TIER = [4, 8, 12, 16, 20]`, so promoting residences also expands the housing cap that gates `update_population_growth`. Save format bumped to v10 for the new field. New `residence_promotes_when_tier_fully_satisfied` test
79. **Multiplayer command sync** — new `anno_sim::commands::Command` enum (`SetTaxRate`, `SetDiplomacy`, `Buy`, `Sell`) with a magic-byte (`0x43`/'C') wire prefix to disambiguate from the host's untagged snapshot stream. `Simulation::apply_command` is the authoritative dispatcher, and the host's `SessionEvent::GameData` handler decodes any tag-prefixed payload as a `Command` and applies it. Defensive: client receive skips tag-prefixed payloads in case a stray one ever arrives. The `SetTaxRate` command remains for replay/simulation callers after the non-original T tax panel removal. 3 new sim tests cover round-trip encode/decode and the untagged-payload rejection
80. **Trade-route draft stop modes removed** — the former map draft editor carried per-stop LOAD / UNLOAD / BOTH modes and exposed L/U/B while route mode was active. Those live shortcut controls were removed together with the R-key editor; route stop data structures remain available to scenario import, free-trader logic, and any future faithful ship/warehouse route UI
81. **Pirate event scheduler source-waiting** — `tick_events` keeps the pirate-event hook, but live gameplay no longer rolls the former implementation-only 1-in-3 pirate spawn chance. Scenario-loaded pirate ships, player surrender to pirate slot 6, and pirate-faction combat remain active; new hideout-spawned pirates wait for the decoded source scheduler/target-selection state rather than being fabricated from a random gate.
82. **SZS writer retained outside live gameplay** — `SzsFile::encode_islands(&[Island])` emits a chunk-formatted byte stream (`INSEL5` + `INSELHAUS` per island) that round-trips back through `SzsFile::parse`. The former F8 in-game export shortcut has been audited out; the encoder remains available to tooling and covered by the `anno_formats::szs::tests` round-trip for number/dims/x_pos/y_pos and tile records (building_id, x, y, orientation, anim, flags)
83. **Replay system** — new `anno_sim::replay` module: `Recording { initial: SaveState, entries: Vec<(game_clock, Command)> }` with magic-byte (`REPL`) prefix + version field for forward-compat. `Recorder` buffers in-process; `save_recording` / `load_recording` handle on-disk serialization via bincode. `replay_into(&Recording, &mut Simulation)` applies the initial snapshot then dispatches every recorded command via the same `apply_command` the multiplayer host uses, so deterministic sim playback is one call. 3 new sim tests (record-then-replay round-trip, on-disk file persistence, bad-magic rejection)
84. **Settings file + F10 panel removed** — the modern persistent settings module and F10 panel were removed in the authenticity pass. Runtime options now live in the Appendix D O options menu and do not serialize a separate settings file
85. **Fixed market prices restored** — the former dynamic stock-sensitive price vector is no longer part of `Simulation`. `Simulation::current_price(good)` delegates directly to the fixed per-good `prices::price_of(good)` table, matching Anno's static buy/sell pricing; the regression test `current_price_uses_fixed_price_table` verifies that warehouse stock swings do not move prices
86. **Right-click context menu removed** — the former Shift+Right-click floating action menu is no longer in the game binary. Plain RMB keeps the original fixed action surface used by the current implementation: move selected units to the clicked tile, otherwise inspect the clicked tile/building/warehouse
87. **Material-gated construction** — placement now requires Wood, Tools, and Bricks drawn from the player's warehouses on the target island, not just gold. New `Simulation::warehouse_pay_materials(island_id, owner, wood, tools, bricks)` does an all-or-nothing pre-check + atomic withdraw across multiple warehouses. Player placer reports "build FAILED: need N wood, M tools, K bricks" when short; AI's `RequestBuild` silently retries next tick when it can't afford the materials, so the AI economy is now genuinely constrained by its supply chain. 2 new sim tests cover sufficient-stock pay-down and short-stock refusal
88. **Day/night cycle tint removed** — the non-original terrain-wide color cycle is no longer in the render path; terrain keeps the source palette except for object-specific overlays such as construction progress, drought/depleted status, and selection/coverage markers
89. **Perf overlay (F12)** — bottom-right panel showing rolling 60-sample averages of sim-tick microseconds, render+blit microseconds, total frame microseconds, and FPS. Times are captured around `sim.tick` and the post-`canvas.present` interval; `render_us = frame_us - sim_us` so the breakdown is exact. Disabled by default (no overhead unless toggled) and renders on top of every other panel, so debugging a slow combat tick or a renderer regression is one keypress away
90. **AI placement reachability** — `IslandMap::find_reachable_spot` extends `find_open_spot` with a connectivity check: each candidate must be reachable from a given anchor (the AI's warehouse) via 4-directional walkability BFS (cap 4096 nodes). New `IslandMap::reachable` falls back to a walkable 4-neighbour when the anchor itself is blocked (e.g. its own footprint sits on a now-built tile), so the AI's second placement still finds the graph. AI's `RequestBuild` dispatcher swaps to the reachable variant so it never lands buildings on islets the carriers can't walk to. 3 new island-map tests cover open-map reachability, wall-cut blocking, and isolated-half avoidance
91. **A-key market panel removed after command routing** — Buy/Sell commands still route through `Simulation::apply_command` and read `sim.current_price`, but the former A-key bulk-trade panel and its Left/Right/Shift controls are no longer part of the live keyboard surface. Future faithful trading UI should attach those command paths to warehouse/ship menus rather than a global market table
92. **Trickle construction materials** — `BuildingInstance` gains `wood_needed` / `tools_needed` / `bricks_needed`. Placement no longer pre-deducts materials from warehouses; instead the building stores its outstanding requirements. Each entity tick attempts to draw 1 of each pending material from any player warehouse on the same island, and `construction_ms_remaining` only decrements once all three counters hit 0. `is_built()` checks both timer and materials, so a building short on bricks visibly stalls in-progress until supply catches up. Save format bumped to v11. 2 new sim tests cover stall-on-shortage and trickle-from-warehouse
93. **Trade-route management panel removed** — the former Shift+R centered routes panel listed every player-owned `TradeRoute` and allowed Backspace/Delete removal with ship despawn. That global manager has been removed from the live keyboard surface because Appendix D does not assign Shift+R and the original flow should surface route management through ship/warehouse menus rather than a standalone debug-style table. The underlying `TradeRoute` and `TradeShip` simulation types remain for the route-draft path and scenario loading
94. **Fishery coastal-only placement** — `IslandMap::is_coastal(x, y)` returns true iff the tile is walkable AND any 4-neighbour is out of bounds (i.e. on the perimeter of the island). Game placer rejects any building whose `output_good == Good::Fish` unless at least one footprint tile is coastal, with a "Fisheries must be placed on the coast" banner. Per-island coastal logic re-uses the existing walkability bitmap so no new sim state is needed. New `coastal_tiles_are_on_the_perimeter` test covers corners + edges + interior
95. **Idle building maintenance halving** — `BuildingInstance` gains `idle_ticks` counter. `tick_production` increments it every production tick the building yielded 0 output (and resets on any positive output). When `idle_ticks ≥ IDLE_MAINTENANCE_THRESHOLD` (5), the building's maintenance contribution to `Player::building_maintenance` is halved — so a workshop starved of inputs no longer drains gold at full rate, and the moment supply returns the count resets and full upkeep resumes. Save format bumped to v12. New `idle_building_maintenance_halves` test exercises both directions
96. **Fog-of-war exploration map** — new `anno_sim::exploration::ExplorationMap` (per-island bool grid). `Simulation::exploration` is lazily populated; `tick_exploration` runs each market tick and reveals a 5-tile square around every player-owned warehouse, building, and (land) military unit. `mark_radius` clamps at edges, `coverage_128` reports fraction explored on the same 0..=128 scale used elsewhere. Bits only flip true. Renderer dimming pass is left as a follow-up; the data layer is ready. 3 unit tests in `exploration::tests` plus an end-to-end test that drives `tick_exploration`
97. **Build queue panel** — Q opens a centered list of every player-owned `BuildingInstance` with `!is_built()`. Each row shows name, position, progress %, and outstanding wood/tools/bricks; rows are colored green when materials are all in (just waiting for the construction timer) and amber when still waiting on supply. Empty case renders "(no buildings under construction)". Read-only — diagnostic complement to the trickle-construction system
98. **Combat damage floating numbers** — `combat::DamageEvent` records every hit (`tick_combat` for unit-vs-unit, `tick_building_damage` for building damage). `Simulation::damage_events` is drained each frame by the game binary into a `floating_nums` Vec; each is rendered as a `-N` label drifting upward 24px and fading over 1.5s, color-coded amber for buildings and red for units. Numbers anchor on tile coordinates → texture coordinates → screen coordinates so they line up exactly with the hit. `tick_combat` and `tick_building_damage` signatures both grew an `events: &mut Vec<DamageEvent>` parameter; existing tests pass `&mut Vec::new()`
99. **Per-faction ownership rings** — military units now render a thin faction-color ring under their sprite (blue=human, orange/green/purple/pink/yellow for AI slots 1-5, red for pirates). Yellow selection ring still wins precedence when a unit is selected, so the ring system unifies "who is this?" and "is this me?" reads. `player_color(owner)` table lives in the renderer; later tasks can reuse it for building outlines and chat-prefix coloring
100. **Civilian wanderers** — the removed cream-pixel vector has been replaced by simulation-owned `Figure` civilians spawned on the source 4999 ms residence cadence. The game loads `ADELWEIBL`..`PILGER` from `figuren.cod` into `CivilianConfig`, so spawned civilians carry the source definition-order marker (`0x58..=0x5f`) and use the decoded `Gfx + AnimOffs` TRAEGER.BSH bases plus the decoded walk-frame count. Fallback constants preserve assetless tests, but live gameplay renders the source sprites and consumes the shared source RNG stream
101. **Free trader arrivals** — `tick_events` now also has a 1-in-5 chance per tick to drop a small parcel of an exotic good (Spices / Cocoa / Silk / Jewelry) into the player's first warehouse at +25% of its current market price; failure modes (no warehouse, can't afford, warehouse full) cleanly skip with no banner. `Simulation::event_log` is drained by the game binary for voice-announcement routing only, not for chat text
102. **Quicksave thumbnails removed** — the former F5 thumbnail dump was removed with the dedicated quicksave path, leaving F5 available for the original normal-speed control
103. **Treaty proposals** — `Command::SetDiplomacy` is now a proposal when `state != War`. Unilateral declarations of war still take effect immediately; alliances and peace no longer use `(units×10 + gold/200)` or `civilization_power` acceptance shortcuts. Until the original acceptance/cooldown path is ported, non-war proposals leave the current relation unchanged and emit a source-waiting diplomacy event for cue routing. Diplomacy panel routes through `apply_command`
104. **Tax panel growth preview** — each tier row now shows per-tier income contribution and the next-tick population delta (`+18 g, Δ+5`), so the player can read both the gold and demographic consequences of a tax tweak before committing. Panel widened to 380px to fit
105. **Stockpile alerts** — new `tick_stockpile_alerts` runs every 6 market ticks; warehouses ≥ 90% capacity on any good push a `[stock] warehouse @ (x,y) overflowing on <Good> (qty/cap)` event into `event_log` for voice-announcement routing. These alerts no longer render as chat text
106. **First-launch tutorial banner removed** — the former bottom-center hint is no longer a live in-game overlay. The shipped tutorial scenarios are the replacement path for first-time guidance
107. **Diversified AI military spawn** — `RequestMilitary` no longer spits Swordsmen-only; spawn distribution is 40% Swordsmen / 30% Musketeers / 20% Cavalry / 10% Cannons with per-type costs (100 / 150 / 200 / 300). Spawning stops mid-batch if the AI can't afford the next pick
108. **Production chain panel (X)** — lists every `BuildingDef` with an `output_good` as `inputs → output [n owned]`. Rows colored green when the player owns at least one producer of that good and red-orange for missing producers, so chain gaps are obvious. Up to 28 chains shown in a 460-px panel
109. **Keys help panel (F11)** — read-only reference listing every keybinding grouped by purpose (build/city, trade, diagnostics, world/view, save/system). Until full rebinding lands, this gives the player a lookup that doesn't require memorising the doc-comment header
110. **Workspace regression suite** — the new ticks, authenticity removals, source-data parsers, and entity-pathing changes are covered by the workspace tests; use the current `cargo test --workspace` output for exact counts rather than pinning a stale total in this ledger
111. **Fire Maxbrand cap** — `BuildingInstance` stores `fire_damage_ticks`, and `disaster::ignite_building` stops applying fire damage after the source `haeuser.cod` default `Maxbrand: 4` count parsed at `1602_exe.c:68086`. Save format bumped to v21 for the bincode layout change. Disaster regression `ignite_stops_at_maxbrand_tick_cap` covers the default cap
112. **Free trader target-seek gate** — after a free trader finishes docking, the sim now keeps it in the target-seeking state until the source `1602_exe.c:57709-57714` gate `(rand() & 3) == 0` fires, then runs the existing `FUN_004547e0` / `FUN_004549f0` nearest-port/profit selector. A targetless trader no longer re-docks at its old Kontor just because its stale target coordinates equal its current position. Two regressions pin the idle targetless state and the gated post-dock retarget
113. **Dynamic Kontor capacity** — shipping scenarios do not pre-place KONTOR tiles in INSELHAUS, so the game-side dynamic Kontor fallback now uses `haeuser.cod`'s base Kontor `Maxlager: 50` (`Nr=271`, `INFRA_KONTOR_1`) instead of the legacy placeholder 30-cap warehouse. The full authored Kontor `Maxlager` audit is pinned as `[50, 75, 100, 20, 20, 20, 20]`
114. **Free trader target-gate attribution** — the `(rand() & 3) == 0` executable citation applies only to post-dock target seeking (`1602_exe.c:57709-57714`). New-trader admission follows the manual-backed warehouse-count population rule instead of borrowing that target gate.
115. **Per-definition Maxbrand handoff** — `BuildingDef` now carries the inherited `haeuser.cod` `Maxbrand` value as `max_brand_damage_ticks`, `load_building_defs` verifies the shipping corpus inherits `4`, and `ignite_building_with_cap` accepts the selected building definition's cap instead of using a hard-coded global. Regression `fire_effect_uses_building_definition_maxbrand_cap` proves the lower-level fire effect respects the definition cap.
116. **Ruinenr parser/handoff** — `CodFile` now resolves the original `Ruinenr` token table from `1602_exe.c:66354-66367`, including `@Ruinenr` relative updates after source-format `ObjFill` template copies. `BuildingDef::ruin_id` carries the resolved code into simulation definitions (`0xff = NORUINE`), and the parser/bridge tests pin the Kontor and destroyed-Kontor ladder values from `haeuser.cod`
117. **Source ruin replacement on destruction** — combat-destroyed buildings now emit a named `TileClear` carrying the source `Ruinenr` byte. The game-side drain resolves the executable ruin table from `1602_exe.c:68896-68918` through parsed `haeuser.cod` `Id` values, removes the destroyed footprint, and inserts land/strand/multitile ruin sprites instead of only deleting INSELHAUS tiles. `NORUINE` still clears without replacement; authored ruins remain blocking in the island walkability map
118. **Source ruin random variants** — `CodFile` now parses the executable `RandAnz`/`RandAdd` fields at building-definition offsets `0x60`/`0x62` and exposes `ruin_variant_building`, which follows the original definition-stride selection `rand() % RandAnz * RandAdd` from `FUN_00463f40`. The game-side destruction drain consumes draws from the simulation's shared source RNG, one draw for same-footprint ruin replacement and one draw per fallback cell, and places the selected variant definition's `Gfx` ladder. Regression tests pin field-ruin variant selection where `RandAdd = 1` advances to a definition whose sprite jumps by 6.
119. **Shared source RNG stream** — `anno_sim::source_rand::SourceRand` now pins the Microsoft C runtime `rand()` sequence used by the executable after `srand(GetTickCount())` (`1602_exe.c:106312-106313`). The simulation event/free-trader/civilian/disaster random branches use that stream instead of the former xorshift state, consuming additional draws for branch choices that previously borrowed high bits from the wide xorshift word. Live gameplay seeds the single simulation stream from uptime milliseconds, and ruin replacement draws from that same stream through `Simulation::next_source_rand`; unit tests use explicit seeds and pin the first MSVC outputs.
120. **PLAYER4-seeded diplomacy baseline** — game startup now initializes `Simulation::diplomacy` from PLAYER4's parsed per-slot relationship table instead of forcing player 0 and AI slot 1 to WAR whenever slot 1 is active. Authored hostile special-faction edges, such as native-faction SHIP4 raiders, are still applied on top of that baseline. The game-binary regression keeps an active slot-1 AI neutral when PLAYER4 relationships are neutral.
121. **Scenario-authored startup units and ships** — the game binary no longer seeds fixed land militia at hard-coded coordinates and no longer creates a generated player trade route/ship between the first two warehouses. Startup military units and trade ships now enter through authored SHIP4 records only; empty/two-city test scenarios prove no synthetic units, routes, or ships are added.
122. **STADT4-only startup population** — scenario initialization no longer injects fixed fallback inhabitants for player 0 or AI slot 1 when STADT4 population fields are empty or absent. Each city's `tier_population` contributes exactly to its `owner_slot`, and every player total is recomputed from those authored tier counts. Empty/no-city scenarios now start at zero population until gameplay creates inhabitants.
123. **PLAYER4-exact startup treasury** — scenario initialization now applies each present PLAYER4 slot's `starting_gold` exactly, including zero, instead of retaining `Player::new_human` / `Player::new_ai` constructor defaults when the source record says 0. The game-binary regression covers a zero-gold human slot and a nonzero AI slot.
124. **No synthetic processor input stock** — scenario-loaded processing buildings and newly placed processing buildings now start with zero `input_1_stock` / `input_2_stock` unless actual supply later delivers goods. The removed shortcut had filled every input buffer to `Maxlager` despite `INSELHAUS` carrying no per-building stock fields. Game-binary regressions cover both startup and placement paths.
125. **No synthetic SHIP4 trader routes** — authored SHIP4 trader hulls now spawn as `TradeShip`s with the unrouted sentinel route id. Startup no longer invents per-owner Wood↔Tools round trips between the first two warehouses, because no decoded route table is attached to the parsed SHIP4 trader record. A game-binary regression covers a trader plus two warehouses still producing zero `TradeRoute`s.
126. **STADT4-backed cities list** — the C cities panel now lists parsed STADT4 city records by name, owner slot, population total, and island number instead of treating every active warehouse as a "city" and rendering owner/island/tile debug rows. Foreign cities remain hidden unless a trade agreement with player 0 exists. A game-binary regression covers own-city visibility, trade-agreement visibility, neutral-city hiding, blank-name skipping, and row formatting.
127. **Source-named ships list** — SHIP4 trader names now survive into `TradeShip`, save format v22, and the S panel lists owned ships by authored/fallback name, class, and status instead of route ids, cargo internals, coordinates, health, or cannon debug. Regressions cover the data-bridge name handoff, save round-trip, and row rendering.
128. **Scenario-bounds free-trader spawn** — newly admitted roaming free traders now spawn on the edge of the loaded `OceanMap` derived from SZS island extents, scanning that edge for a navigable ocean tile, instead of using the old fixed 200×200 debug square. The assetless fallback remains only for tests without an ocean map. Regressions cover the edge chooser and the live `tick_free_traders` spawn path.
129. **Free-trader ocean routes** — `assign_next_port` now converts a selected Kontor into the nearest navigable ocean docking tile and fills the free trader's ocean A* path from the loaded `OceanMap`. With an ocean map present, an unreachable inland target leaves the trader targetless instead of falling back to a land-cutting straight line. Regressions cover routed docking paths and unreachable-port refusal.
130. **Free-trader departure routes** — free traders with no candidate Kontor, or with a selected Kontor that cannot be connected to ocean, now set `leaving = true` and sail to the nearest navigable loaded-world edge before despawning. The no-ocean 200×200 edge is only the assetless fallback. Regressions cover no-candidate departure through `tick_one` and unreachable-port departure without land cutting.
131. **No direct fallback for ocean-routed free traders** — free-trader targets derived from the loaded `OceanMap` now carry a path-required marker, so `tick_one` will not silently resume straight-line grid movement if the ocean route is absent or consumed before reaching the target. Direct movement remains only for assetless/no-ocean fallback targets. Regressions cover both the no-drift failure mode and arrival at an already reached ocean target.
132. **Manual-count free-trader admission** — new roaming free traders now appear as soon as the active Kontor count calls for another ship, instead of passing through the removed implementation-only 1-in-4 spawn gate. The fidelity registry no longer lists `free_trader_spawn_gate`; RNG use remains on edge selection and the binary-backed post-dock target-seek gate. A regression uses seed 1, whose first MSVC rand value would have failed the removed gate, and proves the trader is admitted.
133. **Free-trader score WARE-domain clamp** — `free_trader_port_profit_score` now follows `1602_exe.c:58704 FUN_004549f0`'s good loop over original `text.cod [WARE]` ids 2..0x18. Local-only intermediates such as Silk, Fish, Cotton, WildGame, Grapes, Meat, Stone, and SugarCane no longer make a port attractive during target selection just because the Rust simulation has fallback prices for them. Regressions cover non-WARE sell offers and non-WARE trader cargo both scoring zero.
134. **Base Kontor capacity constructor** — `Warehouse::new` now uses the source base Kontor `Maxlager: 50` instead of the old 30-cap placeholder. Explicit `with_capacity` callers still carry authored 50/75/100/20 Kontor variants, and the serde default remains 30 only for legacy snapshots missing `default_capacity`. A warehouse regression covers default construction, capacity, deposit clamping, and stock.
135. **No fabricated volcano origins** — `tick_disaster_event` no longer turns a random player-owned building into a volcano eruption centre. The lower-level `disaster::erupt_at` effect remains available, but live eruption scheduling now waits for decoded source volcano anchors instead of applying the speculative 1-in-32 branch to every settlement. The fidelity probability registry also dropped the unused `fire_extinguish` and inactive `volcano_eruption` rows so unresolved odds only describe active simulator behavior. A regression uses source-rand seed 2, which would have triggered the removed branch, and proves no `[volcano]` event or damage is fabricated.
136. **No scalar-score AI relation flips** — `tick_diplomacy` no longer declares war, sues for peace, or backs out of war from the implementation-only `civilization_power` ratio. The remaining tactical AI code may react defensively or assign escorts during an already-established `Diplomacy::War`, but relation changes now require explicit/source-loaded diplomacy paths until the original `FUN_0042adf0` / `FUN_0042b000` / `FUN_0042b160` state-machine and cooldown fields are ported. Regressions prove an overpowered military AI stays Neutral without a source relation change, and an outmatched AI at War does not auto-reset to Neutral from the score alone.
137. **No scalar-score treaty acceptance** — `Command::SetDiplomacy` no longer lets a stronger proposer force Allied or Neutral relations through `civilization_power`. War remains a unilateral relation change; non-war proposals now return false, leave the current relation untouched, and log that source diplomacy acceptance is still needed. A regression proves a gold/population/unit-dominant player cannot force alliance from Neutral or peace from War through the score path.
138. **No fabricated fire origins** — `tick_disaster_event` no longer picks a random human-owned building and ignites it through the speculative `FIRE_IGNITION_GATE`. The lower-level capped fire effect remains available, but live fire scheduling now waits for decoded source triggers/targets just like volcano scheduling waits for source anchors. The fidelity probability registry dropped `fire_ignition`, and regressions prove seed 8 no longer fabricates `[fire]` damage while the per-definition `Maxbrand` helper still caps direct fire ticks.
139. **No pirate spawn without source hideout** — `tick_pirate_event` no longer falls back to spawning a pirate ship at a random offset near the targeted player trade ship. Regression `pirates_do_not_spawn_without_source_hideout` seeds the removed random gate and proves no pirate unit or forced pirate-war edge is fabricated without a source event trigger.
140. **No scalar-score AI offensive raids** — `tick_diplomacy` no longer sends idle AI units at an enemy warehouse because a `civilization_power`/score comparison says the AI is winning. Wartime defense spawning and naval escorts remain gated on existing `Diplomacy::War`; source offensive AI still waits for decoded player-slot offensive state and cooldowns. Regression `ai_score_does_not_dispatch_offensive_raid` locks the removed path down.
141. **No provisional pirate spawn gate** — the speculative `pirate_event_spawn` probability row and `PIRATE_EVENT_GATE` constant were removed from the live fidelity registry. Even with an active player trade ship, a built `PIRATWOHN`/`Piratflg:1` hideout, and a seed that satisfied the old 1-in-3 gate, `tick_pirate_event` now leaves pirate units, pirate diplomacy, and the source RNG stream untouched until the original event scheduler is decoded. Regression `pirate_event_does_not_spawn_from_hideout_without_source_trigger` pins that behavior.
142. **No fixed-key tribute transfer** — the diplomacy panel no longer maps G to a hard-coded 500-gold `GiftGold`. The Anno 1602 manual section 7.4 says tribute is paid by clicking the attitude thumb, choosing an amount with a slider, then clicking the hand; until that UI/state is implemented, the live panel only exposes player selection and relation proposals. Regression `diplomacy_panel_help_omits_fixed_tribute_shortcut` keeps the removed shortcut out of the visible panel text.
143. **No unpinned PLAYER4 AI personality mapping** — PLAYER4 `slot_u16_0x18` remains parsed and corpus-pinned, but it no longer drives live AI personality or difficulty. Until the executable semantics are decoded, `personality_from_slot_byte` returns Economic/Medium for every value rather than converting observed scenario bytes into invented Easy/Balanced/Military/Hard behavior. Regression `personality_from_slot_byte_does_not_invent_unpinned_mapping` covers observed and out-of-range values.
144. **No gold-stockpile AI cooldown acceleration** — `AiController::tick` no longer treats a 15 000-gold treasury as permission to halve build, military, and trade cooldowns. That threshold had no decoded source owner and changed live AI pacing from a scalar wealth check. Cooldowns now only receive their normal tick decrement until the original per-player pacing state is ported. Regression `rich_ai_does_not_accelerate_cooldowns_from_gold_stockpile` pins the removed path.

### Authenticity audit (2026-05-03)

After a faithful-clone audit of the 110 features above, several
were found to be modern-strategy-game drift rather than authentic
Anno 1602 mechanics. The following have been **removed** from the
codebase to keep the project honest:

- **#67 Auto-pause for info panels** — Anno never paused on menus.
- **#85 / #91 Dynamic market prices** — Anno used FIXED per-good prices.
- **#86 Right-click context menu** — Anno's RMB had a fixed action.
- **#88 Day/night cycle tint** — Anno 1602 has no day/night cycle.
- **#95 Idle building maintenance halving** — Anno paid full upkeep.
- **#97 Build queue panel** — no such panel in Anno.
- **#98 Combat damage floating numbers** — never in Anno.
- **#100 Cosmetic civilians (cream pixels)** — Anno had real sprites
  walking real paths; the pixel-stub implementation was misleading.
- **#101 Free trader teleport arrivals** — Anno's free trader was a
  ROAMING SHIP that visited warehouses, not gold-for-goods teleports.
- **#102 PPM thumbnail dump** — concept fine but PPM file isn't shown
  in the slot picker; reverted until rendering catches up.
- **#105 Stockpile chat alerts** — Anno used voice announcements.
- **#106 First-launch tutorial banner** — Anno had a tutorial scenario.
- **#108 Production chain panel** — modern strategy UI, not Anno.
- **#48 Chat `/gold +500` commands** — Anno had no console.
- **#44 Wake-up alarm SFX (using destroy sample)** — should be
  speech samples; reverted until proper voice wiring lands.

A faithful replacement pass for some of these (real free trader ship,
proper voice announcements, real civilian sprites) is queued.

### Authenticity audit pass 2 (2026-05-03)

Second sweep targeting modern flat-list diagnostic panels and other
remnants. Anno surfaced data through right-click info windows on the
specific object you cared about, not through global tables. The
following have been **removed**:

- **Economy graph panel (G)** — three-band gold/pop/satisfaction
  history overlay was a strategy-game add-on.
- **Production overview panel (P)** — global per-good aggregate table
  with sparklines was modern UI; Anno used per-building info windows.
- **Player roster panel (Tab)** — global slot listing isn't Anno;
  diplomacy in Anno is per-opponent.
- **Per-island warehouse table (U)** — global stock-by-warehouse grid;
  Anno surfaced this by right-clicking the Kontor.
- **Carrier-cargo letter chip** — single-letter goods overlay on
  carriers was a debug crutch. It has been replaced by the source-defined
  TRAEGER empty/loaded animation swap from `figuren.cod`.
- **Treaty proposal scoring formula** — the arbitrary
  `units * 10 + gold/200` rule and its later `civilization_power(slot)`
  replacement have been removed from relation changes and score-only
  offensive dispatch. Actual non-war acceptance and offensive AI still wait
  for the original relation/cooldown state machines behind `FUN_0042b4b0`
  and the per-personality dispatchers `FUN_0042adf0/b000/b160`.

### Authenticity audit pass 3 (2026-07-08)

Third sweep removed remaining non-original utility overlays and their
backing state from the live code path:

- **#84 F10 settings panel** — in-game persistent settings UI is a
  modern shell convenience, not an Anno 1602 game screen. The panel,
  input handling, title/header references, and unused
  `anno_sim::settings` support module were removed.
- **#89 F12 perf overlay** — frame-time diagnostics are useful for
  development but are not part of the original playable surface. The
  overlay, timing sampler, and F12 toggle were removed from the game
  binary.
- **#109 F11 keybindings panel** — removed from the game binary;
  stale control-list references to already-audited K/P/O/U/Q/context
  menu panels were also removed so the executable no longer advertises
  non-faithful global screens.
- **#39 / #69 economy history and warehouse sparklines** — the graph
  panel had already been audited out as a strategy-game add-on, so the
  `EconomyHistory` sampler, per-good stock history, `anno_sim::history`
  module, and `SaveState.history` field were removed. Save format is
  bumped to v17 because new saves no longer carry graph-only bytes.
- **#98 combat/deposit floating-number event stream** — the renderer
  already suppressed floating numbers; the leftover `DamageEvent`
  type, `Simulation.damage_events`, carrier deposit events, and
  combat/disaster event parameters were removed so live simulation no
  longer manufactures non-original visual events.
- **#97 build-priority residue** — the audited-out build queue left a
  player-overridable construction-priority field, command, and
  priority sort in the material trickle tick. Those were removed so
  pending construction drains materials in natural placement order.
  Save format is bumped to v18 because `BuildingInstance` no longer
  serializes the priority byte. The `idle_ticks` field remains only as
  internal `Maxnorohst` / `Doerrflg` state; its raw cycle counter is no
  longer shown in the inspection panel.
- **#60 F6 path-debug overlay** — the carrier A* / ship ocean-path dot
  overlay was a development diagnostic rather than an Anno 1602
  screen. The toggle, input binding, render flag, and debug-dot drawing
  block were removed; movement path state remains part of the actual
  carrier and ship simulation.
- **#63 J fleet/freight panel** — the global read-only trade-ship table
  was the same flat diagnostic UI class as the previously removed
  production, roster, and warehouse tables. The `J` binding, toggle
  state, and centered panel were removed. Ships, cargo, routes, and
  sprite rendering are unchanged; the original S own-ships list is now
  restored separately.
- **#104 tax-panel growth preview** — the tax panel remains, but the
  predictive per-tier income and next-population-delta suffix was
  removed. Rows now show only current tax, satisfaction, and
  population values; no duplicated growth formula runs in the UI.
- **Shift+J music jukebox panel** — the centered MUSIC8 track picker
  exposed internal asset filenames as an in-game utility panel. The
  picker, Shift+J binding, selection state, and render block were
  removed. Background music playback and the existing M/N/V controls
  remain.
- **#71 / #72 F4 ship spawn and F7 colony drop** — those shortcuts
  created trade ships at the first warehouse and Kontors directly on
  the active island, bypassing the original shipyard / ship-delivered
  colony flows. The bindings, magic-spawn branches, and stale city-list
  hint were removed.
- **S screenshot hotkey** — the game binary no longer exposes a plain
  `S` PPM capture shortcut. Slot-save `S` inside the L save panel is
  unchanged.
- **#82 F8 scenario export hotkey** — SZS encoding remains in
  `anno-formats`, but the live game no longer writes
  `saves/<scenario>.export.szs` from a developer hotkey.
- **Shift+E in-game scenario editor residue** — the live game no
  longer exposes the minimal editor mode, free finished placement,
  owner cycling, objective mutation hotkeys, or editor HUD. Normal
  placement is again always player-0 owned and resource / construction
  gated.
- **#99 synthetic ownership markers** — the generated per-unit
  faction-color rings and per-building owner chips were removed from
  the main render pass. Unit sprites, building sprites, selected-unit
  yellow ring feedback, and minimap settlement dots remain.
- **#45 F2 live scenario picker** — the in-game overlay that scanned
  `Szenes/` and re-execed the current binary with a different `.szs`
  has been removed from the playable loop. Scenario files still load
  through the startup path argument; an original-style front-end menu is
  separate from live gameplay.
- **#31 F5/F9 quicksave and quickload** — the dedicated
  `saves/<scenario>.quicksave.bin` shortcut path was removed from the
  live key loop. The L slot panel remains as the explicit save/load
  surface while the eventual front-end/menu flow is matched.
- **Manual speed keys** — F5/F6/F7 now set normal, double, and
  quadruple game speed per the original keyboard appendix, replacing
  the modern F/G incremental speed controls.
- **Manual zoom keys** — F2/F3/F4 now select bird-eye, normal, and
  detailed sprite zoom per the original keyboard appendix, replacing
  the modern +/- and mouse-wheel display-zoom controls.
- **Manual pause key** — Pause now pauses the game per Appendix D, while
  Esc or right-click resumes a manually paused game. Space no longer owns
  pause in the live key loop.
- **Manual white-flag key** — W now applies Appendix D's ship surrender
  action instead of toggling the world map. In combat mode, selected owned
  trade ships or naval units transfer to pirate slot 6; selected trade
  ships are detached from their player route.
- **Manual video/speech key** — F now opens Appendix D's video sequences /
  speech menu. Up/Down selects the video-sequence or speech-announcement
  row, Left/Right/Enter toggles it, and the speech row gates routed
  objective/event/destruction announcement WAV playback.
- **Manual active-object jump key** — J now centers the camera on the
  current active object per Appendix D: selected trade ship, selected
  military unit, selected building info card, or the current right-click
  inspection target. The former J freight diagnostic remains absent.
- **Manual options key** — O now opens/closes Appendix D's options menu.
  The live menu controls music playback, music volume, video sequences,
  and speech announcements using the existing runtime state; the former
  O player-roster diagnostic remains absent.
- **Manual troop assembly keys** — Ctrl+1-9 stores the current selected
  player-owned military units, and 1-9 recalls that stored troop assembly.
  The former normal-mode 1/2/3 formation and zoom fallback shortcuts were
  removed from the live key loop; build mode still uses 1-9 for the active
  construction page.
- **Manual warehouse-cycle key** — H now cycles the camera through the
  player's active warehouses per the original keyboard appendix,
  replacing the modern HUD visibility toggle.
- **Manual diplomacy key** — D now opens/closes the diplomacy panel per
  the original keyboard appendix. Y no longer toggles diplomacy, and the
  former D demolish shortcut has been removed from the live key loop. The
  later fixed G tribute shortcut, Shift+G tribute multiplier, and panel-local
  T shortcut for gifting Tools have also been removed from the live diplomacy
  surface; the underlying gift commands remain for future source-shaped
  tribute/warehouse/ship interaction paths.
- **Manual rotation keys** — Z/X now rotate the selected build preview in
  opposite directions, matching Appendix D's counter-clockwise/clockwise
  pair instead of the former Z-only cycle.
- **Manual cities key** — C now opens/closes the own and trade-agreement
  cities list per Appendix D. The diagnostic service-coverage overlay no
  longer occupies a live gameplay key.
- **Manual ships key** — S now opens/closes the player's own ship list per
  Appendix D, covering active trade ships and owned naval units.
- **Manual info key** — I now opens/closes info/status mode per Appendix D.
  Building object cards are gated to that mode instead of opening on every
  normal left-click.
- **Manual combat key** — K now opens/closes combat mode per Appendix D.
  Unit selection is gated to that mode instead of being active on every
  normal left-click.
- **#58 event log on chat overlay** — the game binary no longer diffs
  diplomacy/building state into `[diplo]` / `[build]` chat lines, and
  sim-originated `event_log` lines no longer render in the multiplayer
  chat feed. Objective-completion lines likewise no longer enter that
  chat feed. The drained events still drive the original-style audio cue
  path where available.

### Authenticity audit replacements (2026-05-03)

Faithful replacements for items removed in the two audit passes,
with RE references where available:

- **#101 Free trader as roaming ship** — `crates/anno-sim/src/free_trader.rs`.
  Spawns at the world edge, sails to player Kontors via the existing
  ocean A*, docks for several ticks exchanging goods at standard
  `prices::price_of` rates, then leaves.
  - **Spawn rule** — Anno 1602 manual section 11.4.3 "Placing ships":
    *"A free traders' ship will automatically be placed as soon as
    two warehouses have been built in your island chain. … the more
    warehouses built in your chain of islands, no matter which colour
    player has built them, the more free traders there will be."* So
    `target_traders = active_kontors / 2`, capped at 8.
  - **Spawn origin** — newly admitted traders enter from the edge of
    the loaded `OceanMap` derived from SZS island extents, with a
    navigable edge tile selected from the source-shaped RNG stream.
    Assetless simulations retain only a test fallback square.
  - **Default stock** — manual section 8.1 *"Tools and raw materials,
    such as iron ore, may be scarce at the beginning, and the traders
    carry a supply of such items."* Trader carries `Tools` and `Ore`,
    capped by `figuren.cod` `Nummer: HANDLER` `Maxware: 6` (= 60 t);
    other goods are added when players sell them to the trader, again
    only up to the HANDLER hold capacity.
  - **Target-seek gate** — `1602_exe.c:57713`'s `(rand() & 3) == 0`
    gate is applied when the trader is seeking its next target after
    docking. New-trader admission follows the manual warehouse-count
    rule and has no separate stochastic gate.
  - **Target shortlist** — `1602_exe.c:58597 FUN_004547e0` keeps the
    12 nearest candidate ports by `|dx| + |dy|/4`; `FUN_004549f0`
    then scores trade profit only for original WARE ids 2..0x18 and
    picks the best positive-scoring port, falling back to a random
    shortlisted reachable port only when no profitable trade exists.
    `assign_next_port` now applies both
    stages, maps the selected Kontor to an ocean docking tile, and
    fills a loaded-`OceanMap` A* path, so distant 13th+ Kontors are not
    eligible for the current hop even when they advertise buy/sell
    sliders. If no shortlisted port can be connected, the trader leaves
    by the nearest loaded-world edge rather than waiting indefinitely.
  - **Object identity** — type tag `0x35 = HANDLER`
    (`:11248 s_HANDLER_004982c8`, `:47208` resolver), per-ship struct
    stride `0x86` at `&DAT_004cf358` (`:50190 FUN_004487e0`).
    Renderer uses the HANDLER live SHIP.BSH base from `figuren.cod`
    (`Gfx: GFXSHIP+16`) instead of the player HANDEL1/HANDEL2 hulls.
  - **Player slot** — `:83179` confirms slot 4 with 1M starting gold
    and ID `0xd`.
  - **Tick rate** — 10 Hz / 600 ticks-per-minute decoded from `:98053`
    time-display `sprintf("%02d:%02d", DAT_005b6040 / 600, …)`.
  - **No visit cap** — `local_68 < 0xc` is treated as the port
    shortlist size, not a ship lifetime. After docking, a trader
    waits for the source target-seek gate, then retargets another
    Kontor instead of leaving after 12 visits.
  - The remaining `DOCK_TICKS` dwell value is still a simplification
    until the at-Kontor dialog counter is pinned.
- **#100 Civilian wanderers** — `crates/anno-sim/src/civilian.rs`.
  A dedicated 4999 ms building dispatcher spawns one of eight
  civilian figures from each eligible finished `WOHN` building,
  then the figure walks three tiles in a random compass direction
  and despawns. RE: `1602_exe.c`
  lines 84620 (`if (4999 < DAT_005491c8)` per-building tick), 84666
  (`FUN_00443a90(0x5a, ...)` ALTER spawn), 94389
  (`FUN_00443a90(0x5c, 1, sVar11, 1)` PASSANT spawn), 11856 (ADEL
  spawn on player init), 46943 (`FUN_00443a90` figure-allocator).
  `figuren.cod` definition order indices `0x58..=0x5f` resolve to
  ADELWEIBL/ADEL/ALTER/FRAU/PASSANT/VETERAN/KINDREIF/PILGER at
  `GFXZIVIL` + `N×64` (= TRAEGER.BSH sprite bases
  1272/1336/1400/1464/1528/1592/1656/1720, computed from the
  `GFXTRAEGER..GFXZIVIL` chain). The game renderer now treats these
  as their own `sim.figures` class, uses the parsed civilian walk-frame
  count from `figuren.cod`, and indexes TRAEGER.BSH from the stored
  GFXZIVIL base without applying the carrier loaded/empty offset.
- **Carrier sprite by cargo (audit #66 follow-up)** — `figuren.cod`
  `Nummer: TRAEGER` defines exactly two walk animations: anim 0 at
  `AnimOffs:0` (empty) and anim 1 at `AnimOffs:64` (loaded). The
  original game does NOT have per-good carrier sprites — the loaded
  silhouette is generic. Renderer in `game.rs` now picks anim 1 when
  the figure's action is `CarryingGoods`, anim 0 when `Returning`.
  No "letter chip"; the sprite swap IS the indicator.
- **#105 / #44 Event audio cues** — `crates/anno-game/src/bin/game.rs`
  loads three RE-cited SAMPLES/*.wav files used by the original
  for in-game alerts and plays them per drained `event_log` line
  keyed by line prefix:
    - `event.wav` (`1602_exe.c:106460` `_DAT_005b5e4c =
      _MaxwaveLoad_4(s_event_wav_…)`) — generic alert ping for
      stockpile / treasury lines.
    - `piraten.wav` (`:106441`) — pirate / hostile-attack warning.
    - `triumph.wav` (`:106444`) — successful trade / treaty / victory.
  The numbered SPEECH8 WAVs are voice sentences (used elsewhere for
  citizen-demand speech); per-event playback uses the named SAMPLES
  files above. The F video/speech menu's speech switch now gates these
  routed announcement cues.
- **#106 Tutorial scenario instead of first-launch banner** — the
  five `Tutorial0..4.szs` scenarios shipped with the original under
  `extracted/Szenes/`. They remain launchable as scenario files; the
  removed first-launch banner is not replaced by an in-game hint.

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
11. ~~Carrier dispatching~~ ✓ — Carriers spawn, walk A* paths to warehouses, deposit goods, return on A* paths, and despawn, with their load cap initialized from TRAEGER's parsed `Maxtrag:4`
12. ~~Population model~~ ✓ — 5-tier demand system (Pioneer→Aristocrat), per-capita consumption, warehouse supply withdrawal, satisfaction tracking, economy integration (tax income, bankruptcy)
13. ~~A* pathfinding~~ ✓ — 8-directional A* with octile heuristic, island walkability grids from COD building categories, corner-cutting prevention, and decoded loaded/empty `Wegspeed` road-cost weighting

### Phase 4: Deeper RE (in progress)
13. ~~AI analysis~~ ✓ — AI controller with 3 personalities (Economic/Military/Balanced), building priority system, tax adjustment, difficulty scaling, cooldown-based decision pacing
14. ~~Combat formulas~~ ✓ — 10 unit types (6 land + 4 naval), per-type stats (HP/damage/speed/range), diplomacy matrix, engagement detection, per-tick damage, Lanchester battle prediction
15. ~~Trade routes~~ ✓ — multi-stop routes, cargo management, ship movement, free trader AI
16. ~~Multiplayer protocol~~ ✓ — Maxnet.dll decompiled (77 functions via Ghidra), DirectPlay protocol mapped (13 exports, 12 message types), `anno-net` crate with TCP replacement
