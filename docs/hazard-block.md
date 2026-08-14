# City hazard block (fire / plague / vagrant) — RE reference

Companion to `docs/growth-timer.md` and `docs/logistics-gaps.md`. Addresses are
VA in `extracted/1602.exe`; line numbers are `decompiled/1602_exe.c`. Claims are
VERIFIED against the disassembly unless marked INFERRED.

Nothing here is implemented. The Rust already models the *effects* of
`house.lifecycle_flags & 3` (`source_transfer_delta`) and of `city[0x1fe]`
(`growth_blocked`) — but nothing ever sets either.

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

`def[0x6a] & 0x80` is **`Destroyflg`** (loader `:66892` → `:66913-66915`;
authored only on `RUINE` / `STRANDRUINE` records). So this is the burnt-out
marketplace reaper: one ruined market removed per event cycle, at a random
stride-4 phase. **It draws one `rand()` whether or not it finds anything**,
which is what makes it mandatory for stream fidelity.

This also answers the open question in `docs/logistics-gaps.md` §5: `def+0x6a`
bit 7 is `Destroyflg`, and the same bit makes `FUN_00479ca0` refuse to afflict a
ruin (`:86842`).

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
| 6 | duration in phases (`0x14` = 20 plague, `0x19` = 25 fire) |
| 7 | island-local city slot |

Hash `FUN_0047a630` (`:87204`): `((island & 3) * 8 + (x & 7)) * 8 + (y & 7)`,
probe window `[h, min(h + 0x20, 0x120))` — 32 linear probes, clamp dead. This is
**much coarser** than the kind-13 hash: only the low 3 bits of each axis
participate, so a 64×64 island folds 8×8-fold. **Collisions are the norm.** When
the window is full, insert returns 0 and registers nothing — and it has already
removed the tile's previous entry, so an over-full bucket silently loses
afflictions.

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

`city[0x1fe]` is a **signed 16-bit count**, not a flag. The Rust models it as
`growth_blocked: bool`, which suffices for `source_transfer_delta` but not for
the plague gate (`== 0`) or the message threshold (`> 2`).

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

**Fire spread** (`:87042-87117`) draws up to 3: a `rand() & 0x7F < 0x13`
(19/128) gate, a uniform pick over the area scan, then a 3-bit roll into
`DAT_0049af08` keyed on `Holz >= Ziegel`. Radius is `(w + h - 1)/4 + 2` using
the signed-divide idiom, over building kinds 1-7, `0x0d`, `0x0e`.

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

## 5. The igniters

All three scan that island's slice of the kind-13 table, bounded by
`source_index(island,0,0) .. min(source_index(island,0xff,0xff) + 0x40, 0x1040)`
— exactly the Rust's `SourceKind13LocationTable::source_index`.

| | filter | attempts | roll | RNG |
| --- | --- | --- | --- | --- |
| `FUN_0047b850` plague | un-afflicted, tier ≥ 2, matching city slot | 4 | occupancy ≥ half, then `rand() & 0xF` into `DAT_0049aed8` | 1-8, or 0 if no candidates |
| `FUN_0047b540` fire | un-afflicted, **tier ≤ 1**, matching city slot | 3 | `rand() & 7` into `DAT_0049af08`, row `Holz >= Ziegel` | exactly 2, 4 or 6 |
| `FUN_0047b710` vagrant | un-afflicted, tier ≥ 2, matching city slot | 3 | `rand() & 7` into `DAT_0049af18`, row = gallows bit | 2, 4 or 6 |

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
`rand() & 7`.

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

## 9. Staging

**Stage 1 — required for RNG fidelity on a ~100-inhabitant pioneer city.** At
`pioneers ≈ 100` the fire branch is **live**: `mod = 200`, threshold 31, i.e.
**15.5 % per cycle — a fire roughly every 7 minutes per city**. Needed: the
affliction table and registrar, `FUN_0047b540`, `FUN_0047f510`, the fire branch,
`FUN_0047a020` type 2, the ruin conversion, `DAT_0049af08`, and the area-scan
family. Plague and vagrant are unreachable (they need 200 / 300 citizens).

Boundary worth checking before assuming: at **79 or fewer** pioneers with fewer
than 250 settlers, the entire fire section draws **zero** `rand()` and only the
two unconditional draws remain.

**Stage 2 — 200+ residents.** Plague: `FUN_0047b850`, `FUN_0047a020` type 1,
`DAT_0049aed8`, `DAT_005a7758`, the capacity table, the doctor/bathhouse bits,
and `city[0x1fe]` as a real counter.

**Stage 3 — 300+ / cosmetic.** The vagrant spawn, `DAT_0049af18`, the gallows
bit, and the two message emitters. Only the two `rand()` calls inside
`FUN_0047b710` affect the stream.
