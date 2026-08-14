# Playing the first mission flawlessly — roadmap

Goal: the Rust engine supports a complete, correct playthrough of the
campaign's first mission, **"Halfway there"** (`New Horizons0.szs` — the
campaign titles in text.cod `[KAMPAGNE]` map onto the `New Horizons*` /
`On His Majesty's Service*` / … scenario families in arc order).

## The mission

- Human (slot 0) starts with **one ship, no settlement**: the LargeTrader
  *Verena* at world (131, 239) carrying the classic loadout — **50 + 10
  tools, 50 wood, 20 food** (ship-cargo ids 2/7/4 = tools/wood/food,
  live-verified against the original's hold UI; an earlier registry-based
  reading of ore/meat/wool was wrong).
- AI (slot 1) owns the two big islands (Abemsberg, Abmont) and a warship.
- Twelve pristine islands (30×30 / 40×40 / 50×52) are free to settle.
- AUFTRAG4 goals: **100 total inhabitants** and **100 Aristocrats** —
  i.e. found a colony and climb the entire civilization ladder with every
  production chain and infrastructure tier.

## Engine gap map (2026-08-14)

Working already: goal decode + objective evaluation; scenario loading;
construction (`place_building` with terrain/gold/tier gates and full
source-record integration); production + carriers; the exact
demand/consumption cycle; kind-13 population growth (houses seed at
amount 0x40 = 1 resident and grow at good satisfaction — the deferred
immigration event block only matters ≥ 200 residents); promotions with
material costs; the infrastructure coverage scan; per-root operating
costs; trade primitives (Buy/Sell/LoadShip/UnloadShip/DispatchCart).

Missing, in dependency order:

1. **Stock-island instantiation.** The twelve free islands ship as
   `INSEL5` records only (size + position + all-7 fertilities + ore
   records) — no tiles. The original picks a stock island file at
   scenario start: `FUN_00469690` formats
   `<base><climate><size><NN>.SCP` (climate dirs `Nord\/Sued\/NordNat\/
   SuedNat\` at `0x49add0`, size prefixes `lit/med/mit/big/lar`; the
   pick is random per playthrough) and `FUN_00469770` loads the file's
   INSELHAUS into the island record. `.scp` files parse with the
   existing chunk reader (`INSEL3` 40-byte header + plain INSELHAUS).
   The Rust needs: family selection by width (30=lit, 40=med, 50=mit,
   100=big), climate by map-half, a **seeded deterministic pick**,
   fertility rolling for the all-7 records, and INSEL5 ore-record
   application, feeding the normal island-map path.
2. **Authored ship cargo → gameplay.** `TradeShip.source_cargo_slots`
   is retained raw but never decoded into the typed `cargo` — spawn
   must decode ids 2/7/4.
3. **Player ship sailing.** `MoveUnit` only drives military units;
   trade ships move on routes. Add a direct-sail command reusing the
   ocean path machinery (`compute_path_to_stop` internals).
4. **Kontor founding.** No flow exists: place the Kontor def on a
   beach adjacent to the ship (free of materials, as in the original),
   allocate the source city record, create the `Warehouse`, unload the
   ship cargo into it, run the coverage scan.
5. **Coverage-scan trigger on placement.** The scan currently runs at
   load and after kind-13 replacements; `place_building` must dirty the
   island too (the original sets the island dirty flags on every
   construction).
6. **The playthrough driver.** A scripted player
   (`crates/anno-game/examples/play_mission1.rs` → later a pinned
   regression) staging: sail → found → wood/food/cloth economy →
   pioneers → chapel + settlers → infrastructure ladder + luxury
   chains (southern-island colonies or AI trade for
   tobacco/spice/cocoa) → 100 aristocrats, asserting objective
   completion and economy sanity at each stage.
7. **Late-stage growth fidelity.** Above ~200 residents the deferred
   city event block (immigration waves, `FUN_0047a020`) becomes
   active-path; port it when the driver reaches that scale (full RE
   notes in the task and `docs/original-capture.md`).
