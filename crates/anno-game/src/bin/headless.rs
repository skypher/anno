//! Headless fixed-timestep lockstep driver.
//!
//! Runs the real scenario simulation (the same `Simulation` the SDL game
//! constructs) with a pinned RNG seed and a constant synthetic timestep,
//! and emits one JSON line per dump interval describing the observable
//! state: game clock, RNG word, state hash, per-player economy, unit and
//! ship positions, and which subsystem tickers fired.
//!
//! The dump schema is shared with the Frida capture harness for the
//! original 1602.exe (`tools/capture/`), so `tools/compare` can diff the
//! two runs tick by tick. See `docs/lockstep.md`.
//!
//! Usage:
//!   headless --scenario extracted/szenes/game00.szs [options]
//!
//! Options:
//!   --data-dir <dir>    directory with haeuser.cod / figuren.cod (default: extracted)
//!   --seed <u32>        RNG seed, mirrors srand(GetTickCount()) (default: 1)
//!   --dt <ms>           fixed timestep per tick (default: 100)
//!   --ticks <n>         number of ticks to run (default: 3000)
//!   --dump-every <n>    emit a dump line every n ticks (default: 10)
//!   --replay <file>     apply a recorded command stream while running
//!   --out <file>        write JSONL here instead of stdout

use anno_sim::fidelity::{Subsystem, TickScheduler};
use anno_sim::simulation::Simulation;
use std::io::Write;
use std::path::PathBuf;

struct Options {
    scenario: PathBuf,
    data_dir: PathBuf,
    seed: u32,
    dt_ms: u32,
    ticks: u32,
    dump_every: u32,
    replay: Option<PathBuf>,
    out: Option<PathBuf>,
}

fn parse_args() -> Result<Options, String> {
    let mut opts = Options {
        scenario: PathBuf::new(),
        data_dir: PathBuf::from("extracted"),
        seed: 1,
        dt_ms: 100,
        ticks: 3000,
        dump_every: 10,
        replay: None,
        out: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| format!("{name} expects a value"))
        };
        match arg.as_str() {
            "--scenario" => opts.scenario = PathBuf::from(value("--scenario")?),
            "--data-dir" => opts.data_dir = PathBuf::from(value("--data-dir")?),
            "--seed" => {
                opts.seed = value("--seed")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--dt" => {
                opts.dt_ms = value("--dt")?
                    .parse()
                    .map_err(|e| format!("--dt: {e}"))?
            }
            "--ticks" => {
                opts.ticks = value("--ticks")?
                    .parse()
                    .map_err(|e| format!("--ticks: {e}"))?
            }
            "--dump-every" => {
                opts.dump_every = value("--dump-every")?
                    .parse()
                    .map_err(|e| format!("--dump-every: {e}"))?
            }
            "--replay" => opts.replay = Some(PathBuf::from(value("--replay")?)),
            "--out" => opts.out = Some(PathBuf::from(value("--out")?)),
            other if opts.scenario.as_os_str().is_empty() && !other.starts_with('-') => {
                opts.scenario = PathBuf::from(other);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if opts.scenario.as_os_str().is_empty() {
        return Err("usage: headless --scenario <file.szs> [--data-dir dir] [--seed n] \
                    [--dt ms] [--ticks n] [--dump-every n] [--replay file] [--out file]"
            .into());
    }
    if opts.dt_ms == 0 || opts.dump_every == 0 {
        return Err("--dt and --dump-every must be nonzero".into());
    }
    Ok(opts)
}

fn subsystem_name(subsystem: Subsystem) -> &'static str {
    match subsystem {
        Subsystem::Production => "production",
        Subsystem::PlayerControl => "player_control",
        Subsystem::Population => "population",
        Subsystem::Diplomacy => "diplomacy",
        Subsystem::MarketCoverage => "market",
        Subsystem::Ships => "ships",
        Subsystem::Military => "military",
        Subsystem::Events => "events",
    }
}

fn dump_line(sim: &Simulation, tick: u32, dt_ms: u32, fired: &[&'static str]) -> String {
    let mut players = String::new();
    for (slot, player) in sim.players.iter().enumerate() {
        if !players.is_empty() {
            players.push(',');
        }
        let population = player
            .population
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",");
        players.push_str(&format!(
            "{{\"slot\":{},\"gold\":{},\"population\":[{}]}}",
            slot, player.gold, population
        ));
    }

    let mut units = String::new();
    for unit in sim.military_units.iter().filter(|unit| unit.active) {
        if !units.is_empty() {
            units.push(',');
        }
        units.push_str(&format!(
            "{{\"owner\":{},\"x\":{},\"y\":{},\"health\":{}}}",
            unit.owner, unit.tile_x, unit.tile_y, unit.health
        ));
    }

    let mut ships = String::new();
    for ship in sim.trade_ships.iter().filter(|ship| ship.active) {
        if !ships.is_empty() {
            ships.push(',');
        }
        ships.push_str(&format!(
            "{{\"owner\":{},\"x\":{},\"y\":{}}}",
            ship.owner, ship.world_x, ship.world_y
        ));
    }

    let fired = fired
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{{\"tick\":{tick},\"sim_ms\":{},\"game_clock\":{},\"source_time_ticks\":{},\
         \"rng_state\":{},\"state_hash\":\"{:016x}\",\"buildings\":{},\"warehouses\":{},\
         \"figures_active\":{},\"players\":[{players}],\"units\":[{units}],\
         \"ships\":[{ships}],\"fired\":[{fired}]}}",
        tick as u64 * dt_ms as u64,
        sim.game_clock,
        sim.source_time_ticks,
        sim.source_rand_state(),
        sim.state_hash(),
        sim.buildings.len(),
        sim.warehouses.len(),
        sim.figures.iter().filter(|f| f.is_active()).count(),
    )
}

fn main() {
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let cod_data =
        std::fs::read(opts.data_dir.join("haeuser.cod")).expect("failed to read haeuser.cod");
    let cod = anno_formats::cod::CodFile::parse(&cod_data).expect("failed to parse haeuser.cod");
    let defs = anno_sim::data_bridge::load_building_defs(&cod);
    let figures = match std::fs::read(opts.data_dir.join("figuren.cod")) {
        Ok(bytes) => anno_formats::figuren::FiguresFile::parse(&bytes),
        Err(error) => {
            eprintln!("(figuren.cod not loaded: {error}) — running with default figure config");
            anno_formats::figuren::FiguresFile {
                constants: Default::default(),
                figures: Vec::new(),
            }
        }
    };
    let szs_data = std::fs::read(&opts.scenario).expect("failed to read scenario");
    let szs = anno_formats::szs::SzsFile::parse(&szs_data).expect("failed to parse scenario");

    let mut sim = anno_game::scenario::build_simulation(&szs, &cod, &defs, &figures);
    sim.seed_source_rand(opts.seed);

    let replay = opts.replay.as_ref().map(|path| {
        let recording = anno_sim::replay::load_recording(path).expect("failed to load replay");
        eprintln!(
            "Replaying {} command(s) from {}",
            recording.entries.len(),
            path.display()
        );
        recording
    });
    let mut replay_cursor = 0usize;

    let mut out: Box<dyn Write> = match &opts.out {
        Some(path) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).expect("failed to create output file"),
        )),
        None => Box::new(std::io::BufWriter::new(std::io::stdout())),
    };

    // A parallel scheduler trace: which subsystem cadences fire on which
    // tick. Uses the same fidelity::TIMING_SPECS that compare_trace()
    // validates against captures from the original executable.
    let mut scheduler = TickScheduler::new();

    writeln!(out, "{}", dump_line(&sim, 0, opts.dt_ms, &[])).expect("write");

    for tick in 1..=opts.ticks {
        if let Some(recording) = &replay {
            while replay_cursor < recording.entries.len()
                && recording.entries[replay_cursor].0 <= sim.game_clock
            {
                anno_game::game_commands::apply_game_command(
                    &mut sim,
                    &szs.islands,
                    &cod,
                    &defs,
                    &recording.entries[replay_cursor].1,
                );
                replay_cursor += 1;
            }
        }

        let events = scheduler.advance_real_time(opts.dt_ms, sim.speed_multiplier);
        sim.tick(opts.dt_ms);
        // Apply queued kind-13 house replacements (the source map writer
        // runs synchronously; headless has no renderer overlay to patch).
        sim.drain_source_kind13_replacements(&cod);

        if tick % opts.dump_every == 0 || tick == opts.ticks {
            let fired: Vec<&'static str> = events
                .iter()
                .map(|event| subsystem_name(event.subsystem))
                .collect();
            writeln!(out, "{}", dump_line(&sim, tick, opts.dt_ms, &fired)).expect("write");
        }
    }
    out.flush().expect("flush");

    if let Some(recording) = &replay {
        if replay_cursor < recording.entries.len() {
            eprintln!(
                "warning: {} replay command(s) still pending at exit (game_clock {})",
                recording.entries.len() - replay_cursor,
                sim.game_clock
            );
        }
    }
    eprintln!(
        "Done: {} ticks × {} ms, game_clock {}, state_hash {:016x}",
        opts.ticks,
        opts.dt_ms,
        sim.game_clock,
        sim.state_hash()
    );
}
