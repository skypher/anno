//! Building definitions and instances.
//!
//! Ported from the building definition table at DAT_00619b60 (136 bytes each)
//! and the active building array at PTR_DAT_0049aebc (20-byte stride, 1037 entries).

use crate::types::{Good, ProductionType};

/// Maximum active building instances.
pub const MAX_BUILDINGS: usize = 1037;

/// Building definition (loaded from haeuser.cod).
#[derive(Debug, Clone)]
pub struct BuildingDef {
    pub id: u16,
    pub category: u8,
    pub width: u8,
    pub height: u8,
    pub production_type: ProductionType,
    /// Building kind from COD (BODEN, GEBAEUDE, HQ, etc.)
    pub kind: String,
    /// Production kind from COD (HANDWERK, MARKT, KONTOR, KIRCHE, etc.)
    pub prod_kind: String,
    /// Service radius in tiles (0 = no service area).
    /// Used by marketplaces (extend warehouse access), churches, taverns, etc.
    pub radius: u16,
    pub output_good: Good,
    pub input_good_1: Good,
    pub input_good_2: Good,
    pub output_rate: u16,
    pub input_1_rate: u16,
    pub input_2_rate: u16,
    pub storage_capacity: u16,
    pub cycle_time_ms: u32,
    pub carrier_interval_ms: u32,
    pub cost_gold: u32,
    pub cost_tools: u16,
    pub cost_wood: u16,
    pub cost_bricks: u16,
    pub maintenance_cost: u16,
    /// Native-village flag (`Nativflg: 1` inside HAUS_PRODTYP).
    /// Identifies buildings belonging to the indigenous-village
    /// faction (chief's hut, native plantations, native guard huts).
    /// Used to gate civilian spawning, friendly-faction trade UI,
    /// and the manual sec. 7.5/8.6 native-trade behaviour.
    pub native: bool,
    /// Required infrastructure level. RE: haeuser.cod `Bauinfra`
    /// field whose values follow `INFRA_STUFE_<tier><letter>`
    /// (`STUFE_1A` → Pioneer, `_2*` → Settler, `_3*` → Citizen,
    /// `_4*` → Merchant, `_5*` → Aristocrat). Special tags
    /// (`INFRA_BURG_*`, `INFRA_KONTOR_*`, `INFRA_WACHTURM`,
    /// `INFRA_MUSKETE`, `INFRA_KANON`) gate upgrade chains.
    /// `min_tier = 0` → no infrastructure requirement.
    pub min_tier: u8,
    /// Maximum consecutive production cycles a building will run
    /// without input materials before going idle. RE: haeuser.cod
    /// `Maxnorohst` field. Distribution in haeuser.cod (12× value
    /// 6, 4× value 8, 1× value 5) — defaults to 6 in our parser.
    /// Buildings with no input requirement (output-only resources
    /// like Wood) ignore this since they never lack input.
    pub max_no_input_ticks: u8,
    /// Plantation drought flag (`Doerrflg: 1` in haeuser.cod). When
    /// set, this building is a plantation crop tile that can dry up
    /// from prolonged inactivity — once idle for an extended period
    /// it stops producing entirely until rebuilt. RE: 7 building
    /// entries carry this flag, all plantation crop tiles
    /// (Getreide / etc.).
    pub can_dry_up: bool,
    /// Per-terrain walking speed quad. RE: haeuser.cod `Wegspeed`
    /// field — each terrain tile lists 4 speeds in 1/100 units.
    /// The most common quad is `145, 120, 170, 100` (plain ground:
    /// 145 empty off-road, 120 loaded off-road, 170 empty on-road,
    /// 100 loaded on-road). Roads boost empty carriers but
    /// slightly slow loaded ones. `[100; 4]` = no preference.
    pub wegspeed: [u16; 4],
    /// Door flag (`Tuerflg: 1` in haeuser.cod). Marks residences
    /// from which civilians may emerge. 56 buildings carry this.
    /// `false` for buildings without addressable doors (production
    /// houses, public services, etc.) — civilian spawning skips
    /// these.
    pub has_door: bool,
    /// Upgradeable flag (`Ausbauflg: 1` in haeuser.cod). 10
    /// residence buildings carry it; promotion (Pioneer → Settler
    /// → … → Aristocrat) is gated on this. False for fixed-tier
    /// buildings that can't be promoted.
    pub upgradeable: bool,
    /// Maximum cumulative production work the building can do
    /// before needing repair / overhaul. RE: haeuser.cod
    /// `Maxenergy` field — varies 3..230 with most common values
    /// 5/50/115. 0 means uncapped (default for buildings without
    /// the field).
    pub max_energy: u16,
    /// Ore-deposit size for ore-source buildings. RE: haeuser.cod
    /// `Erzbergnr` — `ERZBERG_KLEIN` (small, 80t per Tim Howgego
    /// appendix) or `ERZBERG_GROSS` (large, 240t total). `None`
    /// for non-ore buildings; the simulation can use this to
    /// initialise per-deposit `output_stock` caps so mines deplete
    /// finite resources rather than producing forever.
    pub ore_deposit: OreDeposit,
    /// Pirate-faction flag (`Piratflg: 1` in haeuser.cod). 3
    /// entries — pirate Kontor + huts. Mirrors `native` for the
    /// pirate slot 6.
    pub pirate_owned: bool,
    /// Defensive cannon count (`Kanon: <n>` in haeuser.cod). 4
    /// turret/castle buildings carry `Kanon: 2`. Drives passive
    /// defensive damage radiating from owned towers/castles.
    pub defensive_cannons: u8,
}

/// Ore-deposit size. Manual sec. 6.7 + Tim Howgego's resources
/// appendix: small deposits give 80 tons, large deposits 240 tons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OreDeposit {
    #[default]
    None,
    Small,  // ERZBERG_KLEIN — 80 tons
    Large,  // ERZBERG_GROSS — 240 tons
}

impl OreDeposit {
    /// Total ore tonnage available from this deposit. 0 for `None`.
    pub fn capacity(self) -> u16 {
        match self {
            OreDeposit::None => 0,
            OreDeposit::Small => 80,
            OreDeposit::Large => 240,
        }
    }
}

/// An active building instance in the world.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildingInstance {
    pub def_id: u16,
    pub island_id: u8,
    pub tile_x: u16,
    pub tile_y: u16,
    pub owner: u8,
    pub active: bool,

    /// Production efficiency (0-128 scale, 128 = 100%).
    pub efficiency: u8,

    /// Current stock levels.
    pub input_1_stock: u16,
    pub input_2_stock: u16,
    pub output_stock: u16,

    /// Production timer (counts down from cycle_time).
    pub production_timer_ms: u32,

    /// Accumulated production work.
    pub total_work: u16,

    /// Construction time remaining in ms. While > 0 the building is placed
    /// but not yet operational: production is gated, carriers are not
    /// dispatched, and the renderer applies a blue tint with a progress bar.
    /// Set to 0 (default) for fully-built buildings.
    #[serde(default)]
    pub construction_ms_remaining: u32,

    /// Total construction duration in ms (for progress %, copied from def
    /// at placement time). 0 means instant build.
    #[serde(default)]
    pub construction_ms_total: u32,

    /// Hit points for combat damage. Decremented by adjacent enemy units
    /// each military tick; building is removed when health reaches 0.
    #[serde(default = "default_building_health")]
    pub health: u16,

    /// Residence tier for `WOHN` buildings: 0=Pioneer, 1=Settler, 2=Citizen,
    /// 3=Merchant, 4=Aristocrat. Promoted when its tier is fully
    /// satisfied. Higher tiers grant more housing capacity. Unused for
    /// non-WOHN buildings.
    #[serde(default)]
    pub house_tier: u8,

    /// Materials still owed before construction can finish. Drained by
    /// the entity tick from the player's island warehouses. While any
    /// remain > 0, `construction_ms_remaining` doesn't decrement.
    #[serde(default)]
    pub wood_needed: u16,
    #[serde(default)]
    pub tools_needed: u16,
    #[serde(default)]
    pub bricks_needed: u16,

    /// Consecutive production ticks this building produced 0 output.
    /// When the count reaches the idle threshold, the building's
    /// maintenance contribution is halved (and pings back up to full
    /// once production resumes).
    #[serde(default)]
    pub idle_ticks: u32,
    /// Construction priority: higher = drained materials first.
    /// 0 = normal (default), 1 = high, 2 = critical. The entity
    /// tick sorts pending-construction buildings by `-priority`
    /// before trickling materials so the player can override the
    /// natural placement order.
    #[serde(default)]
    pub build_priority: u8,
}

/// Production ticks a building must idle before its maintenance halves.
pub const IDLE_MAINTENANCE_THRESHOLD: u32 = 5;

fn default_building_health() -> u16 { BUILDING_MAX_HEALTH }
pub const BUILDING_MAX_HEALTH: u16 = 100;

impl BuildingInstance {
    pub fn new(def_id: u16, island_id: u8, tile_x: u16, tile_y: u16, owner: u8) -> Self {
        Self {
            def_id,
            island_id,
            tile_x,
            tile_y,
            owner,
            active: true,
            efficiency: 0,
            input_1_stock: 0,
            input_2_stock: 0,
            output_stock: 0,
            production_timer_ms: 0,
            total_work: 0,
            construction_ms_remaining: 0,
            construction_ms_total: 0,
            health: BUILDING_MAX_HEALTH,
            house_tier: 0,
            wood_needed: 0,
            tools_needed: 0,
            bricks_needed: 0,
            idle_ticks: 0,
            build_priority: 0,
        }
    }

    /// Returns true if the building is finished and operational.
    pub fn is_built(&self) -> bool {
        self.construction_ms_remaining == 0
            && self.wood_needed == 0
            && self.tools_needed == 0
            && self.bricks_needed == 0
    }

    /// Construction progress in 0..=128 (matches efficiency scale).
    pub fn construction_progress_128(&self) -> u8 {
        if self.construction_ms_total == 0 {
            return 128;
        }
        let done = self.construction_ms_total - self.construction_ms_remaining;
        ((done as u64 * 128) / self.construction_ms_total as u64).min(128) as u8
    }
}
