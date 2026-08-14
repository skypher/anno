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

## The colony plan (2026-08-14)

> **RETRACTED — the island fertility table that stood here was wrong.**
> It came from `Island::fertilities`, which parsed `INSEL5[0x0C..0x14]`.
> Those eight bytes are the **per-settlement owner array** (7 = unsettled),
> not crop fertilities — the save writer `FUN_00468740`
> (`1602_exe.c:72890-72897`) copies them from runtime `island+0xac`. The
> "93 % of slots read 7" observation that made the sentinel look plausible
> was simply every unsettled settlement slot.
>
> Real fertility is the u32 crop bitmask at `INSEL5[0x5C..0x60]` → runtime
> `island+0x5c` (`:72847`), tested as
> `island+0x5c & (1 << (ware - 0x2d))` (`FUN_0046b0a0` / `FUN_0046aff0`,
> `:74674`). The repo already parses it correctly as
> `IslandSourceResourceState::crop_flags`; the fertility API just never
> used it. Spot values read off the real mask: islands 4 and 7 carry
> full-strength cotton (`0x1191`), island 10 is **Cocoa** (`0x11c1`), and
> a baseline `0x1181` (Grain / Grass / Wood / Fish) is present everywhere.
>
> Two further findings came out of the fix. The original **never rolls
> fertility on stock-island instantiation**: `FUN_00469770` (`:73780-73810`)
> keeps the scenario's authored mask when the island's climate byte
> (`INSEL5[0x64]`) matches the map half — which it always does in shipping
> content — and otherwise resets it to bare `0x1181`. The only synthesis
> site in the whole executable is the editor's random-island path
> (`:44273`). And `INSEL5[0x64]` is itself a **half-strength fertility
> channel**: `FUN_0046aff0:74676-74684` yields `0x40` (rather than `0x80`)
> for Tobacco/Sugarcane/Vines in the north and Spices/Cotton/Cocoa in the
> south even when the mask bit is clear. Half-strength feeds a per-tile
> dither, so it means "grows on some tiles", not "refused".

The real authored fertilities, read off the shipped INSEL5 records. Bit
index is `ware - 0x2d`; `0x1181` (Grain + Grass + Wood + Fish) is present
on every island. The AI holds 0 and 1; there is no island 12.

| isl | size | clim | full strength | half strength |
| --- | --- | --- | --- | --- |
| 0 | 100×90 | S | Spices, Cotton, Cocoa | — |
| 1 | 100×90 | N | Tobacco, Vines | Sugarcane |
| 2 | 40×40 | N | **Sugarcane** | Tobacco, Vines |
| 3 | 40×40 | N | — | Tobacco, Sugarcane, Vines |
| 4 | 40×40 | S | Cotton | Spices, Cocoa |
| 5 | 40×40 | N | Tobacco | Sugarcane, Vines |
| 6 | 40×40 | N | Tobacco | Sugarcane, Vines |
| 7 | 40×40 | S | Cotton | Spices, Cocoa |
| 8 | 50×52 | N | **Vines** | Tobacco, Sugarcane |
| 9 | 50×52 | S | Spices | Cotton, Cocoa |
| 10 | 50×52 | S | **Cocoa** | Spices, Cotton |
| 11 | 30×30 | S | — | Spices, Cotton, Cocoa |
| 13 | 30×30 | S | — | Spices, Cotton, Cocoa |
| 14 | 30×30 | N | — | Tobacco, Sugarcane, Vines |

The demand ladder needs Food, TobaccoProducts, Spices, Cocoa, Alcohol,
Cloth, Clothing and Jewelry, so no single island can finish the mission —
which is the authored point of the scenario. Reading the ladder against
the table:

- **Cocoa** is on the home island (10) at full strength — convenient, and
  the largest free island in the scenario shares that size with 9 and 8.
- **Spices** at full strength is island 9 (50×52, south).
- **Tobacco** at full strength is island 5 or 6 (40×40, north).
- **Alcohol** is the awkward one. Full-strength Vines exist only on island
  8 (50×52, north) and full-strength Sugarcane only on island 2 (40×40,
  far north at (36,20)) — the home island has neither, at any strength,
  because Sugarcane and Vines are northern crops and island 10 is
  southern. Half-strength is not an escape here.
- **Cloth** needs no fertility (sheep farm on grass, which every island
  has) and **Clothing** is the tailor at STUFE_3C.
- **Jewelry** needs gold ore, which is not in the crop mask at all — ore
  rides separate 8-byte INSEL5 records. Islands 0, 1, 11, 13 and 14 carry
  ware `0x02` (iron); island 0 also carries `0x03`. Whether `0x03` is the
  gold deposit is not yet confirmed — open question for the endgame.

So the mission's authored shape is: settle island 10, then take a northern
island for alcohol, then a southern one for spices and a northern one for
tobacco.

Once the BAUINFRA ladder landed the early chains became unambiguous. What
matters is a building's `Bauinfra` rung, not its fertility: the fertility
crops are mostly *late*. Def indices and rungs from `examples/dump_defs`:

| def | building | output | rung | available |
| --- | --- | --- | --- | --- |
| 270 | fishery | Food | NIX | t0 |
| 403 | hunter's hut | Food | NIX | t0 |
| 402 | forester | Wood | NIX | t0 |
| 412 | sheep farm | Wool | NIX | t0 |
| 388 | weaving hut | Cloth ← 2 Wool | NIX | t0 |
| 386 | butcher | Food ← 2 Meat | STUFE_1A | 30 total |
| 390 | rum distillery | Alcohol ← 2 Sugar | STUFE_2C | 40 settler+ |
| 409 | vineyard | Alcohol ← 4 Grapes | STUFE_2C | 40 settler+ |
| 385 | bakery | Food ← 2 Flour | STUFE_2D | 75 settler+ |
| 405 | cotton plantation | Wool ← Cotton | STUFE_3C | 200 citizen+ |
| 387 | tailor | Clothing ← Cloth | STUFE_3C | 200 citizen+ |

So the **cloth chain needs no fertility and no rung at all**: sheep farm →
weaving hut, both placeable from tick 0. The cotton plantation is a
*later, denser* wool source, not the early one — an earlier draft of this
plan had that backwards. Since group-1 satisfaction saturates on cloth
alone, the entire pioneer → settler step is reachable on the home island
with t0 buildings.

Alcohol is the first good that needs a rung *and* a crop: STUFE_2C opens
at 40 settler-and-above, and the raw material is Sugarcane or Vines —
neither on the home island at any strength, which is the gate that forces
the second settlement.

A harvester also has to be **sited on its raw resource**. The worker
searches the static map roots for ripe (kind 9) cells whose output ware
matches the building's `Rohstoff`, inside the building's compiled
`Radius` measured as a circle (`FUN_00404d70` rows). Island 10 carries
559 BAUM (ware 53), 627 GRAS (52) and 585 FISCHE (57) cells, none near
the west-coast founding anchor — so "nearest free tile" siting gives a
forester with no trees. Ware slot is `0x2d + crop bit`.

## Stage status

- **Stage 1 (done).** Sail → found → market + chapel + 10 huts; the
  pioneer settlement grows to the hut cap.
- **Stage 2 (done).** With cloth and alcohol injected into the
  warehouse, tier-1 satisfaction rises from 0 to 128 and pioneers
  promote to settlers (first promotion ~3 sim-minutes after supply
  starts, charged ~194 gold), then settlers accumulate while a tier-2
  reservation opens. This confirms the promotion gate is supply-driven,
  not time-driven: the stage-1 plateau at `sat[1] = 0` was faithful.
- **Stage 3 (partly done).** The injection is gone and the driver places
  the real t0 buildings — forester, fishery, market, chapel, four huts,
  two sheep farms, weaving hut — in a wood-bounded order, the forester
  first since it is the only wood source and costs no wood. All of them
  place and complete with no construction stall.

  The first run produced **nothing at all**: food, wool, wood and cloth
  flat at zero. An audit found six stacked defects, no subset of which
  yields a partial result that could be measured, all now fixed
  (`c778115`):

  1. Live map-cell records were admitted on the **outer** `HAUS Kind:`
     (`+0x04`) instead of the nested `HAUS_PRODTYP Kind:` (`+0x1c`) that
     `FUN_00481450` switches on (`1602_exe.c:92790-92892`). Every real
     production building is outer `Kind: GEBAEUDE`, so none got a live
     record; they fell through to a legacy path whose efficiency is 0.
  2. The type-11 city-cart supplier filter tested the outer kind; the
     original has no kind test and scores by the `Ware` byte
     (`FUN_004717b0`, `:80580-80586`). No cart could collect anything.
  3. The type-8 workshop-carrier filter had the same fault; the correct
     predicate is `Ware == requested || Ware == ALLWARE` (`:80352-80353`).
  4. The harvest worker's delivery was discarded — only the idle-cooldown
     tail of `FUN_0047d940` was ported, never the buffer credit
     (`:89797-89807`), so the rate `min(128, (stock<<7)/Rohmenge)` was
     permanently zero. Note `def+0x23` is `Workstoff`, not the raw ware.
  5. `Maxlager`, `Prodmenge`, `Rohmenge`, `Workmenge`, `Interval`,
     `Maxnorohst`, `LagAniFlg` and `Randwachs` are authored inside
     `Objekt: HAUS_PRODTYP`, but the COD parser ran its typed handlers
     only at the top level of a block, so **all 500 definitions carried
     zero**. `Maxlager == 0` fails the activity guard before anything
     else matters — the highest-leverage item of the six.
  6. A `"MARKT" => 7, "KONTOR" => 8` outer-kind fallback existed only to
     prop up mislabelled test fixtures.

  A forester now runs the full chain on island 10: raw buffer 32 →
  storage 320 (its cap) → output 10 (`Maxlager`). Two things still stop
  the *mission* flow from getting there, both open:

  - **Harvest candidates are rejected once a Kontor exists.**
    `admits_plantation_worker_path` requires
    `cell.source_map_owner_slot == worker_owner`, but wild resource cells
    are all slot 7 while founding stamps the player's buildings with the
    city selector 0. The same forester at the same tile produces with no
    Kontor and never harvests with one. Commit `e4e9282` — which made
    `place_building` prefer the city record over the tile's owner bits —
    is the likely cause and is under review.
  - **Nested kinds 4, 5 and 6 spawn no worker figures**, so the sheep
    farm, hunter and fishery never harvest even though island 10 carries
    627 ripe GRAS and 585 FISCHE cells. (An earlier claim that no stock
    island ships ripe grass was wrong.) This blocks *all* t0 food, not
    just cloth: the fishery and hunter are the only food sources before
    STUFE_1A.

- **Stage 4 (after).** The maturation timer `FUN_0047ca80` (`:88970`).
  Without it a harvested cell never regrows, so even a working forester
  strips its woodland and stalls permanently — and a newly placed field
  tile never ripens at all, which is what plantation agriculture needs.
