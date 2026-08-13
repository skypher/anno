'use strict';
//
// Frida capture harness for the original Anno 1602 `1602.exe`.
//
// Turns the shipping Windows binary into a steppable, observable engine so
// its per-tick state can be diffed against the Rust reimplementation. It:
//
//   1. Pins the RNG seed by hooking srand / GetTickCount, so a run is
//      reproducible and matches a headless Rust run seeded with the same
//      value (both use the MSVC LCG; see crates/anno-sim/src/source_rand.rs).
//   2. Hooks the simulation dispatcher FUN_00489670 and, on each call,
//      dumps the observable state (player economy, buildings, RNG word) as
//      one JSON line — the SAME schema the headless Rust driver emits
//      (crates/anno-game/src/bin/headless.rs), so tools/compare can diff them.
//   3. Records which subsystem ticker fired each tick by hooking the twelve
//      tickers inside FUN_00489670, producing the scheduler trace that
//      anno_sim::fidelity::compare_trace validates.
//
// Addresses are from docs/re-notes/architecture.md. They are absolute PE
// addresses assuming the default image base 0x400000; the script rebases
// them onto the actual loaded module base, so ASLR is handled.
//
// This script is UNTESTABLE in this repository — it requires the copyrighted
// 1602.exe, which is not present. It is written against the documented
// address map and Frida's stable API; treat the offsets as the primary risk
// surface and validate the first dump against a known savegame before
// trusting a full run. See tools/capture/README.md.

// ---- Configuration (overridable from the Python driver via rpc/env) -------

const CONFIG = {
    // Fixed RNG seed. Mirrors `Simulation::seed_source_rand(seed)`.
    seed: 1,
    // Emit a state dump every N sim ticks (matches headless --dump-every).
    dumpEvery: 10,
    // Stop the process after this many sim ticks (0 = run until closed).
    maxTicks: 3000,
    // Default image base the static addresses were recorded against.
    assumedImageBase: ptr('0x400000'),
    moduleName: '1602.exe',
};

// ---- Static addresses (image-base-relative) -------------------------------

const ADDR = {
    // Simulation dispatcher: one call == one sim tick.
    simDispatch: 0x00489670,
    // The twelve subsystem tickers inside FUN_00489670, with the cadence
    // documented in architecture.md. `key` matches the headless dump's
    // subsystem names where they overlap.
    tickers: [
        { key: 'tile_anim',   addr: 0x0047ca80, interval_ms: 40000 },
        { key: 'production',  addr: 0x0047daf0, interval_ms: 999 },
        { key: 'population',  addr: 0x0047f8a0, interval_ms: 9999 },
        { key: 'citizens',    addr: 0x0047b9c0, interval_ms: 15000 },
        { key: 'island_evt',  addr: 0x0046b3e0, interval_ms: 29999 },
        { key: 'events',      addr: 0x00478ab0, interval_ms: 0 },
        { key: 'ships',       addr: 0x004791b0, interval_ms: 1000 },
        { key: 'multislot',   addr: 0x004798e0, interval_ms: 1000 },
        { key: 'military',    addr: 0x0047a020, interval_ms: 9999 },
        { key: 'projectile',  addr: 0x0047a8c0, interval_ms: 9999 },
        { key: 'diplomacy',   addr: 0x00476350, interval_ms: 4999 },
        { key: 'ai',          addr: 0x0042b4b0, interval_ms: 0 },
    ],

    // Data tables (architecture.md "Key Data Tables").
    playerData: 0x005b7680,       // 160 bytes each, 7 players
    islandData: 0x005e6b20,       // 2816 bytes each, 50 islands
    buildingDefs: 0x00619b60,     // 136 bytes each
    activeBuildingsPtr: 0x0049aebc, // PTR: 20 bytes each, 1037 max

    // Game constants.
    speedMultiplier: 0x0049af74,
};

// Per-player record layout (DAT_005b7680, 160 bytes/player). The gold and
// population-tier offsets below are PLACEHOLDERS pending a field census —
// they must be confirmed against a known savegame before the economy
// comparison is meaningful (see README "Calibrating field offsets"). The
// RNG-word and tick-count comparisons do not depend on them.
const PLAYER = {
    stride: 160,
    count: 7,
    goldOffset: 0x00,        // TODO: confirm
    populationOffset: 0x10,  // TODO: confirm; 5 × u32
    populationTiers: 5,
};

// MSVC CRT `holdrand` — the 32-bit LCG state rand()/srand() operate on. Its
// address is CRT-build specific and NOT in architecture.md; the driver can
// override it (rpc `setHoldrand`) once located. When null, the RNG word is
// reported as -1 and only tick/state comparison is meaningful.
let HOLDRAND_ADDR = null;

// ---- Rebasing -------------------------------------------------------------

let base = null;
function at(staticAddr) {
    // Rebase a default-image-base address onto the loaded module.
    return base.add(ptr(staticAddr).sub(CONFIG.assumedImageBase));
}

// ---- State ----------------------------------------------------------------

let tick = 0;
let firedThisTick = [];

function readHoldrand() {
    if (HOLDRAND_ADDR === null) return -1;
    try {
        return HOLDRAND_ADDR.readU32();
    } catch (_e) {
        return -1;
    }
}

function readPlayers() {
    const out = [];
    const table = at(ADDR.playerData);
    for (let slot = 0; slot < PLAYER.count; slot++) {
        const rec = table.add(slot * PLAYER.stride);
        let gold = 0;
        const population = [];
        try {
            gold = rec.add(PLAYER.goldOffset).readS32();
            for (let t = 0; t < PLAYER.populationTiers; t++) {
                population.push(rec.add(PLAYER.populationOffset + t * 4).readU32());
            }
        } catch (_e) {
            // Unmapped / not-yet-calibrated: leave zeros.
            while (population.length < PLAYER.populationTiers) population.push(0);
        }
        out.push({ slot, gold, population });
    }
    return out;
}

function emit(obj) {
    // One JSON object per line on stdout via Frida's message channel; the
    // Python driver forwards it verbatim to the JSONL output file.
    send(obj);
}

function dump() {
    emit({
        kind: 'dump',
        tick: tick,
        rng_state: readHoldrand(),
        players: readPlayers(),
        fired: firedThisTick.slice(),
    });
}

// ---- Installation ---------------------------------------------------------

function install() {
    const module = Process.getModuleByName(CONFIG.moduleName);
    base = module.base;
    emit({ kind: 'log', message: `module ${CONFIG.moduleName} base ${base}` });

    // 1. Pin the seed: force srand() to the configured value and neutralize
    //    the GetTickCount()-derived entropy the startup path feeds it.
    const getTickCount = Module.findExportByName('kernel32.dll', 'GetTickCount');
    if (getTickCount !== null) {
        Interceptor.replace(
            getTickCount,
            new NativeCallback(() => CONFIG.seed, 'uint32', [])
        );
        emit({ kind: 'log', message: `GetTickCount pinned to ${CONFIG.seed}` });
    }
    // Also hook srand directly if the CRT is dynamically linked; a statically
    // linked CRT (common for this era) exposes no export, in which case the
    // GetTickCount pin above is the effective control.
    const srand = Module.findExportByName(null, 'srand');
    if (srand !== null) {
        Interceptor.attach(srand, {
            onEnter(args) {
                args[0] = ptr(CONFIG.seed);
            },
        });
        emit({ kind: 'log', message: 'srand argument pinned' });
    }

    // 2. Subsystem tickers → trace events. Attaching before the dispatcher
    //    hook's onLeave means firedThisTick is populated by the time we dump.
    for (const ticker of ADDR.tickers) {
        try {
            Interceptor.attach(at(ticker.addr), {
                onEnter() {
                    firedThisTick.push(ticker.key);
                },
            });
        } catch (e) {
            emit({ kind: 'log', message: `ticker ${ticker.key} hook failed: ${e}` });
        }
    }

    // 3. Simulation dispatcher: one call == one sim tick.
    Interceptor.attach(at(ADDR.simDispatch), {
        onEnter() {
            firedThisTick = [];
        },
        onLeave() {
            tick += 1;
            if (tick % CONFIG.dumpEvery === 0) {
                dump();
            }
            if (CONFIG.maxTicks > 0 && tick >= CONFIG.maxTicks) {
                emit({ kind: 'done', tick: tick });
            }
        },
    });

    emit({ kind: 'log', message: 'hooks installed' });
    dump(); // tick 0 baseline
}

// ---- RPC (driver control) -------------------------------------------------

rpc.exports = {
    configure(overrides) {
        Object.assign(CONFIG, overrides || {});
        return CONFIG;
    },
    setHoldrand(addrString) {
        HOLDRAND_ADDR = ptr(addrString);
        return HOLDRAND_ADDR.toString();
    },
    setPlayerLayout(layout) {
        Object.assign(PLAYER, layout || {});
        return PLAYER;
    },
    // Inject one player command by synthesizing the Win32 message the UI
    // would post. The driver supplies the resolved (msg, wParam, lParam);
    // command→message translation is scenario-specific and lives in the
    // driver, not here. Requires the game window handle.
    postMessage(hwndString, msg, wParam, lParam) {
        const postMessageW = new NativeFunction(
            Module.getExportByName('user32.dll', 'PostMessageW'),
            'int',
            ['pointer', 'uint32', 'uint32', 'int32']
        );
        return postMessageW(ptr(hwndString), msg, wParam, lParam);
    },
    install() {
        install();
        return true;
    },
};
