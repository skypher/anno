//! Building definitions and instances.
//!
//! Ported from the building definition table at DAT_00619b60
//! (136-byte stride). The active-building array at
//! `PTR_DAT_0049aebc` is 20-byte stride × 1037 entries in the
//! original; we use Vec sizing instead of a fixed cap.

use crate::types::{Good, ProductionType};

/// Default fire-damage cap from `haeuser.cod` `Maxbrand: 4`.
/// The parser reads it at `1602_exe.c:68086` and carries it through
/// the stateful COD definition stream unless a later building
/// overrides the property.
pub const DEFAULT_MAX_BRAND_DAMAGE_TICKS: u16 = 4;

/// Original `Ruinenr` sentinel for buildings that do not leave a ruin.
pub const NO_RUIN_ID: u8 = 0xff;

/// Source placement command fields emitted by `FUN_004631b0` for one live
/// building placement. `definition_offset` is the on-disk INSELHAUS u16;
/// the executable resolves it by adding `0x4e20` before looking up haeuser.
///
/// The remaining fields are the exact inputs packed into the record word:
/// `orientation` is `param_3`, `variant` is `param_9`, `metadata` is
/// `param_4`, `map_owner_slot` is `param_5`, and `dynamic_object_owner` is
/// `param_6`. `random_seed` records the five random bits written at 17..=21.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceBuildingCommand {
    pub definition_offset: u16,
    pub orientation: u8,
    pub variant: u8,
    pub metadata: u8,
    pub map_owner_slot: u8,
    pub random_seed: u8,
    pub dynamic_object_owner: u8,
}

impl SourceBuildingCommand {
    /// Decode every source-command field from a parsed INSELHAUS record.
    pub fn from_island_tile(tile: anno_formats::szs::IslandTile) -> Self {
        Self {
            definition_offset: tile.building_id,
            orientation: tile.orientation & 3,
            variant: (tile.orientation >> 2) & 0x0f,
            metadata: (tile.orientation >> 6) | ((tile.anim_count & 0x3f) << 2),
            map_owner_slot: tile.source_owner(),
            random_seed: ((tile.flags >> 1) & 0x1f) as u8,
            dynamic_object_owner: tile.source_dynamic_object_owner(),
        }
    }

    /// Encode the command in the eight-byte INSELHAUS layout used by
    /// `FUN_004631b0`. The position bytes are supplied by the instance.
    pub fn to_island_tile(self, x: u8, y: u8) -> anno_formats::szs::IslandTile {
        anno_formats::szs::IslandTile {
            building_id: self.definition_offset,
            x,
            y,
            orientation: (self.orientation & 3)
                | ((self.variant & 0x0f) << 2)
                | ((self.metadata & 3) << 6),
            anim_count: ((self.metadata >> 2) & 0x3f) | ((self.map_owner_slot & 3) << 6),
            flags: ((self.map_owner_slot as u16 >> 2) & 1)
                | ((self.random_seed as u16 & 0x1f) << 1)
                | ((self.dynamic_object_owner as u16 & 0x0f) << 6),
        }
    }
}

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
    /// Authored building `Maxenergy` from haeuser.cod. The source compiles
    /// it as `round(Maxenergy * 32)` at definition offset `+0x64`, where
    /// `FUN_0047a650` compares it with accumulated category-6 raw damage.
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
    /// Defensive cannon count (`Kanon: <n>` in haeuser.cod). Four
    /// turret/castle buildings carry `Kanon: 2`.
    pub defensive_cannons: u8,
    /// Fire-damage cap from `Maxbrand` in haeuser.cod. The shipping
    /// file sets the template/default to 4, and the stateful COD
    /// parser inherits it across building definitions unless a later
    /// definition overrides it.
    pub max_brand_damage_ticks: u16,
    /// Source `Ruinenr` code from haeuser.cod. The original parser's
    /// token table lives at `1602_exe.c:66354-66367`; `0xff` means
    /// `NORUINE`.
    pub ruin_id: u8,
    /// Fertility this building's plantation/farm requires on
    /// the host island. Derived from haeuser.cod's `Rohstoff`
    /// field via the editor.cod-pinned name mapping
    /// (TABAKBAUM → Tobacco, KAKAOBAUM → Cocoa, ZUCKERROHR
    /// → Sugarcane, WEINTRAUBEN → Vines, BAUMWOLLE → Cotton,
    /// GEWUERZBAUM → Spices, GETREIDE → Grain). `None` for
    /// universal buildings (wood/stone/cloth chain) that work
    /// on any island.
    pub required_fertility: Option<anno_formats::szs::Fertility>,
}

/// Ore-deposit size. Manual sec. 6.7 + Tim Howgego's resources
/// appendix: small deposits give 80 tons, large deposits 240 tons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OreDeposit {
    #[default]
    None,
    Small, // ERZBERG_KLEIN — 80 tons
    Large, // ERZBERG_GROSS — 240 tons
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
    /// Source `island + 0xac` map-object slot for a live `Kind=HQ` building.
    /// `None` means this building is addressed through its static map cell.
    #[serde(default)]
    pub source_dynamic_object_slot: Option<u8>,
    /// The original placement command for player-created buildings. Scenario
    /// buildings retain their authored INSELHAUS records instead.
    #[serde(default)]
    pub source_placement_command: Option<SourceBuildingCommand>,
    pub active: bool,

    /// Production efficiency (0-128 scale, 128 = 100%).
    pub efficiency: u8,

    /// Current stock levels.
    pub input_1_stock: u16,
    pub input_2_stock: u16,
    pub output_stock: u16,

    /// Production timer (counts down from cycle_time).
    pub production_timer_ms: u32,

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

    /// Hit points used by modeled disaster effects.
    #[serde(default = "default_building_health")]
    pub health: u16,

    /// Fire damage ticks already applied to this building during its
    /// current burn cycle. RE: haeuser.cod `Maxbrand: 4` default,
    /// parsed at `1602_exe.c:68086`.
    #[serde(default)]
    pub fire_damage_ticks: u16,

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

    /// Consecutive low-efficiency production ticks. Used for
    /// haeuser.cod `Maxnorohst` warm-up reset and `Doerrflg`
    /// plantation dry-up.
    #[serde(default)]
    pub idle_ticks: u32,
    /// Remaining-ore counter for ore-mine buildings (RE: haeuser.cod
    /// `Erzbergnr` deposit size). Initialised to the deposit's
    /// `OreDeposit::capacity()` when the mine is placed; each
    /// successful production cycle decrements by `output_rate`.
    /// When 0, the mine refuses further production. `u16::MAX`
    /// = uncapped (default for non-mine buildings).
    #[serde(default = "default_remaining_ore")]
    pub remaining_ore: u16,
}

fn default_remaining_ore() -> u16 {
    u16::MAX
}

fn default_building_health() -> u16 {
    BUILDING_MAX_HEALTH
}
pub const BUILDING_MAX_HEALTH: u16 = 100;

impl BuildingInstance {
    pub fn new(def_id: u16, island_id: u8, tile_x: u16, tile_y: u16, owner: u8) -> Self {
        Self {
            def_id,
            island_id,
            tile_x,
            tile_y,
            owner,
            source_dynamic_object_slot: None,
            source_placement_command: None,
            active: true,
            efficiency: 0,
            input_1_stock: 0,
            input_2_stock: 0,
            output_stock: 0,
            production_timer_ms: 0,
            construction_ms_remaining: 0,
            construction_ms_total: 0,
            health: BUILDING_MAX_HEALTH,
            fire_damage_ticks: 0,
            house_tier: 0,
            wood_needed: 0,
            tools_needed: 0,
            bricks_needed: 0,
            idle_ticks: 0,
            remaining_ore: u16::MAX,
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

#[cfg(test)]
mod tests {
    use super::SourceBuildingCommand;
    use anno_formats::szs::IslandTile;

    #[test]
    fn source_command_round_trips_every_fun_004631b0_field() {
        let source = IslandTile {
            building_id: 0x1234,
            x: 0x56,
            y: 0x78,
            orientation: 0b1011_0110,
            anim_count: 0b1101_1010,
            flags: 0b0000_0011_0110_1011,
        };

        let command = SourceBuildingCommand::from_island_tile(source);

        assert_eq!(command.definition_offset, 0x1234);
        assert_eq!(command.orientation, 2);
        assert_eq!(command.variant, 13);
        assert_eq!(command.metadata, 106);
        assert_eq!(command.map_owner_slot, 7);
        assert_eq!(command.random_seed, 21);
        assert_eq!(command.dynamic_object_owner, 13);
        let encoded = command.to_island_tile(source.x, source.y);
        assert_eq!(encoded.building_id, source.building_id);
        assert_eq!(encoded.x, source.x);
        assert_eq!(encoded.y, source.y);
        assert_eq!(encoded.orientation, source.orientation);
        assert_eq!(encoded.anim_count, source.anim_count);
        assert_eq!(encoded.flags, source.flags);
    }
}
