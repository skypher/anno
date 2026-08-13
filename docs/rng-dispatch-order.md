# RNG dispatch order & determinism roadmap

For a bit-exact reproduction the Rust sim must draw from the shared MSVC-CRT
`rand()` LCG in the **same order** the original does. This documents where we
are against that bar. It is the reference for finishing RNG fidelity.

## The shared RNG

The original seeds `srand(GetTickCount())` once (`1602_exe.c:106312`) and every
subsystem draws from the single CRT `rand()` stream:
`state = state*0x343fd + 0x269ec3; return (state >> 0x10) & 0x7fff`. Reproduced
bit-exact in `crates/anno-sim/src/source_rand.rs` (`SourceRand::next`); the sim
wrappers `next_source_rand()` / `next_rand()` (`simulation.rs`) share one
`rng_state`. The RNG primitive is **not** the problem — the draw *order* and
*coverage* are.

## The per-slice dispatcher `FUN_00489670`

Each ≤200 ms slice (see `docs/lockstep.md` and `scaled_sim_ms`) dispatches its
subsystems in a strict fixed order. The critical-section boundaries are mutexes
only; they do not reorder. 11 of the 13 slots draw `rand()`:

| Slot | Func | Subsystem | Draws rand? |
|--|--|--|--|
| S1 | `FUN_0047ca80` | periodic / tile-animation clock | no |
| S2 | `FUN_0047daf0` | building production/processing | yes (carrier gate `(rand()&3)==0`) |
| S3 | `FUN_0047f8a0` | city economy + population growth | yes (direct) |
| S4 | `FUN_0047b9c0` | kind-13 record phase-clock dispatch | yes |
| S5 | `FUN_0046b3e0` | island/resource environment (regen) | yes (direct) |
| S6 | `FUN_00478ab0` | deferred combat-action queue drain | yes |
| S7 | `FUN_004791b0` | combat/ship figure motion + opcode | yes |
| S8 | `FUN_004798e0` | market / building-service work (1000 ms) | yes |
| S9 | `FUN_0047a020` | ambient/native figure spawn + scatter | yes (direct) |
| S10 | `FUN_0047a8c0` | animation phase clock (9999 ms) | no |
| S11 | `FUN_00476350` | economy/diplomacy + round-robin player-control (4999 ms) | yes |
| S12 | `FUN_0042b4b0` | AI player controller (gated `!(DAT_005b706c&2)`) | yes |
| S13 | `FUN_00486b60` | gated secondary dispatch (`DAT_005b6b2c`) | yes |
| S14 | `FUN_00451890` | figure/entity motion integrator (per-update) | yes |

## Where the Rust `Simulation::step` diverges

Two classes of divergence keep the stream from matching today:

1. **Reordering of ported RNG subsystems.** The Rust draw order by source-slot
   is `S5, S6, [free-trader extra], S3, S4, S12, S14`; the source requires
   ascending `S3, S4, S5, S6, S12, S14`. So `tick_source_resource_environment`
   (S5) and the deferred-combat hits (S6) are hoisted **ahead of**
   `tick_source_city_dispatch` (S3) and `tick_source_kind13_dispatch` (S4).
   Latent RNG-divergence: only bites in a slice where ≥2 of them actually draw
   (event-gated), but that does happen under load. S12 (player controllers) and
   S14 (figure motion) are already in correct relative order.
   Evidence: dispatcher body `1602_exe.c:97975-97998` (S3,S4 before S5,S6) vs
   `simulation.rs` step order (resource before city/kind13).

2. **Unported RNG draws.** Six slots that draw in the source have their draws
   missing in Rust: S2 (production carrier gate), S7 (combat/ship motion), S8
   (market service), S9 (ambient spawn — subsystem unimplemented), S11
   (diplomacy/player-control), S13 (gated secondary — unimplemented). Plus a
   Rust-only draw with **no** source analog: `tick_free_traders`'
   `free_trader_target_gate` (`(rand()&3)==0`, `fidelity.rs`), a documented
   stand-in. Every missing/extra draw permanently offsets the stream.

## Why no reorder was applied (yet)

Reordering `step()` now is not warranted: there is **no captured original RNG
trace** to validate against (replay determinism today is Rust-vs-Rust), the
stream is non-bit-exact regardless while six slots are unported, and moving
city/kind13 across the timer block would churn many golden-output tests for no
proven fidelity gain. This is a report-with-evidence finding, not a fix.

## Roadmap to RNG bit-exactness

1. Port the six unported RNG draws (S2, S7, S8, S9, S11, S13) into their
   subsystems, and fold the free-trader AI into its true source slot (S11/S12
   player-control), removing the stand-in draw.
2. Then dispatch the `tick_source_*` ports in strict `FUN_00489670` order
   (S1…S14), interleaving the currently-separate timer layer (production,
   population, market, diplomacy, ships) at its real source position so
   `{city, kind13}` precede `{resource, deferred-combat}`.
3. Only then is a live RNG-word comparison against the original meaningful.
   Capture is read-only interval snapshots today (`docs/original-capture.md`);
   pinning/observing the original's `rand()` word needs code injection, which is
   blocked on this wow64 Wine — so this step waits on a viable injector.
