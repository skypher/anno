# City hazard block (fire / plague / vagrant) — RE reference

Companion to `docs/growth-timer.md` and `docs/logistics-gaps.md`. Addresses are
VA in `extracted/1602.exe`; line numbers are `decompiled/1602_exe.c`. Claims are
VERIFIED against the disassembly unless marked INFERRED.

**Status.** §9 Stage 1 and Stage 2 are implemented (`data_bridge.rs`,
`simulation.rs`, `source_cell.rs`): the affliction table and registrar,
`FUN_0047b540`, `FUN_0047f510`, the fire and plague branches of the event
block, `FUN_0047a020` types 1 and 2, the deferred type-7 ruin conversion,
`DAT_0049af08`, `DAT_0049aed8`, `DAT_005a7758`, and both rasterisers of the
`FUN_004722f0` area scan. Stage 3 (vagrant, message emitters) is not, and its
`rand()` draws are **not** made — see the deviations noted in §9.

**Corrections found while implementing** are marked `[FIX]` below; those found
during Stage 2 specifically are marked `[FIX2]`.

## 0. Corrections to earlier notes

| earlier claim | verdict |
| --- | --- |
| block at `:91455-91517` | **Partly wrong.** The gate `if (city[0x1e0] <= now)` opens at `:91441` / `0x0047fcc4` and closes at `:91519`. `0x47fd8f` is only where the *plague roll* starts. |
| "one `rand()` per city per event cycle" | **Incomplete.** `FUN_0047f510` at `:91442` draws one unconditionally too, and can demolish a building. Floor is **2** draws, not 1. |
| re-arm = 600-670 ms | **Units wrong.** `DAT_005b6040` counts 100 ms ticks (`:97970-97973`) — the same fact the gold-slope work established. The re-arm is 600-670 deciseconds = **60.0-67.0 s** of scaled sim time. Likewise `17999` = 30 min, `0x2a2f` = 18 min. |
| `FUN_0046a8c0` demolishes the building | **Wrong.** It *posts a deferred action of type 7* (`:74160-74163`). Execution is `FUN_0046a630` case 7 → `FUN_00463f40` (`:69706`), the ruin conversion, which draws **1 `rand()` per footprint cell**. Different dispatch slot, different RNG position. |
| vagrant is purely decorative | Correct that it has no economic effect, but its probability row is keyed on **gallows coverage**, so it is not flavour-only. |

**Correction to `docs/growth-timer.md` §1**: that doc says the 32860-entry
growth table "is not saved". The `"TIMERS"` chunk indeed does not save it, but
`FUN_00485c20` (`:94867-94911`) saves the whole table as the **`"ROHWACHS2"`**
chunk, 8 bytes per live entry, island-filtered. Corrected in that file.

**Correction to `docs/rng-dispatch-order.md`**: it labels S9 "ambient/native
figure spawn + scatter". S9 is `FUN_0047a020`, the affliction tick.

## 1. Where the block sits

`FUN_0047f8a0` is dispatch slot **S3** (`FUN_00489670`, `:97979`), after
`FUN_0047daf0` (S2) and before `FUN_0047b9c0` (S4). It visits **two city slots
per call**; the city stride is 600 bytes.

Hazards run for the local human's cities and for AI cities in state `0xC` —
exactly the Rust's existing `source_city_satisfaction_allows`. The block sits
textually **between the demand cycle and the INFRA unlock sweep**, both already
ported.

City fields used (byte offsets into the 600-byte record):

| offset | meaning |
| --- | --- |
| `+0x19` | island-local city slot 0..7; matches map bits 19..21 |
| `+0x1e0` u32 | next hazard-event time, in 100 ms ticks |
| `+0x1e4` / `+0x1e8` u32 | last growth / shortage message time |
| `+0x1f8` i16 | last `FUN_0047b410` transfer delta |
| `+0x1fe` i16 | **active affliction count** |
| `+0x220..+0x230` | u32 population per BGruppe |

Population suffix sums are computed just above the block (`:91400-91403`):
`pop[4] = tier[4]`, `pop[i] = tier[i] + pop[i+1]`. So `pop[0]` is the total,
`pop[1]` settlers-and-above, `pop[2]` citizens-and-above.

## 2. The block, faithful pseudocode (`:91441-91518`)

```
if (city[0x1e0] > now_ticks) { /* nothing, no rand */ }
else {
    FUN_0047f510(city);                                  // ALWAYS 1 rand, §3

    // two message emitters, no rand
    if (city[0x1f8] > 0 && pop[2] >= 200 && now - city[0x1e4] >= 18000) { ...; city[0x1e4] = now }
    if ((city[0x257] & 1) && owner == local && now - city[0x1e8] >= 10800) { ...; city[0x1e8] = now }

    // PLAGUE
    if (pop[1] >= 200 && city[0x1fe] == 0) {
        mod = pop[2] >= 400 ? 180 : pop[2] >= 200 ? 250 : 400;
        if ((rand() % mod) < 13) FUN_0047b850(city);
    }

    // VAGRANT
    if (pop[2] >= 300) {
        mod = pop[2] >= 700 ? 180 : pop[2] >= 500 ? 250 : 400;
        if ((rand() % mod) < 151) FUN_0047b710(city);
    }

    // FIRE — strictly exclusive if/else chain
    pioneers = tier[0];
    if (pioneers >= 80) {
        mod = pioneers >= 150 ? 100 : pioneers >= 120 ? 150 : 200;
        if ((rand() % mod) < 31) FUN_0047b540(city);
    } else {
        s = tier[0] + tier[1];
        if (s >= 250) {
            mod = s >= 350 ? 100 : s >= 300 ? 180 : 250;
            if ((rand() % mod) < 16) FUN_0047b540(city);
        } else if (pop[1] >= 250) {
            if ((rand() & 0xFF) < 8) FUN_0047b540(city);
        }
        // else: NO rand at all
    }

    city[0x1e0] = now_ticks + (60 + (rand() & 7)) * 10;   // ALWAYS 1 rand
}
```

Bit-exactness notes:

- `rand()` returns `[0, 0x7FFF]`, so `%` on a non-negative dividend matches Rust
  `%`. The decompiler's `& 0x800000ff` style masks are the MSVC signed-`%` idiom
  and collapse to plain `& 0xFF` / `& 0xF` / `& 7`.
- `0x0047fde1` / `0x0047fe61` only *load* the `rand` thunk pointer; they are not
  calls.
- The plague **gate** reads `pop[1]` but the **band** is chosen from `pop[2]`.
- The fire chain is `if/else`: once `pioneers >= 80` the settler branches are
  never evaluated.
- `city[0x1e0]` is initialised to `now + 600` at city creation, so the first
  event fires 60 s after the city exists.

**RNG budget per event cycle:** `FUN_0047f510` 1 always; plague roll 1 if gated
in, plus 1-8 if it fires; vagrant roll 1 if gated in, plus 2/4/6 if it fires;
fire roll 1 if any sub-gate passes, plus 2/4/6 if it fires; re-arm 1 always.
Minimum **2**, typical pioneer city **3**, worst case 23.

## 3. `FUN_0047f510` — the unconditional draw (`:91032-91073`)

```
r = rand();                                        // ALWAYS
p = kind_1..7_table_base(island) + (r & 3) * 0x14
for (; p < end; p += 0x50)                         // stride FOUR records
    if (p[0] == island) {
        def = gfx_table[tile(p[1],p[2]) & 0x1FFF];
        if (def[0x1c] == 7 && (def[0x6a] & 0x80) != 0 && ((tile >> 19) & 7) == city[0x19]) {
            FUN_0046a8c0(island, p[1], p[2], 7);   // deferred demolish
            return;
        }
    }
```

`def[0x6a] & 0x80` is **`Destroyflg`** (loader `:66892` → `:66913-66915`).
So this is the burnt-out marketplace reaper: one destroyed market removed per
event cycle, at a random stride-4 phase. **It draws one `rand()` whether or not
it finds anything**, which is what makes it mandatory for stream fidelity.

`[FIX]` `Destroyflg` is **not** "authored only on `RUINE` / `STRANDRUINE`". The
shipped haeuser.cod authors it on 26 records: `RUINE`, `STRANDRUINE`, every
`HANG`/`HANGECK`/`HANGQUELL`, `STRAND`/`STRANDECKA`/`STRANDECKI`/`STRANDMUND`,
`FLUSS`/`FLUSSECK`, the `IDHAFEN+20` `HQ`, two native `GEBAEUDE` plantations —
and, decisively, **`Zerstörter Marktplatz`** (`Id: IDDIVERS+22`,
`Kind: GEBAEUDE`, nested `HAUS_PRODTYP Kind: MARKT`, `Destroyflg: 1`). That
last record is the *only* definition in the file satisfying both of
`FUN_0047f510`'s tests, so the reaper's identification is right — but it selects
a specific authored ruined-marketplace building, not "a ruin standing where a
market used to be".

This also answers the open question in `docs/logistics-gaps.md` §5: `def+0x6a`
bit 7 is `Destroyflg`, and the same bit makes `FUN_00479ca0` refuse to afflict
any tile carrying it (`:86842`) — which excludes ruins, but also river, beach,
slope and the destroyed marketplace itself.

## 4. The affliction table `DAT_005a5100`

Base `0x005A5100`, **8 bytes/entry, 0x120 = 288 entries**. `FUN_00478580`
(`:85690-85696`) resets byte 0 of every entry to `0xFF` = free.

| byte | meaning |
| --- | --- |
| 0 | island; `0xFF` free |
| 1, 2 | island-local x, y of the object root |
| 3 | bits 0..4 type (**1 plague, 2 fire**); bits 5..7 last-seen phase |
| 4 | zeroed, never read |
| 5 | elapsed phase counter |
| 6 | duration in phases — see below |
| 7 | island-local city slot |

`[FIX]` Byte 6 is **not** "20 plague / 25 fire". It is written by the caller,
and the two callers disagree: `FUN_0047b540` pushes `0x19` (25 phases) when it
*ignites* a fire, while both spread paths converge on `LAB_0047a56d`, which
pushes a literal `0x14` for plague **and** fire alike (verified in the
disassembly: `push 0x14` at `0x0047a3ad` and `0x0047a562`). So an ignited fire
burns for 25 phases and a fire that spread from one burns for 20.

`[FIX2]` `FUN_0047b850` — plague *ignition* — also pushes `0x14`
(`1602_exe.c:88374`). So `0x19` is unique to fire ignition: every plague,
however it started, lives exactly 20 phases.

Hash `FUN_0047a630` (`:87204`): `((island & 3) * 8 + (x & 7)) * 8 + (y & 7)`,
probe window `[h, min(h + 0x20, 0x120))` — 32 linear probes, clamp dead (the
hash tops out at 255 and `255 + 0x20 = 287 < 0x120`). This is **much coarser**
than the kind-13 hash: only the low 3 bits of each axis participate, so a 64×64
island folds 8×8-fold. **Collisions are the norm.** When the window is full,
insert returns 0 and registers nothing.

`[FIX]` "…and it has already removed the tile's previous entry, so an over-full
bucket silently loses afflictions" overstates it. `FUN_00479be0` (the removal
lookup) and `FUN_00479ca0` (the free-slot search) probe the **same** window from
the **same** home slot, so a tile that already held an entry always finds its own
just-freed slot: re-registering an existing affliction never fails, even in a
completely full bucket. What a full window loses is a *new* affliction on a tile
that had none — the roll is spent, the object state is not applied, and
`city[0x1fe]` does not move.

There is a parallel identical table at `DAT_005624a8` using the same hash; that
one is combat damage accumulation, not afflictions. Do not conflate them.

### Persistence — none

Exhaustive grep: `DAT_005a5100` appears at exactly four sites (reset, lookup,
registrar, processor). **No save or load chunk touches it.** Worse, the per-house
affliction bits are also dropped: `FUN_00485530` (`:94801-94861`) packs each
kind-13 record into 16 bytes for `SIEDLER`, taking flag bits `0x0004`…`0x0400` —
**bits 0 and 1, the affliction state, are not saved**.

So in the original, **saving and reloading cures every plague and extinguishes
every fire** (mechanism VERIFIED, user-visible effect INFERRED). A port may
persist it — strictly better — but must not assume the original's saves carry it.

### The registrar `FUN_00479ca0(island, x, y, type, duration)` (`:86810-86902`)

Removes any prior entry at the tile, finds a free slot in the probe window
(returns 0 if none), refuses if `def[0x6a] & 0x80` (ruin), then per nested kind:
1..7 sets the "burning" bit `record[0x0f] & 0x80` in the 0x14-byte table; `0x0d`
sets `lifecycle_flags = (flags & ~3) | (type & 3)` on the kind-13 record; `0x0e`
sets `record[0x11] & 0x20`. Then `city[0x1fe] += 1` and the entry is written.

Messages: plague posts only when the city has **more than two** simultaneous
afflictions; fire posts unconditionally to the local player.

`city[0x1fe]` is a **signed 16-bit count**, not a flag. The Rust used to model
it as `growth_blocked: bool`, which sufficed for `source_transfer_delta` but not
for the plague gate (`== 0`) or the message threshold (`> 2`); it is now
`SourceCityRecord::active_afflictions: i16` with a `growth_blocked()` accessor.

### The processor `FUN_0047a020(dt_ms)` — slot S9 (`:86964-87157`)

10 s phase clock; **8 entries per dispatcher call**, cursor wrapping the table.
A full sweep is 36 calls ≈ 7.2 s at 1× speed, under the 10 s phase, so every
live entry steps exactly once per phase. Lifetimes: plague 20 phases = 200 s,
fire 25 phases = 250 s of scaled sim time.

**Plague spread** (`:86998-87041`) draws up to 3 `rand()`: an occupancy gate
against the 129-byte ramp `DAT_005a7758` indexed by `amount * 128 / 2560`
(2560 = BGruppe-4 `Maxwohn << 6`), then a uniform pick over the area-scan
results, then a 4-bit roll into `DAT_0049aed8` keyed on doctor + bathhouse
coverage. Radius 4, houses only, target must be un-afflicted and at ≥ half
capacity.

`[FIX2]` The occupancy gate is read off the **infected** house, not the
target: `FUN_00479f70(*pbVar6, pbVar6[1], pbVar6[2])` resolves the entry's own
tile. The target's half-capacity test is a *separate*, later guard that costs
no draw. A missing kind-13 record on the infected tile short-circuits before
the first draw, so an affliction sitting on something that is not a residence
costs nothing at all.

`[FIX2]` `DAT_005a7758` is **not** authored data and is not in the executable's
initialised sections — `0x5a7758` lands past `.data`'s `SizeOfRawData`.
`FUN_00478470` (`:85782-85784`) builds it at startup from three
`FUN_00403370` linear 8.8 fixed-point segments: `(0, 0x19) → 0..0x19`,
`(0x19, 0x26) → 0x19..0x33`, `(0x26, 0x80) → 0x33..0x40`. The per-step
increment is a truncating divide, so the ramp is
`[0,1,2,…,25, 27,29,…,49, 51,51,…,64]` — slope 1, then 2, then a crawl. Since
the gate is `rand() & 0x7f < ramp[i]`, the ceiling is 64/128 = 50 % per phase
for a completely full aristocrat house, and a house at 1/16 capacity is at
6/128 ≈ 4.7 %.

**Fire spread** (`:87042-87117`) draws up to 3: a `rand() & 0x7F < 0x13`
(19/128) gate, a uniform pick over the area scan, then a 3-bit roll into
`DAT_0049af08` keyed on `Holz >= Ziegel`. Radius is `(w + h - 1)/4 + 2` using
the signed-divide idiom, off the burning root's **unrotated** `+0x10`/`+0x14`
size (the bounding box uses the *oriented* size instead, via `FUN_00463880`).

`[FIX]` "over building kinds 1-7, `0x0d`, `0x0e`" conflates two different
filters. Those are the **nested** kinds (`def+0x1c`) of the *post-selection*
switch, applied to the root the pick resolves to. The area scan itself has no
kind filter of that shape: `FUN_00472930` opens and reports **outer** map kinds
(`def+0x04`) `{3, 4, 5, 6, 7, 10, 0x0e, 0x24, 0x25}` — `TOR`, `MAUER`,
`MAUERSTRAND`, `TURM`, `TURMSTRAND`, `WALD`, `GEBAEUDE`, `HAFEN`, `WMUEHLE` —
and marks every one of them as a candidate, unconditionally. Residences are
outer `Kind: GEBAEUDE` with nested `Kind: WOHNUNG`, so they are in the set;
`STRASSE` (1) and `BODEN` (11) are not, and are not even traversable. Fire
therefore only ever walks between structures that physically touch, and never
crosses a street or a patch of grass. Contrast the plague's `FUN_004724d0`,
which opens `{1, 0x0b, 0x0c, 0x0d, 0x12, 0x1d, 0x1e}` for *travel* but reports
only cells matching the caller's kind bitmask and city slot.

`[FIX2]` Three details of `FUN_004724d0` (`:81101-81213`) the sentence above
leaves out, and all three change the reported set:

- A **candidate is opened whatever its outer kind**. The `switch` on `def+0x04`
  falls through `default:` to `if (bVar8) goto case_1`, so a residence — outer
  `GEBAEUDE`, not in the travel set — is both traversable and reported. Without
  this the plague could never leave the house it started in.
- Outer kind **3 (`TOR`) is conditionally traversable**: only when the live gfx
  index equals `(def[0x14] * def[0x10]) / 2 + def[0x84]`, the open-gate frame.
  Everything else in the travel set is unconditional.
- The candidate test is `(param_4 & 1 << (def[0x1c] & 0x1f)) != 0 && (ws[0x1c]
  == 7 || ws[0x1c] == ((map_word >> 0x13) & 7))`. `ws[0x1c]` is the **centre
  tile's** slot, written by `FUN_004722f0`, and `7` there is a wildcard.
- It masks the stored class (`class & 0x7f | candidate << 7`) where
  `FUN_00472930` writes `class | 0x80` unmasked. Immaterial for shipped data,
  where every class is `0x20`.

**Expiry** (`:87119-87147`): plague just heals (`flags &= 0xFFFC`), frees the
entry, **0 rand**, no demolition. Fire posts deferred action type 7 and
**deliberately does not free the entry** — so it re-posts every phase until the
removal lands (mechanism VERIFIED, "repeats until removal" INFERRED). Action 7
runs `FUN_00463f40` (ruin conversion, 1 `rand()` for a same-size footprint or
one per cell otherwise), then the full coverage rescan `FUN_00482120`.

### The area-scan family

`FUN_004722f0(ws, island, <unused>, x, y, radius)` (`:80996`) — `param_3` is a
dead parameter. The work struct carries the radius, an orientation-aware bbox
derived from `FUN_00463880`, the owner slot, a result cap and a result buffer of
stride 0x18. `FUN_004724d0` / `FUN_00472930` rasterise the bbox into the shared
2-byte scratch grid `DAT_005bb480`; `FUN_00471fb0` floods breadth-first from the
centre; `LAB_00472ad0` appends until the cap.

**This is the single largest implementation cost of the feature**, and the only
part whose result set is not trivially derivable. It must be exact because
`rand() % n` consumes the stream and `n` is the result count.

Mechanics, now that it is ported (`SourceHazardScanGrid` in `data_bridge.rs`):

- `FUN_004722f0` centres the box on `anchor + (oriented_size - 1) / 2` and sizes
  it `2r + 1 + ((oriented_size - 1) & 1)` per axis, so an even footprint gets an
  extra column/row. `param_3` is dead; the work struct's owner slot comes from
  the **centre tile's** map word, and `7` in that field is a wildcard.
- Byte 0 of each scratch cell is the traversal marker (`0` free, `0x0c` the
  blocked fill, `1..=8` the direction stamped on enqueue, `0x0b` the origin);
  byte 1 holds the path class in bits 0..6 and the "report me" flag in bit 7.
- `FUN_00471fb0` is a bucketed Dijkstra, bucket width `0x40`. The seed carries
  cost `0x40`; each round subtracts `0x40` from every queued entry and expands
  only those that reach zero or below, carrying the residue. Levels are walked
  **backwards** into a double buffer; the four diagonals are enqueued before the
  four orthogonals and each diagonal refuses to cut a corner.
- Step costs come from `FUN_0046f8a0` (`:78686-78717`): classes `0x00..=0x20`
  cost `(0x40, 0x5b)` orthogonal/diagonal, then two ramps run to class 126, and
  class `0x7f` is the stored `(0x1f8, 0x2cc)`. `param_3 == 3` selects
  `Wegspeed[3]`, and **every** authored `Wegspeed` quad ends in `100`, which
  compiles to class `0x20` — so in the shipped data the hazard flood is
  uniform-cost, orthogonal `0x40`, diagonal `0x5b`.
- `LAB_00472ad0` appends `(work.local_x + x, work.local_y + y)` at stride `0x18`
  and returns the struct's stored `1`, so a reported cell is always expanded;
  what it controls is the continue flag `ws+0x20`, which it clears the moment
  the 20-entry buffer fills, stopping the flood mid-round. The origin cell is
  never reported — the flood clears its candidate bit before starting.

## 5. The igniters

All three scan that island's slice of the kind-13 table, bounded by
`source_index(island,0,0) .. min(source_index(island,0xff,0xff) + 0x40, 0x1040)`
— exactly the Rust's `SourceKind13LocationTable::source_index`.

| | filter | attempts | roll | RNG |
| --- | --- | --- | --- | --- |
| `FUN_0047b850` plague | un-afflicted, tier ≥ 2, matching city slot | 4 | occupancy ≥ half, then `rand() & 0xF` into `DAT_0049aed8` | 2-8, or 0 if no candidates |
| `FUN_0047b540` fire | un-afflicted, **tier ≤ 1**, matching city slot | 3 | `rand() & 7` into `DAT_0049af08`, row `Holz >= Ziegel` | exactly 2, 4 or 6 |
| `FUN_0047b710` vagrant | un-afflicted, tier ≥ 2, matching city slot | 3 | `rand() & 7` into `DAT_0049af18`, row = gallows bit | 2, 4 or 6 |

`[FIX2]` The plague's "1-8" is off by one at the bottom. Its attempt loop
draws the uniform pick *first* and the table index only if the occupancy gate
passed, so an attempt costs 1 or 2 draws and the routine can only return early
after a 2-draw attempt. **1 and 3 are both reachable as intermediate sums but
1 is not a possible total**: the reachable budgets are `{0} ∪ {2..=8}`. A run
of four under-occupied picks is the 4-draw floor for a routine that never
consults the table at all. Pinned by
`source_plague_ignition_rand_budget_is_two_to_eight`.

The vagrant's `FUN_0044b620` allocates entity class `0x0f` with figure def
`0x5c` (`PASSANT`), which self-destructs past 49 shared civilian steps and draws
**no `rand()`** — so a stub that spawns nothing is stream-identical.

Candidate arrays sit on a `_chkstk` frame of ~1020 pointers while the slice
holds 1087 entries, with no bound check. Arithmetic VERIFIED; reachability
INFERRED and not worth reproducing.

## 6. The probability tables (real bytes)

```
DAT_0049aed8  3 rows x 16   (plague)
  row 0: 00 01 01 00 00 01 00 01 00 01 01 00 00 01 00 01   ->  8/16 = 50.00 %
  row 1: 00 00 00 01 00 01 00 00 00 00 00 01 00 01 00 01   ->  5/16 = 31.25 %
  row 2: 00 00 01 00 01 00 00 00 00 00 00 00 01 00 00 00   ->  3/16 = 18.75 %

DAT_0049af08  2 rows x 8    (fire)
  row 0: 01 01 01 01 00 01 01 01                           ->  7/8  = 87.5 %
  row 1: 00 00 01 00 01 00 00 01                           ->  3/8  = 37.5 %

DAT_0049af18  2 rows x 8    (vagrant)
  row 0: 01 01 01 01 01 01 01 01                           ->  8/8  = 100 %
  row 1: 00 00 01 00 00 00 00 01                           ->  2/8  = 25 %
```

The **positions** matter, not just the counts — the index is `rand() & 15` or
`rand() & 7`. All three verified byte-for-byte against `extracted/1602.exe`
(2026-08-15).

`[FIX2]` Re-verified independently while implementing Stage 2, by mapping
`0x0049aed8` through the PE section table to file offset `0x9aed8` and reading
48 bytes: the transcription above is **exactly right**, all three rows, all 48
positions. `DAT_0049af08` and `DAT_0049af18` follow it contiguously at
`0x9af08` and `0x9af18` and also match. The three tables live in `.data`
(RVA `0x98000`, raw at `0x98000`) but inside its initialised prefix, which is
why they read out of the file at all — unlike `DAT_005a7758`, which does not.

Row selection. Plague: `((flags >> 9) & 1) + ((flags >> 6) & 1)` = how many of
**doctor** (`KLINIK`, bit `0x200`) and **bathhouse** (`BADEHAUS`, bit `0x40`)
cover the house. Fire: `Holz >= Ziegel`, which from the shipped housing costs is
row 1 for BGruppe 0/1 and row 0 for 2/3/4 — and since `FUN_0047b540` only picks
tier 0/1, **ignition always uses row 1 (3/8)**; both rows are live in the spread
path. Vagrant: gallows coverage (bit `0x800`) — without a `GALGEN` the spawn is
certain, with one it drops to 25 %.

The coverage scan resets `flags &= 0xF003` before re-deriving, so bits 2..11 are
recomputed each pass while bits 0..1 and 12..15 survive — which makes `BRUNNEN`
coverage (bit 12) *sticky* once set. VERIFIED; likely an original bug, not worth
reproducing until proven observable.

## 7. Interaction with ported systems

`FUN_0047b410` — already ported as `source_transfer_delta` — takes two inputs
this feature supplies. Growth requires `(house[8] & 3) == 0` **and**
`city[0x1fe] == 0`, so any affliction anywhere in the city stops *all* growth
there. Decay subtracts `DAT_00562180[(house[8] & 3) * 2]` = `[0, 0x100, 0xC0]`,
already `lifecycle_penalty` in `data_bridge.rs`. Since S4 demolishes any kind-13
record whose resident count reaches 0 through the same deferred action, **a long
plague kills houses indirectly on the growth path**, not on the affliction path.

Cures are `FUN_0047b2c0` / `FUN_0047b320`, driven by the `ARZT` and `LOESCH`
figure state machines.

A fire's demolition reuses the shared removal path, which already maintains the
population ledger, the promotion reservation, the kind-13 slot, the affliction
entry, `city[0x1fe]` and the coverage-dirty flag — so a port that reuses its
existing kind-13 removal gets all of that for free.

## 8. Rust gap list

New `Simulation` state: a `SourceAfflictionTable` mirroring
`SourceKind13LocationTable` (probe 0x20, 288 slots, the 3-bit-per-axis hash),
plus the 10 s phase clock and the sweep cursor. **Unlike the growth timer the
probe policy must be modelled literally** — with this hash, window-full is a
normal outcome, not an edge case.

The one breaking change: `SourceCityRecord::growth_blocked: bool` becomes
`active_afflictions: i16`, with a `growth_blocked()` accessor so
`SourceKind13TransferInputs` is untouched.

`BuildingDef` must expose `Holz` and `Ziegel` on **every** definition, not just
the five housing records — fire spread reads them off arbitrary targets. The
comparison is order-preserving whether or not the `<< 5` scaling is applied.

`DAT_0061fa4c = [128, 384, 960, 1600, 2560]` should be derived from
`Maxwohn << 6`; `SOURCE_KIND13_AMOUNT_CAPACITIES` already holds this, and
`DAT_0061fb6c` is simply its index 4.

Hooks: the block inside `tick_source_city_dispatch` between the demand cycle and
the unlock sweep; a new `tick_source_afflictions` at the S9 position. Bump
`SAVE_VERSION`; all new fields `#[serde(default)]`, which reproduces the
original's save behaviour anyway.

**As built (Stage 1).** `data_bridge.rs`: `SourceAfflictionTable` /
`SourceAfflictionEntry`, `SOURCE_FIRE_PROBABILITY_TABLE`,
`SourceHazardScanGrid` + `source_path_step_costs` +
`source_hazard_scan_kind_is_burnable`, and `SourceCityRecord::
active_afflictions` with a `growth_blocked()` accessor.
`source_cell.rs`: `source_destroy_flag`, `source_wood_cost_fixed`,
`source_bricks_cost_fixed`, `source_path_class_loaded_road` on every map cell.
`simulation.rs`: `tick_source_city_hazard_event`,
`source_reap_destroyed_marketplace`, `source_ignite_fire`,
`source_register_affliction` / `source_remove_affliction_at`,
`tick_source_afflictions` → `source_spread_fire` / `source_expire_fire`,
`source_hazard_area_scan`, and `tick_source_deferred_map_demolitions` (drained
at the S6 position, which is what puts the ruin conversion's draws one slice
after the sweep that posted them).

`SAVE_VERSION` 141 → 142, and `MIN_LOADABLE_VERSION` with it: bincode is not
self-describing, so the `bool` → `i16` widening and the four appended map-cell
fields cannot be read out of an older payload — `#[serde(default)]` does not
help there.

**As built (Stage 2).** `data_bridge.rs`: `SOURCE_PLAGUE_PROBABILITY_TABLE`,
`source_plague_probability_row`, `SOURCE_PLAGUE_OCCUPANCY_RAMP` (rebuilt from
`FUN_00403370` in a `const fn`, since the source array is uninitialised on
disk), `source_plague_occupancy_index`,
`source_hazard_scan_kind_is_walkable`, `SOURCE_PLAGUE_SCAN_NESTED_KIND_MASK`,
`SOURCE_PLAGUE_SCAN_RADIUS`, and `SourceHazardScanGrid::set_open` next to the
existing `set_burnable`. `simulation.rs`: the plague roll inside
`tick_source_city_hazard_event`, `source_ignite_plague`,
`source_plague_occupancy_admits`, `source_spread_plague`,
`source_expire_plague`, and a `SourceHazardScanMode` parameter on
`source_hazard_area_scan` selecting `FUN_00472930` or `FUN_004724d0`.

No new persisted state, so **`SAVE_VERSION` is untouched** — the plague reuses
the affliction table and the kind-13 lifecycle bits Stage 1 already saved.

Pinned by `source_plague_probability_table_rows_are_pinned`,
`source_plague_occupancy_ramp_matches_the_startup_segments`,
`source_city_hazard_plague_gate_rand_budget`,
`source_plague_band_is_chosen_from_citizens_not_settlers`,
`source_plague_ignition_rand_budget_is_two_to_eight`,
`source_plague_spreads_to_a_neighbour_and_then_heals_it` and
`source_plague_area_scan_reports_only_same_city_residences` in
`simulation.rs`, plus the corpus test
`crates/anno-game/tests/source_plague.rs`.

## 9. Staging

**Stage 1 — required for RNG fidelity on a ~100-inhabitant pioneer city.** At
`pioneers ≈ 100` the fire branch is **live**: `mod = 200`, threshold 31, i.e.
**15.5 % per cycle — a fire roughly every 7 minutes per city**. Needed: the
affliction table and registrar, `FUN_0047b540`, `FUN_0047f510`, the fire branch,
`FUN_0047a020` type 2, the ruin conversion, `DAT_0049af08`, and the area-scan
family. Plague and vagrant are unreachable (they need 200 / 300 citizens).

Boundary worth checking before assuming: at **79 or fewer** pioneers with fewer
than 250 settlers, the entire fire section draws **zero** `rand()` and only the
two unconditional draws remain. Confirmed, and pinned by
`source_city_hazard_fire_gate_rand_budget` in `simulation.rs`.

**Stage 1 deviations, deliberate and documented in the code:**

- ~~The plague roll (`pop[1] >= 200 && city[0x1fe] == 0`) and the vagrant roll
  (`pop[2] >= 300`) are omitted **including their `rand()` draws**.~~ The
  plague roll landed in Stage 2; only the vagrant roll (`pop[2] >= 300`) is
  still omitted with its draws.
- The two message emitters draw nothing and are presentation-only.
- The kind-`0x0e` object-state branch of the registrar (`record[0x11] & 0x20`)
  has no record in this port.
- The kind-1..7 table `DAT_0054a3b8` has no insertion hook here, so
  `FUN_0047f510`'s physical scan order is replayed from `source_map_cell_states`
  in command order rather than maintained incrementally. Two consequences: a
  slot freed by a demolition is reused by the replay but was not by the
  original, and the source also keeps service buildings (nested kinds
  `0x11..=0x1b`) in that table while this port allocates them no record.
- The port **persists** the affliction table and the kind-13 affliction bits.
  The original persists neither (§4), so an original save/reload cures
  everything; this is a deliberate improvement, not a reproduction.
- `source_cities_from_scenario` does not read `+0x1e0` out of `STADT4`, so a
  scenario-loaded city keeps `ready_at_ticks == 0` and opens its first event on
  the very first dispatcher visit rather than 60 s in. Cities created at
  runtime through `allocate_source_city` do get the `now + 600` arming
  `FUN_00468e10` applies. Pre-existing gap in the field extraction, now
  observable; worth closing when `STADT4` parsing is revisited.

**Stage 2 — 200+ residents. Implemented.** Plague: `FUN_0047b850`,
`FUN_0047a020` type 1, `DAT_0049aed8`, `DAT_005a7758`, the capacity table, the
doctor/bathhouse bits, and `city[0x1fe]` as a real counter.

At `pop[2] ≈ 240` — mind that this is the *band* input, not the gate's
`pop[1]` — the roll is `mod = 250`, threshold 13, i.e. **5.2 % per cycle**,
rising to 7.2 % once citizens-and-above passes 400. Roughly 94 % of the rolls
that pass then land, since four attempts at 8/16 miss only `(1/2)^4` of the
time on an uncovered house. So a fed citizen colony sees a plague roughly
every twenty minutes, and each one stops *all* growth in that city for 200 s.

**Stage 2 deviations, deliberate and documented in the code:**

- Outer kind 3 (`TOR`) is treated as blocked rather than modelling
  `FUN_004724d0`'s open-gate gfx test, because this port carries no live gfx
  index per cell. A plague will not walk through a city gate; a shipped
  colony rarely has one inside a radius-4 box, but this does shift the
  reported count — and therefore `rand() % n` — where it does.
- The candidate city-slot test reads `SourceKind13Location::source_owner` and
  the map cell's `source_map_owner_slot`, both snapshots of the map word taken
  at placement, where the source re-reads the live word. Stage 1's igniter
  already made the same trade.
- The two message emitters still draw nothing. `city[0x1fe] > 2` is now
  expressible, so the plague's "more than two simultaneous afflictions"
  threshold could be wired up whenever presentation is.

The remaining divergence point is **Stage 3's vagrant roll at `pop[2] >= 300`**
and its 2/4/6 spawn draws.

**Stage 3 — 300+ / cosmetic.** The vagrant spawn, `DAT_0049af18`, the gallows
bit, and the two message emitters. Only the two `rand()` calls inside
`FUN_0047b710` affect the stream.
