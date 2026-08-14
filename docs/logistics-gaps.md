# Production / logistics gaps — RE reference

Companion to `docs/growth-timer.md`. Addresses are VA in `extracted/1602.exe`;
line numbers are `decompiled/1602_exe.c`. Rust symbols are named rather than
line-cited because the files move; grep for the symbol. Claims are VERIFIED
against the decompilation or disassembly unless marked INFERRED or OPEN.

Anchor for the line/VA correspondence: `test byte ptr [edi + 0x6a], 0x40`
disassembles at `0x004089eb`, which is `LAB_004089eb` at `:7617`.

**Two claims in the original defect list were wrong** and are corrected below:
the city-store capacity formula already matches the source (§5), and the
carrier path templates are not a missing invalidation — the original keeps no
path cache at all (§3).

## 0. Vocabulary

Three unrelated things get called "type 8":

| term | meaning |
| --- | --- |
| production kind `def+0x1c` | nested `HAUS_PRODTYP Kind`. 1 `HANDWERK`, 2 `PLANTAGE`, 7 `MARKT`, 8 `KONTOR`, 9 `ROHSTOFF`, 10 `ROHSTWACHS`, 13 `WOHNUNG` |
| map kind `def+0x04` | outer `HAUS Kind`. 19 `MEER`, 35 `HQ`, 12/29 the ruin kinds |
| figure type | `FUN_00446ca0` arg 1. 8 workshop carrier, 11 city cart, 12 plantation worker |

Storage is 1/32-ton fixed point throughout: `0x20` = 1 t, `0x140` = 10 t.

## 1. The founded Kontor never gets a live cell record

**Confirmed.** There is no separate "found Kontor" command in the original — a
Kontor placed from a docked ship is an ordinary build through `FUN_004084d0`
(`:7423`), which per accepted tile runs `FUN_00465170` (settlement + claim),
then `FUN_00481450` (live records), then the command journal and ore bind
(`:7761-7767`). The settlement is created *inside* `FUN_00465170`
(`:70641-70649`) via `FUN_00468ce0` (`:73100`), which takes the first free of
the eight pointers in `island+0xac`, allocates a 600-byte city record, stamps
the founding tile's slot bits into both map layers, and returns the slot — or 7
on failure.

`FUN_00481450` case 8 (`:92822-92836`) then allocates the live record through
`FUN_00481fc0` (`:93140-93206`): an open-addressed table `DAT_0054a3b8`, 0x14
bytes per entry, hash `((island & 3) * 0x20 + (x & 0x1f)) * 0x20 + (y & 0x1f)`
(`FUN_0047cc60`, `:89066`), 48 linear probes. On success it runs
(`:93194-93205`):

```
FUN_0047f450(city, def, flag)                      // city[0x1d8] += Kosten
if ((def[0x1c] == 7 || def[0x1c] == 8) && (def[0x6a] & 0x80) == 0)
    city[0x1fa] += 1                               // the storage-root count
city[0x216] += 1
city[0x218] += def[0x34]
```

So the Kontor owns a live record from the moment it is founded, and that record
is what makes `city+0x1fa` non-zero — which is what gives the city store any
capacity at all (§5).

**In the port**, `found_kontor` builds only `new_static` and never pushes to
`source_map_cell_states`. `SourceMapCellState::new` already gates correctly
(`allocates_source_scheduler_record`, `1..=8 | 30`; `KONTOR` is 8) — the port
simply never asks. Two consumers therefore see nothing: the transfer-root count
(so city capacity is under-counted) and the type-11 `city_origins` filter, which
is the only producer of city carts — **a founded colony dispatches no Karren
until a Marktplatz is built through `place_building`**. Scenario-loaded games are
unaffected because the load path builds the live table correctly, so this bites
only the player's own new colonies.

Two further divergences found in `found_kontor` while checking: it derives the
settlement slot by scanning `active_records()` and `.unwrap_or(0)`, which
silently claims slot 0 if the city pool is full where the original returns 7 and
skips both claim and stamp; and it ignores orientation entirely (`place_building`
has the same latent bug in its `set_walkable` extents).

**Fix.** Extract the shared tail of `place_building` — owner-slot resolution,
command, both cell states, claim, coverage rescan — into one
`install_source_building_records` and call it from both. Use the slot
`allocate_source_city` actually returned, and fail the command when it returns
`None`. Risk LOW: ~15 lines, no new state, no save change, no `rand()`.

## 2. The type-8 carrier search applies no radius bound

**Confirmed.** The bound is a rectangular window *and* a circular carve, both
from the `FUN_00404d70` raster — not a BFS depth limit.

`Radius` enters at `:90014-90019`, where production kind 1 dispatches
`FUN_0044ab60(island, x, y, def[0x20], ware, anim)`. The figure-8 event record
(`DAT_00505e38`, 0x2c stride) keeps Radius at `+0x00`, the root centre at
`+0x08/+0x0a`, the ware at `+0x2a` and **the requesting root's settlement slot**
at `+0x2b` (`:52008`).

The search `FUN_00459150` (`:61630`) computes

```
extra_x = (w - 1) & 1;  extra_y = (h - 1) & 1          // :61664-61671
left = x - radius;  top = y - radius
win_w = extra_x + 1 + radius * 2;  win_h = extra_y + 1 + radius * 2
```

then rasterises `FUN_004704d0`, opens its own footprint `FUN_004710b0`, carves
the disc `FUN_00471280`, and floods `FUN_00471380`.

Three bounding mechanisms, in order:

**(a) The window is the allocation and is absolute.** `FUN_004704d0` (`:79393`)
writes into the fixed scratch grid `DAT_005bb480`, 2 bytes per cell, laid out
`win_w × win_h`. It stamps `0x0c` (impassable) over every cell first
(`:79439-79446`), then fills only the clipped island intersection. Coordinates
outside the window do not exist — the wave cannot leave it.

**(b) The circular carve**, `FUN_00471280` (`:80102`), uses
`DAT_005b7460[radius]` — the same `FUN_00404d70` midpoint-circle table the
settlement claim (`:74459`) and the coverage scan use. Guarded on `radius > 1`,
so radius 0/1 leaves the raw rect.

**(c) Per-cell traversability** (`:79456-79482`) reads the **live** layer
`island+0xAF8`, and opens a cell only for outer map kinds
`{1, 0xb, 0xc, 0xd, 0x12, 0x1d, 0x1e}`, for kind 3 at the footprint centre gfx,
or when it is a goal — goal being
`def[0x1c] ∈ 1..=8 && ((word >> 19) & 7) == record[0x2b]`. Metadata is
`(def[0x58 + speedtyp] & 0x7f) | (goal << 7)`, i.e. the `Wegspeed` entry
selected by the figure definition's `+0x2c`.

**At the boundary**, cells outside the disc are impassable rather than merely
non-goal: a supplier inside the disc whose only route leaves it is unreachable,
and there is no step budget — the flood ends when the frontier empties. For the
Webstube (`Radius: 15`) that is a 31×31 window carved to a radius-15 disc, about
700 cells. The port clones the **entire island** grid per search, which is both
unbounded and far more expensive.

**A second divergence in the same code:** the original's goal test is the
settlement slot; the port matches `supplier.owner == building.owner`, the player
index. A player with two settlements on one island currently lets a workshop in
one draw from a producer in the other.

**Fix.** Thread `source_radius` and `source_map_owner_slot` from the requesting
root into `select_carrier_source_wave`, clip with the existing rect and radius
mask primitives in source order, add the Chebyshev pre-filter the type-11 path
already has, and switch the supplier match to settlement-slot equality. Risk
MEDIUM — it *removes* supply links that currently work, so re-baseline the
economy tests in the same commit.

## 3. Carrier path templates — the framing was wrong

**The original keeps no path cache at all.** `FUN_004704d0` / `FUN_004706e0`
rasterise a fresh `win_w × win_h` scratch grid from the live map layer on every
search (`:79409`, `:79519`). The map layer is the sole source of truth, written
in place by the placement path and cleared by demolition. There is no
walkability grid, no Wegspeed grid, and no dirty flag governing path data —
`DAT_00633740` is a *renderer* flag. Movement cost is not a grid either: the
traced route is compressed by `FUN_00473740` to one byte per step, high nibble
direction, low nibble cost, and read back at `:61619-61621`. The per-cell
`Wegspeed` is baked into the route at search time.

Because the window is at most ~33×33, "rebuild every search" is *cheaper* than
the port's "clone the whole island grid every search".

**In the port**, `IslandMap::from_island` builds eight-odd grids once per island
at load. Only `walkable` is maintained (`set_walkable`), and no source path
search reads it — the searches read `carrier_path_template` /
`city_cart_path_template`, which never move. So player roads give no speed
benefit and player buildings never block carrier routes, exactly as claimed.

**Fix, option A (mechanical).** Add `apply_source_footprint` /
`clear_source_footprint` on `IslandMap` writing exactly what the `from_island`
loop writes, and call them from `place_building`, `found_kontor` and
`demolish_building`. Removal must restore the *backing* definition (§4.2), which
is what `FUN_004641d0` does for a `Ruinenr = 0xff` command. Risk MEDIUM.

**Fix, option B (faithful).** Delete both templates; give `SourcePathGrid` a
constructor that rasterises a window directly from `source_map_kind_cells` +
`civilian_path_cells` (the port's stand-in for `island+0xAF8`), and build per
search. Drops ~6 grids, takes the per-search cost from O(island) to O(radius²),
and folds §2's clip in for free. Risk HIGH — multi-day refactor.

Either way the four `*_movement_speeds` grids should go: nothing in the source
reads a speed grid, and `advance_source_carrier` should take its step cost from
the route metadata.

Recommend A first to unblock §2 testing, B as the end state.

## 4. `Randwachs` is read from the wrong layer

**Confirmed; `docs/growth-timer.md` §6 is correct in every particular, and the
port already has the right data structure — it just isn't consulted.**

`FUN_004684a0` (`:72690-72709`) reads `island[0x2bf]`, byte offset **0xAFC** — a
third full-size layer. `FUN_00469100` (`:73378-73382`) allocates `0xAF4` and
`0xAF8` as one aliased block (the live layer) and `0xAFC` separately (the
backing/terrain layer).

On load (`FUN_00468550`, `:72780-72785`) each INSELHAUS command is copied into
the backing layer only when the cell is unowned, is not a ruin kind, and
`FUN_00480b70(def) == 0` (`:92166-92193`). That predicate copies plain terrain
and ore deposits but **not** forest, crops or pasture — so the backing cell
under a wood or a wheat field still holds the ground record. On save the backing
layer is the first of the two `INSELHAUS` chunks, every root stamped with slot 7;
the second chunk is the live layer filtered to cells that differ.

Decoding the shipped config gives exactly 39 `Randwachs:` occurrences: the
global template at 100, then 75 ×4, 50 ×8, 40 ×2 and 25 ×24 — all on `@Nummer`
41..80, the desert and arid ground records. Everything else inherits 100 → 128
through `ObjFill: 0,MAXHAUS`, which the port's parser already implements. So the
fix is safe: a naïve "read the backing cell" change will not collapse growth to
zero. Scaling is `authored * 128 / 100` truncating: 25→32, 40→51, 50→64, 75→96.

**Fix.** The port already builds `Simulation::source_static_map_backing_cells`
with a faithful port of the copy predicate. Add
`source_terrain_growth_factor(island, x, y)` reading it, and use that at the five
production sites in `simulation.rs` instead of `cell.source_resource_growth_factor`.
Return 128 on a miss and debug-assert. OPEN: whether any land cell in the shipped
scenarios lacks a backing entry. The backing vec is scanned linearly and the
harvest sites are hot — add a per-island index in the same commit. Risk LOW.

## 5. City-store capacity — the headline claim was wrong

`FUN_0047ab00` (`:87510-87516`) is
`(u16)city[0x1fa] * 0x140 - 0x140 + (i32)city[0x20]`, where `city+0x1fa` is the
storage-root count maintained incrementally (`+1` at `:93195`, `-1` at `:93221`,
both at `:92468` on settlement change, predicate
`(def[0x1c] == 7 || def[0x1c] == 8) && (def[0x6a] & 0x80) == 0`) and `city+0x20`
is set by `FUN_00481ee0` (`:93084`) to the compiled `Maxlager` (`def+0x30`,
stored `authored << 5`) on **every** KONTOR placement.

The port's `city_storage_capacity_fixed` computes
`default_capacity * 32 + (roots - 1) * 320`, which is algebraically identical.
**The formula is not the bug.**

The per-good claim is also inverted. `FUN_0047aa00` (`:87414`) computes free
space as `total - stock[ware]`, not `total - Σ stock`: there *is* a per-good cap
and it equals the whole city capacity, so goods do not compete for space. The
port's `deposit_city_good_fixed` already models this. A `0x20` floor applies
twice — free space below 32 reads as zero (`:87421-87423`) and stock below 32
reads as zero to withdrawal (`:87442-87444`). `FUN_0047aac0` returns the
*remainder*; the port returns the accepted amount, which is equivalent.

**The actual defects:**

| | |
| --- | --- |
| a | `default_capacity` is fixed at construction. The source rewrites `city[0x20]` on every KONTOR placement, so a `KONTOR_2` (`Maxlager: 75`) or `KONTOR_3` (100) raises the base. The port has no path that updates it. |
| b | The root count omits the `(def[0x6a] & 0x80) == 0` test. |
| c | The root count scans `source_map_cell_states`, incomplete for founded Kontors — a symptom of §1, not independent. |
| d | `Warehouse::deposit` caps at `default_capacity`, not at city capacity, so every ship unload, free-trader exchange and salvage is under-capped once a second market exists. ~15 call sites. |
| e | No `0x20` floor on free space or available stock. |

**RESOLVED** (was open): `def+0x6a` bit 7 is **`Destroyflg`** — loader cascade
`:66892`, write `:66913-66915`, authored only on `RUINE` and `STRANDRUINE`
records in haeuser.cod. So the exclusion reads "a kind-7/8 root that is a ruin
is not a storage root", which is why `FUN_0047f510` (`:91061`) uses the same bit
to find burnt-out marketplaces to reap and `FUN_00479ca0` (`:86842`) uses it to
refuse to afflict a ruin. Native and pirate village markers were the wrong
guess. See `docs/hazard-block.md` §3.

Relevant to lockstep: `FUN_0047a960`/`FUN_0047a9b0` (`:87376-87406`) do not
write the store directly — they post a kind-0x12 message addressed to the city.
All city-store mutation is deferred to the message pump. INFERRED that it drains
within the same sub-step.

Risk MEDIUM. Land (a) and (e) now, (d) with re-baselining, (b) separately once
the data question is closed. Fix §1 first — (c) falls out of it.

## 6. The buildable-area gate

**Confirmed, and the cap of 7 turns out to be load-bearing for the slot-7
sentinel.**

`FUN_004084d0` resolves the target settlement (`:7518-7535`) via `FUN_0046aec0`,
collapsing both "unowned" and "someone else's" to slot 7 while keeping a
separate `foreign` flag. Then per tile (`:7612-7616`):

```
if (slot == 7) {
    if (((player_kind != 0 && player_kind != 0x0c)
         || FUN_0046b120(island, player) == 0)
        && FUN_0046b100(island) < 7)
        goto ACCEPT;
    buildable = 0;                              // REJECT
} else if (foreign == 0 && def[4] == 0x23) {
    buildable = 0;                              // no second HQ inside your own claim
}
```

`FUN_0046b100` (`:74722`) counts non-null `island[0xac + i*4]` over 8 slots;
`FUN_0046b120` (`:74747`) counts settlements belonging to the player (with
`p == 7` as an "any real player" sentinel). Player kind is
`DAT_005b7680[player * 0xa0]`: 0 human, 0x0c AI, 0x0b pirates, 0x0d trader,
0x0e natives.

**In words:** on unowned ground a build is accepted only if the builder is not a
human or AI player, *or* the builder has no settlement at all on this island —
and the island holds fewer than 7 settlements.

`FUN_0046aec0` (`:74558-74640`) decides "your settlement" by majority vote over
the oriented footprint's live tiles, short-circuiting to a foreign settlement the
moment one is seen. The port's `source_placement_settlement_slot` is already a
faithful port including that short-circuit.

MARKT extends the claim through `FUN_00465170` (`:70650-70702`): kind 7 claims
at its own `Radius`, kind 8 at `max(Radius, 8)`, both via `FUN_0046ac60`
(`:74435-74512`), which converts a cell only when its slot is still 7 and its map
kind is not `MEER`. The port's `claim_source_settlement_area` and
`apply_source_settlement_claim` are accurate ports of this — build on them.

**Why 7:** `FUN_00468ce0` scans 8 slots and returns 7 both as a valid allocation
and as its failure code, while slot 7 is simultaneously the "unowned" sentinel in
the map word. The `< 7` test is what keeps slot 7 unreachable. The port's
`allocate_source_city` scans `0..8`, so its `source_owner == 7` hazard exists
*only* because the cap is missing; adding the gate closes it.

Caveat: the replay/network apply path `FUN_00409150` (`:7874-7930`) calls
`FUN_00465170` + `FUN_00481450` **without** the gate — it trusts the command. So
do not gate the port's replay branch either, or saves and multiplayer desync.

**On rejection the original shows nothing** — `param_5` selects cost query /
preview / commit, a rejected tile is simply skipped, and the build cursor never
becomes valid. The build menu is separately greyed at `:40029-40045`.

**Fix.** A `PlaceOutcome::OutsideSettlement` variant, gated in `place_building`
right after the owner slot is resolved, plus the one-Kontor-per-settlement rule
from `:7662-7665`. Risk MEDIUM — it rejects placements the port currently
accepts, so AI build routines, fixtures and recorded replays that build outside a
claim will start failing; land it with the test updates.

## 7. Order

| item | risk | blast radius |
| --- | --- | --- |
| §1 Kontor live cell | LOW | founded colonies only — do first, §5(c) depends on it |
| §4 `Randwachs` layer | LOW | arid maps only; data already present |
| §5 store capacity | MEDIUM | all deposits; split (a)+(e) from (d), (b) blocked on the OPEN question |
| §2 type-8 radius | MEDIUM | all workshop supply; primitives exist |
| §6 buildable gate | MEDIUM | every placement; reuses the ported claim |
| §3 path grid | MEDIUM→HIGH | every path search; A unblocks §2, B is the end state |
