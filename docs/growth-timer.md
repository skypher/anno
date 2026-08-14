# Raw-resource maturation and regrowth — RE reference

Everything a port of the crop / forest / pasture growth timer needs.
Addresses are VA in `extracted/1602.exe`; line numbers are
`decompiled/1602_exe.c`. Claims are VERIFIED against the disassembly
unless marked INFERRED.

Nothing described here is implemented yet. After
`replace_harvested_raw_resource` fires, a cell stays a growing
(`ROHSTWACHS`) tile forever: nothing re-enrols it and nothing steps it.

## 1. The timer table — `DAT_00562dc8`

It is an **open-addressed hash table**, not a ring; only the sweep
*cursor* rings. Base `0x562dc8`, 8 bytes per entry, `0x805C` = 32860
entries, end `0x5A30A8`. `FUN_00478580` (`:85662-85668`) resets byte 0 of
every entry to `0xFF`, which marks a free slot.

| byte | meaning |
| --- | --- |
| 0 | island index; `0xFF` = free |
| 1 | local x |
| 2 | local y |
| 3 | bucket index 0..=31 |
| 4 | snapshot of `DAT_00562da8[bucket]` at last visit |
| 5 | phase counter, seeded from tile bits 15..18 |
| 6, 7 | zeroed on insert, never read |

Hash (`FUN_0047c810`, `:88863`):

```
h = ((island & 7) * 0x40 + (x & 0x3f)) * 0x40 + (y & 0x3f)   // 0..0x7FFF
```

Probe window `[h, min(h + 0x5C, 0x805C))` — up to 92 linear probes. The
clamp is dead code (`0x7FFF + 0x5C = 0x805B`); the 0x5C tail entries
exist purely as probe overflow. Insert (`FUN_0047c760`, `:88815`) takes
the first free slot in the window and **silently does nothing if the
window is full**. Lookup is `FUN_00481f50` (`:93111`).

Entries are removed **only** by `FUN_0047ca80` on phase completion and by
the global reset. There is no removal on demolition — see §7.

Two parallel 32-entry bucket clocks: `DAT_0054a2f4` (u32 accumulators)
and `DAT_00562da8` (u8 counters, 3-bit). Bucket *i* fires every
`40000 + 7000*i` ms, i.e. 40 000 … 257 000. The sweep cursor
(`PTR_DAT_0049aec0`) visits **206 entries per call**, so a full sweep
takes 160 calls.

**Persistence.** The `"TIMERS"` chunk (`:94964-94984`, 0x298 bytes)
saves `DAT_00562da8` and `DAT_0054a2f4` but **not the table**, which is
rebuilt on load by `FUN_00481450` case 10 while replaying INSELHAUS
commands; each phase counter is re-derived from the tile's bits 15..18,
which are saved in the command word's bits 2..5 (`FUN_004631b0`,
`:69010`).

## 2. Arming, and where the bucket comes from

`FUN_0047c760(island, x, y, bucket)` (`:88815`) clamps `bucket` to 31,
fills the entry, and seeds byte 5 from the tile's current phase bits. It
draws **zero `rand()`** — the whole disassembly `0x47c760-0x47c80b`
contains no call.

The bucket is **not** the authored `Interval`. `FUN_00462d50`
(`:68880-68889`) rewrites definition `+0x3a` in place at load time for
every production-kind-10 record:

```
per_phase_ms = (u32)Interval * 1000 / AnimAnz          // AnimAnz = def+0x78
bucket = largest k with 40000 + 7000k <= per_phase_ms, saturating at 32
```

So growth rates **do** differ per crop; an earlier reading that the
clamp collapses everything into one bucket was wrong.

| record | Interval | AnimAnz | per phase | bucket | period | total ripen |
| --- | --- | --- | --- | --- | --- | --- |
| Weizen | 175 | 5 | 35 000 | 0 | 40 000 | 200 000 |
| Baumwolle, Tabak, Kakao, Gewürze, Weintrauben | 330 | 5 | 66 000 | 3 | 61 000 | 305 000 |
| Zuckerrohr | 300 | 5 | 60 000 | 2 | 54 000 | 270 000 |
| Wald / Palmen | 900 | 5 | 180 000 | 20 | 180 000 | 900 000 |
| Meereszeichen | 450 | 6 | 75 000 | 5 | 75 000 | 450 000 |
| Weideland `IDWEIDE` | 300 | 1 | 300 000 | 32→31 | 257 000 | 257 000 |
| Weideland `IDBODEN` | 200 | 1 | 200 000 | 22 | 194 000 | 194 000 |

**Jitter.** Only `FUN_00481450` case 10 (`:92840`) adds any, arming with
`def[0x3a] + param_7 % 3`. On the build-command and save-replay paths
`param_7` is the command word's bits 17..21, which `FUN_004631b0` filled
with `rand() & 0x1F` — **one `rand()` per placed command, at command
construction, and persisted in the save**. That is the only RNG anywhere
in the growth pipeline. Harvest (`FUN_0047c830`) and the drought sweep
(`FUN_0047c920`) arm with the plain `def[0x3a]`, no jitter.

## 3. Stepping — `FUN_0047ca80(dt_ms)` (`:88972`)

Called from `FUN_00489670` (`:97975`) **first** in the sub-step, with the
same ≤200 ms scaled slice the already-ported `FUN_0047f8a0` receives.

Phase A advances the 32 bucket clocks: `acc[i] += dt`, and on reaching
`40000 + 7000*i` resets the accumulator and bumps `counter[i]` mod 8.

Phase B visits 206 entries from the cursor. For an occupied entry whose
bucket counter has moved since byte 4:

```
e[4] = counter[e[3]]
def  = gfx_table[ tile & 0x1FFF ]              // DAT_00619b60, indexed by Gfx
if def[0x1c] == 10 {                           // ROHSTWACHS — phase only advances here
    e[5] += 1
    if (u32)e[5] >= def[0x78] {                // AnimAnz
        if (def[0x47] & 8) == 0 {              // not Doerrflg
            tile = (tile & ~0x1FFF) | (def[0x10c] & 0x1FFF)   // next record's Gfx
        }
        e[0] = 0xFF                            // free the slot, unconditionally
        continue
    }
}
if def[0x70] == 0 {                            // AnimTime == TIMENEVER == 0
    tile = (tile & 0xFFF87FFF) | ((e[5] & 0xF) << 15)
}
```

- `def+0x70` is `AnimTime` and `TIMENEVER` is registered as **0**
  (`:44507`, `:66370`), so `AnimTime == 0` is the condition for "bits
  15..18 are free to hold the growth phase". The sea resource
  (`AnimTime: 130`) uses those bits for its own animation and never gets
  phase writes — it still counts to `AnimAnz` and promotes.
- `def+0x78` is `AnimAnz` (`:68622`).
- When `def[0x1c] != 10` the phase counter is **not** incremented
  (`0x47cb5f` jumps past the `inc`).
- Tile word: bits 0..12 Gfx, 13..14 orientation, **15..18 phase**,
  19..21 map owner, 26 pending-clear, **29 plantation-worker
  reservation**.
- Layers `+0xAF4` / `+0xAF8` alias one allocation except for the selected
  island, where `+0xAF8` is a working copy. The growth code writes both
  consistently, so **one layer is faithful for the port**.

## 4. The record triple

Compiled records are 0x88 bytes in `@Nummer` order, and **the ±0x88
record stride is the authoritative link**. `Gfx` (`def+0x84`) is derived:
the engine steps to the neighbouring record and reads *that* record's
`Gfx`. Adjacent records' `Gfx` values are **not** adjacent — for Weizen
the growing record is `GFXROHST+0`, ripe is `+5` (the growing record
consumes `AnimAnz` = 5 sprite slots) and withered is `+48`. **Any port
that does `gfx ± 1` is wrong.**

Only the 7 crops are triples `[HAUSWACHS, HAUSFERT, DOERR]`; pasture,
forest, palms and sea resources are **pairs** with no withered record —
haeuser.cod contains exactly 7 `Doerrflg: 1` records. `Doerrflg` is
`def+0x47` bit 3 and marks the third record, not a runtime flag.

`FUN_00468bf0` (`:73020`) corroborates the pairing: the save comparator
normalises both tiles by `if (kind == 10) def += 0x88` before comparing.

The `+0x21` **Ware** byte lives on the ripe record only; growing and
withered records reach it as `def+0xa9` or `def-0x67`.

`RandAnz` / `RandAdd` do **not** interact. Variant selection
(`base + (rand() % RandAnz) * RandAdd * 0x88`) happens only in the
planting and ruin paths (`:88741`, `:69745`), never in the growth timer.
Forest is `RandAnz: 11, RandAdd: 2`; the ±0x88 step stays inside the
selected variant's pair.

## 5. Regrowth after harvest — `FUN_0047c830` (`:88874`)

Called from the type-12 worker state machines with the worker's carried
ware:

```
def = gfx_table[tile & 0x1FFF]
if def[0x1c] == 9 && def[0x21] == ware {        // ripe, matching resource
    target = def - 0x88                         // growing record
    if island[0x45] != 0 && FUN_004684a0(island, ware, x, y) == 0
        target = def + 0x88                     // withered
    tile = (tile & ~0x1FFF) | (target[0x84] & 0x1FFF)
    if target[0x70] == 0 { tile &= 0xFFF87FFF }  // phase bits <- 0
    FUN_0047c760(island, x, y, target[0x3a])     // re-enrol, no jitter
}
tile &= 0xDFFFFFFF                               // clear reservation bit 29, ALWAYS
```

So the full cycle is: harvest → growing record at phase 0 → enrolled at
that record's bucket → `AnimAnz` rollovers later the sweep writes the
ripe record's Gfx and frees the slot. Nothing else participates.

The drought byte is `INSEL5[0x66]` → `island+0x45`, and it is **0 on
every New Horizons0 island**, so the withering paths are inactive for the
first mission. When non-zero, `FUN_0047c920` (`:88916`) runs a periodic
wither/recover sweep reached from `FUN_0046b3e0` (`:75045-75095`), which
draws **one `rand()` per row** for its column start offset.

Source hazard, reachable only on drought islands: the wither step is
`ripe + 0x88` unconditionally, so for pasture and forest — which have no
withered record — it lands on the *next variant's growing record*.

## 6. Growth factor comes from the terrain layer

`FUN_004684a0(island, ware, x, y)` (`:72690`):

```
s = FUN_0046aff0(island, ware)                              // 0, 0x40 or 0x80
s = s * def_of(island[0x2bf][y*width + x])[0x40] / 128      // Randwachs, LAYER 0x2bf
if island[0x45] != 0 && ware not in {0x34,0x35,0x39}
    s -= (island[0x45] * s) / 128
band = DAT_0054697c[s]
col  = ((x & 3) + y*4 + island_index + ware) & 31
return DAT_00499b98[band*32 + col]
```

`Randwachs` is read from `param_1[0x2bf]` — the **base terrain layer**, a
third full-size grid allocated in `FUN_00469100` (`:73382`), populated
only at island load and saved as the first of the two `"INSELHAUS"`
chunks. This is semantically right because `Randwachs` is authored only
on *terrain* records: the `ObjFill: 0,MAXHAUS` template sets
`Randwachs: 100` for every slot, and only the desert markers override it
(25 / 40 / 50 / 75). No crop, forest or pasture record authors it.

**Consequence for the Rust:** `simulation.rs` reads
`cell.source_resource_growth_factor` from the resource cell's own
definition. On ordinary ground both give 128, so it is right by accident;
**on desert terrain it is wrong** — should be 25-75-derived, gets 128.

The dither table `DAT_00499b98` (5 rows × 32, with 2/10/16/23/32 set
bits) is byte-identical to `SOURCE_RESOURCE_GROWTH_MASKS` in
`source_cell.rs`; the band table and growth-strength function
`FUN_0046aff0` are already correct in the port.

## 7. Scope

**Initial ripening uses the same timer.** `FUN_00481450` case 10
(`:92837-92842`) arms any newly placed production-kind-10 tile, including
a player-placed plantation field. The only difference from regrowth is
the `param_7 % 3` jitter.

**Placement has its own growth check** (`:7755-7759`, unported): before
writing the tile, if `def[0x1c] == 10` and
`FUN_004684a0(island, def[0xa9], x, y) == 0`, the placed record advances
by **+0x110 (two records)** — the field is planted already withered. This
runs regardless of the drought byte, so it is live on New Horizons0
wherever the dither band says no.

**Natural terrain is not enrolled at load.** Island forest, pasture and
ore ship in the ripe (kind 9) state and only kind-10 tiles get a timer;
growth starts when a worker harvests.

**Entries are never removed on demolition.** A stale entry whose tile is
no longer kind-10 writes its stale counter into that tile's phase bits on
every bucket rollover, and the slot leaks until a kind-10 definition
reappears at those coordinates. (Removal absence VERIFIED; the visual
consequence INFERRED.) Probably not worth reproducing, but a port that
deregisters on demolition will diverge there.

## 8. Pre-existing defects this sits on top of

1. **The harvest transition never fires with real data.**
   `replace_harvested_raw_resource` guards on `self.kind_code != 9` and
   `advance_raw_resource_to_drought` likewise, but the source tests the
   **nested** production kind `def+0x1c` (`:88888`, `:75056`, `:75084`).
   With shipped data a ripe crop has outer `Kind: WALD` → `kind_code 10`
   and nested `ROHSTOFF` → `source_production_kind_code 9`; ripe pasture
   has outer `BODEN` → 11. Only the synthetic `kind: "ROHSTOFF"` test
   fixture matches. This is the same outer-vs-nested confusion fixed in
   `c778115`, and it means harvested cells never revert — so island
   forest is currently an infinite resource.
2. **`source_definition_offset ± 1`** is a `Gfx` index, but the source
   moves a whole record and reads that record's `Gfx`. See §4.
3. **`source_resource_harvest_transition`'s `island_attenuation == 0 →
   Regrowth` early return** belongs to `FUN_0047c830`'s gate, not to
   `FUN_004684a0` itself, which is also called from the placement path
   where the dither can legitimately return 0.
4. **Growth factor read from the wrong layer** — §6.

## 9. Where it lands in the port

New `Simulation` state, both of which the original saves in `"TIMERS"`:
`source_growth_bucket_elapsed_ms: [u32; 32]` and
`source_growth_bucket_phase: [u8; 32]`.

New `SourceMapCellState` fields mirroring the table entry:
`source_growth_bucket`, `source_growth_bucket_seen`,
`source_growth_phase`, `source_growth_enrolled`, plus
`source_definition_record` (the `@Nummer` slot, for the ±0x88 link) and
`source_placement_variant` (the persisted `rand() & 0x1F`). All
`#[serde(default)]` per the repo convention; bump `SAVE_VERSION`.

The 32860-slot table itself need **not** be modelled literally — the
original does not save it and rebuilds it from tile state. Per-cell
fields are equivalent unless probe-order fidelity is wanted; the only
observable difference is the "window full → silently not enrolled"
failure mode.

The COD parser needs the §2 bucket derivation exposed as a separate field
(do **not** overwrite `source_scheduler_interval`, which the production
scheduler also reads), a `nummer → index` lookup so `record ± 1`
resolves — the parser already builds `building_by_nummer` and drops it —
and `AnimTime` (`+0x70`), currently unparsed.

Hook `tick_source_growth_timers(dt_ms)` in as the **first** call of
`step()`, before `tick_source_resource_environment`, matching
`FUN_00489670`'s order. Since the sweep and the arming draw no `rand()`,
this does not perturb the documented RNG dispatch order.
