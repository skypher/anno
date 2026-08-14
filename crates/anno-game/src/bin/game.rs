//! Anno 1602 — Live game viewer with integrated simulation.
//!
//! Renders the isometric map while running the full simulation loop.
//! Carriers, civilians, trade ships, and military units are drawn from
//! source sprites with marker fallbacks.
//!
//! Controls:
//!   Arrow keys / mouse drag: scroll the map
//!   F2/F3/F4: bird-eye/normal/detailed zoom level
//!   Tab: cycle through islands
//!   W: selected ship hoists white flag and surrenders to pirates
//!   Pause: pause simulation (Esc/right-click resumes)
//!   J: jump to the active object
//!   O: options menu
//!   F5/F6/F7: normal/double/quadruple game speed
//!   B: toggle build mode (then 1-9 to select building, click to place;
//!      [/] cycle category, PgUp/PgDn flip page, Z/X rotate)
//!   F: video sequences and speech menu
//!   D: toggle diplomacy panel (Up/Down=select player, Left/Right=cycle relation)
//!   I: toggle info/status mode (click buildings for object info)
//!   K: toggle combat mode (click own units, right-click to move)
//!   H: cycle between own warehouses
//!   C: list own and trade-agreement cities
//!   S: list own ships
//!   Ctrl+1-9: store selected troop assembly; 1-9 recalls it
//!   Enter: open chat input (multiplayer); type then Enter to send, Esc cancels
//!   L: open save-slot picker (Up/Down, S to save, L to load)
//!
//! Multiplayer flags:
//!   --host PORT          run as host, broadcast snapshots every 1s
//!   --join HOST:PORT     run as client, replace local sim with received snapshots
//!   Left-click on own ship/unit in combat mode: select (Shift+click adds units)
//!   Right-click (with units selected): move-to order
//!   Right-click (no selection): inspect building/tile, or resume if paused
//!   Escape: quit (or close panels / cancel modes / resume pause)

use anno_audio::engine::AudioEngine;
use anno_game::game_commands::{can_place_building, demolish_building, PlaceOutcome};
use anno_game::scenario::init_simulation;
use anno_formats::cod::CodFile;
use anno_formats::col::parse_col;
use anno_formats::szs::{Island, IslandTile, ShipClass, SzsFile};
use anno_render::sprite::{SpriteCategory, SpriteManager};
use anno_sim::ai::AiController;
use anno_sim::building::BuildingInstance;
use anno_sim::combat::{Diplomacy, UnitType};
use anno_sim::data_bridge;
use anno_sim::entity::{ActionType, CargoRoute};
use anno_sim::island_map::IslandMap;
use anno_sim::player::Player;
use anno_sim::simulation::{Simulation, TileClear};
use anno_sim::trade::TradeShipClass;
use anno_sim::types::Good;
use anno_sim::warehouse::Warehouse;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;

const WINDOW_W: u32 = 1280;
const WINDOW_H: u32 = 800;
const BG_COLOR: (u8, u8, u8) = (0x10, 0x20, 0x40);
const DIPLOMACY_PANEL_HELP: &str = "Up/Dn pick  Lt/Rt cycle";

const ZOOM_TILE_W: [i32; 3] = [64, 32, 16];
const ZOOM_TILE_H: [i32; 3] = [31, 15, 7];

/// Networking role chosen at startup.
enum NetRole {
    Solo,
    Host { port: u16 },
    Client { addr: String },
}

fn net_role_port(role: &NetRole) -> u16 {
    if let NetRole::Host { port } = role {
        *port
    } else {
        0
    }
}

/// Tiny 4x5 bitmap font for HUD rendering (ASCII 32-127).
/// Each character is a u32 bitmask: 4 columns × 5 rows, bit 0 = top-left.
mod tiny_font {
    const CHAR_W: u32 = 4;
    const CHAR_H: u32 = 5;

    /// Bitmap glyphs for ASCII 32-90 (space through Z). Others fallback to '?'.
    const GLYPHS: &[(u8, u32)] = &[
        (b' ', 0x00000),
        (b'0', 0x69BD6),
        (b'1', 0x46224),
        (b'2', 0x69246),
        (b'3', 0x69496),
        (b'4', 0x99F11),
        (b'5', 0xF8E1E),
        (b'6', 0x68E96),
        (b'7', 0xF1244),
        (b'8', 0x69696),
        (b'9', 0x69716),
        (b':', 0x04040),
        (b'A', 0x69F99),
        (b'B', 0xE9E9E),
        (b'C', 0x78867),
        (b'D', 0xE9996 + 1 - 1),
        (b'E', 0xF8E8F),
        (b'F', 0xF8E88),
        (b'G', 0x78B97),
        (b'H', 0x99F99),
        (b'I', 0xE444E),
        (b'J', 0x11196),
        (b'K', 0x9ACA9),
        (b'L', 0x8888F),
        (b'M', 0x9F999),
        (b'N', 0x9DB99),
        (b'O', 0x69996),
        (b'P', 0xE9E88),
        (b'Q', 0x699A7),
        (b'R', 0xE9EA9),
        (b'S', 0x78617),
        (b'T', 0xF4444),
        (b'U', 0x99996),
        (b'V', 0x9996A + 1 - 1),
        (b'W', 0x999F9),
        (b'X', 0x96699),
        (b'Y', 0x99644),
        (b'Z', 0xF1248 + 7),
        (b'a', 0x06996),
        (b'b', 0x8E996 + 1 - 1),
        (b'c', 0x07896 + 1 - 1),
        (b'd', 0x17996 + 1 - 1),
        (b'e', 0x06F87),
        (b'%', 0x91249),
        (b'+', 0x04E40),
        (b'-', 0x00E00),
        (b'/', 0x11248),
        (b'.', 0x00004),
        (b',', 0x00024),
        (b'=', 0x0E0E0),
        (b'?', 0x69240),
        (b'(', 0x24842),
        (b')', 0x42124),
        (b'|', 0x44444),
        (b'x', 0x09690),
    ];

    fn glyph(ch: u8) -> u32 {
        for &(c, g) in GLYPHS {
            if c == ch {
                return g;
            }
        }
        // Uppercase fallback
        if ch >= b'a' && ch <= b'z' {
            let upper = ch - 32;
            for &(c, g) in GLYPHS {
                if c == upper {
                    return g;
                }
            }
        }
        0x69240 // '?' fallback
    }

    /// Draw a string onto an RGBA buffer. Returns width in pixels consumed.
    pub fn draw_str(
        buf: &mut [u8],
        buf_w: u32,
        buf_h: u32,
        x: i32,
        y: i32,
        text: &str,
        color: [u8; 4],
        scale: u32,
    ) {
        let mut cx = x;
        for &ch in text.as_bytes() {
            let g = glyph(ch);
            for row in 0..CHAR_H {
                for col in 0..CHAR_W {
                    let bit = row * CHAR_W + col;
                    if (g >> bit) & 1 != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = cx + (col * scale + sx) as i32;
                                let py = y + (row * scale + sy) as i32;
                                if px >= 0 && py >= 0 && (px as u32) < buf_w && (py as u32) < buf_h
                                {
                                    let off = ((py as u32 * buf_w + px as u32) * 4) as usize;
                                    if off + 3 < buf.len() {
                                        let a = color[3] as u16;
                                        let inv_a = 255 - a;
                                        buf[off] = ((color[0] as u16 * a + buf[off] as u16 * inv_a)
                                            / 255)
                                            as u8;
                                        buf[off + 1] = ((color[1] as u16 * a
                                            + buf[off + 1] as u16 * inv_a)
                                            / 255)
                                            as u8;
                                        buf[off + 2] = ((color[2] as u16 * a
                                            + buf[off + 2] as u16 * inv_a)
                                            / 255)
                                            as u8;
                                        buf[off + 3] = 255;
                                    }
                                }
                            }
                        }
                    }
                }
                cx += 0; // row loop doesn't advance cursor
            }
            cx += (CHAR_W * scale + scale) as i32; // advance by char width + 1px spacing
        }
    }

    /// Measure text width in pixels.
    pub fn measure(text: &str, scale: u32) -> u32 {
        if text.is_empty() {
            return 0;
        }
        let chars = text.len() as u32;
        chars * (CHAR_W * scale + scale) - scale
    }
}

/// A building type available for placement.
struct BuildableBuilding {
    /// Index into building_defs / cod.buildings.
    def_idx: usize,
    /// Display name for the UI.
    name: String,
    /// Sprite index (gfx from COD) for rendering.
    sprite_idx: usize,
    /// Category bucket (drives the palette tabs).
    category: BuildCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BuildCategory {
    Production = 0,
    Residence = 1,
    Service = 2,
    Military = 3,
    Special = 4,
}

impl BuildCategory {
    const ALL: [BuildCategory; 5] = [
        BuildCategory::Production,
        BuildCategory::Residence,
        BuildCategory::Service,
        BuildCategory::Military,
        BuildCategory::Special,
    ];

    fn label(self) -> &'static str {
        match self {
            BuildCategory::Production => "PROD",
            BuildCategory::Residence => "RES",
            BuildCategory::Service => "SVC",
            BuildCategory::Military => "MIL",
            BuildCategory::Special => "SPC",
        }
    }

    fn from_def(def: &anno_sim::building::BuildingDef) -> Self {
        let pk = def.prod_kind.as_str();
        if matches!(
            pk,
            "MARKT"
                | "KIRCHE"
                | "KAPELLE"
                | "SCHULE"
                | "HOCHSCHULE"
                | "WIRT"
                | "THEATER"
                | "ARZT"
                | "BADEHAUS"
                | "GALGEN"
                | "KLINIK"
        ) {
            return BuildCategory::Service;
        }
        if matches!(pk, "MILITAR") {
            return BuildCategory::Military;
        }
        if matches!(pk, "KONTOR") || def.kind.as_str() == "KONTOR" || def.kind.as_str() == "HQ" {
            return BuildCategory::Special;
        }
        if def.kind.as_str() == "WOHN" || matches!(pk, "WOHN") {
            return BuildCategory::Residence;
        }
        BuildCategory::Production
    }
}

/// Building placement state machine.
struct BuildingPlacer {
    active: bool,
    /// Available buildings to place.
    buildable: Vec<BuildableBuilding>,
    /// Currently selected building index (into buildable vec).
    selected: usize,
    /// Current page of buildings (9 per page) within the active category.
    page: usize,
    /// Active category tab.
    category: BuildCategory,
    /// Rotation slot 0..rotate-1 picked at placement time.
    orientation: u8,
    /// Tile coordinates under the mouse cursor (if valid).
    hover_tile: Option<(i32, i32)>,
}

impl BuildingPlacer {
    fn new(cod: &CodFile, defs: &[anno_sim::building::BuildingDef]) -> Self {
        let mut buildable = Vec::new();

        for (i, cod_b) in cod.buildings.iter().enumerate() {
            if i >= defs.len() {
                break;
            }
            let def = &defs[i];

            // Only allow placing actual buildings (not terrain, decorations, etc.)
            let dominated_kind = match def.kind.as_str() {
                "GEBAEUDE" | "HQ" => true,
                _ => false,
            };
            let has_production = def.output_good != Good::None;
            let is_service = matches!(
                def.prod_kind.as_str(),
                "MARKT"
                    | "KIRCHE"
                    | "KAPELLE"
                    | "SCHULE"
                    | "WIRT"
                    | "THEATER"
                    | "ARZT"
                    | "BADEHAUS"
                    | "GALGEN"
            );
            let is_military = matches!(def.prod_kind.as_str(), "MILITAR");
            let is_kontor = def.kind.as_str() == "KONTOR" || def.prod_kind.as_str() == "KONTOR";

            if !dominated_kind && !has_production && !is_service && !is_military && !is_kontor {
                continue;
            }

            // Must have a valid sprite
            if cod_b.gfx < 0 {
                continue;
            }

            // Must have a size > 0
            if def.width == 0 || def.height == 0 {
                continue;
            }

            let name = cod_b
                .properties
                .get("Name")
                .cloned()
                .unwrap_or_else(|| format!("Building #{}", cod_b.nummer));

            buildable.push(BuildableBuilding {
                def_idx: i,
                name,
                sprite_idx: cod_b.gfx as usize,
                category: BuildCategory::from_def(def),
            });
        }

        let initial_selected = buildable
            .iter()
            .position(|b| b.category == BuildCategory::Production)
            .unwrap_or(0);
        Self {
            active: false,
            buildable,
            selected: initial_selected,
            page: 0,
            category: BuildCategory::Production,
            orientation: 0,
            hover_tile: None,
        }
    }

    fn category_indices(&self) -> Vec<usize> {
        self.buildable
            .iter()
            .enumerate()
            .filter(|(_, b)| b.category == self.category)
            .map(|(i, _)| i)
            .collect()
    }

    fn next_category(&mut self) {
        let cats = BuildCategory::ALL;
        let idx = cats.iter().position(|c| *c == self.category).unwrap_or(0);
        self.category = cats[(idx + 1) % cats.len()];
        self.page = 0;
        if let Some(&first) = self.category_indices().first() {
            self.selected = first;
        }
    }

    fn prev_category(&mut self) {
        let cats = BuildCategory::ALL;
        let idx = cats.iter().position(|c| *c == self.category).unwrap_or(0);
        self.category = cats[(idx + cats.len() - 1) % cats.len()];
        self.page = 0;
        if let Some(&first) = self.category_indices().first() {
            self.selected = first;
        }
    }

    fn toggle(&mut self) {
        self.active = !self.active;
        if self.active {
            self.hover_tile = None;
        }
    }

    fn selected_building(&self) -> Option<&BuildableBuilding> {
        if !self.active || self.buildable.is_empty() {
            return None;
        }
        self.buildable.get(self.selected)
    }

    /// Indices into `buildable` for the current category page (up to 9).
    fn page_index_slice(&self) -> Vec<usize> {
        let cat = self.category_indices();
        let start = self.page * 9;
        let end = (start + 9).min(cat.len());
        if start >= cat.len() {
            Vec::new()
        } else {
            cat[start..end].to_vec()
        }
    }

    fn select_on_page(&mut self, slot: usize) {
        let cat = self.category_indices();
        let idx = self.page * 9 + slot;
        if let Some(&buildable_idx) = cat.get(idx) {
            self.selected = buildable_idx;
            // Reset orientation; the new building may have a different
            // rotation count, so the previous index could overflow.
            self.orientation = 0;
        }
    }

    fn page_count(&self) -> usize {
        let n = self.category_indices().len();
        if n == 0 {
            0
        } else {
            (n - 1) / 9 + 1
        }
    }

    fn next_page(&mut self) {
        let max_page = self.page_count().saturating_sub(1);
        self.page = (self.page + 1).min(max_page);
    }

    fn prev_page(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
}

/// Convert screen pixel coordinates to isometric tile coordinates.
/// Returns (tile_x, tile_y) relative to the island origin.
fn screen_to_tile(
    screen_x: i32,
    screen_y: i32,
    origin_x: i32,
    origin_y: i32,
    tile_w: i32,
    tile_h: i32,
) -> (i32, i32) {
    // Inverse of: sx = origin_x + (tx - ty) * half_tw
    //             sy = origin_y + (tx + ty) * half_th
    // So: tx - ty = (sx - origin_x) / half_tw
    //     tx + ty = (sy - origin_y) / half_th
    // => tx = ((sx - origin_x) / half_tw + (sy - origin_y) / half_th) / 2
    //    ty = ((sy - origin_y) / half_th - (sx - origin_x) / half_tw) / 2
    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;
    if half_tw == 0 || half_th == 0 {
        return (0, 0);
    }

    // Use fixed-point to avoid rounding issues
    let dx = screen_x - origin_x;
    let dy = screen_y - origin_y;

    // Multiply through to avoid division until the end
    let sum = dx * half_th + dy * half_tw; // proportional to (tx - ty + tx + ty) = 2*tx
    let diff = dy * half_tw - dx * half_th; // proportional to (tx + ty - tx + ty) = 2*ty

    let denom = 2 * half_tw * half_th;

    // Round toward nearest tile
    let tx = if sum >= 0 {
        (sum + denom / 2) / denom
    } else {
        (sum - denom / 2) / denom
    };
    let ty = if diff >= 0 {
        (diff + denom / 2) / denom
    } else {
        (diff - denom / 2) / denom
    };

    (tx, ty)
}

fn scroll_for_island_tile(
    island: &Island,
    sprite_zoom: usize,
    display_zoom: i32,
    tile_x: i32,
    tile_y: i32,
) -> (i32, i32) {
    let tile_w = ZOOM_TILE_W[sprite_zoom];
    let tile_h = ZOOM_TILE_H[sprite_zoom];
    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;
    let img_w = ((island.width as i32 + island.height as i32) * half_tw) + tile_w;
    let img_h = ((island.width as i32 + island.height as i32) * half_th) + tile_h + 500;
    let origin_x = island.height as i32 * half_tw;
    let origin_y = 300;
    let tex_x = origin_x + (tile_x - tile_y) * half_tw + half_tw;
    let tex_y = origin_y + (tile_x + tile_y) * half_th + half_th;
    (
        img_w * display_zoom / 2 - tex_x * display_zoom,
        img_h * display_zoom / 2 - tex_y * display_zoom,
    )
}

fn troop_assembly_slot(key: Keycode) -> Option<usize> {
    match key {
        Keycode::Num1 => Some(0),
        Keycode::Num2 => Some(1),
        Keycode::Num3 => Some(2),
        Keycode::Num4 => Some(3),
        Keycode::Num5 => Some(4),
        Keycode::Num6 => Some(5),
        Keycode::Num7 => Some(6),
        Keycode::Num8 => Some(7),
        Keycode::Num9 => Some(8),
        _ => None,
    }
}

/// Check if a building can be placed at the given tile position on an island.
fn fertility_label(fertility: anno_formats::szs::Fertility) -> &'static str {
    use anno_formats::szs::Fertility;
    match fertility {
        Fertility::Grain => "Grain",
        Fertility::Tobacco => "Tobacco",
        Fertility::Spices => "Spices",
        Fertility::Sugarcane => "Sugarcane",
        Fertility::Cotton => "Cotton",
        Fertility::Vines => "Vines",
        Fertility::Cocoa => "Cocoa",
    }
}

fn fertility_list_label(island: &Island) -> String {
    let fertilities = island.active_fertilities();
    if fertilities.is_empty() {
        "none".into()
    } else {
        fertilities
            .into_iter()
            .map(fertility_label)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CityListRow {
    name: String,
    owner: u8,
    island_number: u8,
    population: u32,
}

fn visible_city_list_rows(
    islands: &[Island],
    diplomacy: &anno_sim::combat::DiplomacyMatrix,
) -> Vec<CityListRow> {
    islands
        .iter()
        .filter_map(|island| {
            let city = island.city.as_ref()?;
            if city.name.trim().is_empty() {
                return None;
            }
            if city.owner_slot != 0 && !diplomacy.has_trade_agreement(0, city.owner_slot) {
                return None;
            }
            Some(CityListRow {
                name: city.name.clone(),
                owner: city.owner_slot,
                island_number: island.number,
                population: city.tier_population.iter().sum(),
            })
        })
        .collect()
}

fn truncate_city_name(name: &str, max_chars: usize) -> String {
    let mut out: String = name.chars().take(max_chars).collect();
    if name.chars().count() > max_chars && max_chars > 0 {
        out.pop();
        out.push('~');
    }
    out
}

fn city_list_line(row: &CityListRow) -> String {
    let name = truncate_city_name(&row.name, 20);
    format!(
        "{:<20} p{} {:>5} i{}",
        name, row.owner, row.population, row.island_number
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShipListRow {
    name: String,
    kind: &'static str,
    status: &'static str,
    warship: bool,
}

fn trade_ship_class_label(class: TradeShipClass) -> &'static str {
    match class {
        TradeShipClass::SmallTrader => "small trader",
        TradeShipClass::LargeTrader => "large trader",
    }
}

fn trade_ship_state_label(state: anno_sim::trade::ShipState) -> &'static str {
    match state {
        anno_sim::trade::ShipState::Sailing => "sailing",
        anno_sim::trade::ShipState::Trading => "trading",
        anno_sim::trade::ShipState::Waiting => "waiting",
        anno_sim::trade::ShipState::Idle => "idle",
    }
}

fn naval_unit_label(unit_type: UnitType) -> &'static str {
    match unit_type {
        UnitType::SmallWarship => "small warship",
        UnitType::LargeWarship => "large warship",
        UnitType::PirateShip => "pirate ship",
        _ => "ship",
    }
}

fn visible_ship_list_rows(
    trade_ships: &[anno_sim::trade::TradeShip],
    military_units: &[anno_sim::combat::MilitaryUnit],
) -> Vec<ShipListRow> {
    let mut rows = Vec::new();
    for (idx, ship) in trade_ships
        .iter()
        .enumerate()
        .filter(|(_, ship)| ship.active && ship.owner == 0)
    {
        let name = if ship.name.trim().is_empty() {
            format!("Ship {}", idx + 1)
        } else {
            ship.name.clone()
        };
        rows.push(ShipListRow {
            name,
            kind: trade_ship_class_label(ship.class),
            status: trade_ship_state_label(ship.state),
            warship: false,
        });
    }
    for (idx, unit) in military_units
        .iter()
        .enumerate()
        .filter(|(_, unit)| unit.owner == 0 && unit.is_alive() && unit.unit_type.stats().is_naval)
    {
        let kind = naval_unit_label(unit.unit_type);
        let name = if unit.name.trim().is_empty() {
            format!("Ship {}", idx + 1)
        } else {
            unit.name.clone()
        };
        rows.push(ShipListRow {
            name,
            kind,
            status: "ready",
            warship: true,
        });
    }
    rows
}

fn ship_list_line(row: &ShipListRow) -> String {
    let name = truncate_city_name(&row.name, 22);
    format!("{:<22} {:<13} {}", name, row.kind, row.status)
}

/// Attempt to place the placer's currently-selected building at
/// `(tile_x, tile_y)` on `current_island`. Thin UI wrapper over
/// `game_commands::place_building`: resolves the placer's selection to a
/// def index + orientation, applies it, and records the equivalent
/// `Command::PlaceBuilding` when a recorder is active.
#[allow(clippy::too_many_arguments)]
fn try_place_building(
    sim: &mut anno_sim::simulation::Simulation,
    islands: &mut [Island],
    current_island: usize,
    defs: &[anno_sim::building::BuildingDef],
    cod: &CodFile,
    placer: &BuildingPlacer,
    tile_x: i32,
    tile_y: i32,
    recorder: &mut Option<anno_sim::replay::Recorder>,
) -> PlaceOutcome {
    let bb = match placer.selected_building() {
        Some(b) => b,
        None => return PlaceOutcome::NoBuildingSelected,
    };
    let outcome = anno_game::game_commands::place_building(
        sim,
        islands,
        current_island,
        defs,
        cod,
        bb.def_idx,
        placer.orientation,
        0,
        tile_x,
        tile_y,
    );
    if matches!(outcome, PlaceOutcome::Placed) {
        if let Some(rec) = recorder.as_mut() {
            rec.record(
                sim.game_clock,
                anno_sim::commands::Command::PlaceBuilding {
                    player: 0,
                    island: islands[current_island].number,
                    tile_x: tile_x as u16,
                    tile_y: tile_y as u16,
                    def_index: bb.def_idx as u16,
                    orientation: placer.orientation,
                },
            );
        }
    }
    outcome
}

fn push_ruin_tiles(island: &mut Island, cod: &CodFile, clear: TileClear) {
    if clear.ruin_id == anno_sim::building::NO_RUIN_ID {
        return;
    }

    let Some(base_ruin) = cod.ruin_building(clear.ruin_id, clear.ruin_uses_strand_table) else {
        return;
    };

    let ruin_size = if matches!(clear.source_orientation & 3, 1 | 3) {
        (base_ruin.size.1, base_ruin.size.0)
    } else {
        base_ruin.size
    };
    if ruin_size == (clear.width as i32, clear.height as i32) {
        let Some(&rand_value) = clear.source_ruin_draws.first() else {
            return;
        };
        let Some(ruin) =
            cod.ruin_variant_building(clear.ruin_id, clear.ruin_uses_strand_table, rand_value)
        else {
            return;
        };
        let definition_offset = ruin
            .source_id
            .checked_sub(anno_formats::szs::INSELHAUS_SOURCE_ID_BASE)
            .and_then(|offset| u16::try_from(offset).ok());
        let Some(definition_offset) = definition_offset else {
            return;
        };
        let command = anno_sim::building::SourceBuildingCommand {
            definition_offset,
            orientation: clear.source_orientation,
            variant: clear.source_variant,
            metadata: 0,
            map_owner_slot: clear.source_map_owner_slot,
            random_seed: 0,
            dynamic_object_owner: 0,
        };
        let (Ok(x), Ok(y)) = (u8::try_from(clear.tile_x), u8::try_from(clear.tile_y)) else {
            return;
        };
        island.tiles.push(command.to_island_tile(x, y));
        return;
    }

    for dy in 0..clear.height {
        for dx in (0..clear.width).rev() {
            let x = clear.tile_x + dx as u16;
            let y = clear.tile_y + dy as u16;
            let draw_index =
                usize::from(dy) * usize::from(clear.width) + usize::from(clear.width - 1 - dx);
            let Some(&rand_value) = clear.source_ruin_draws.get(draw_index) else {
                continue;
            };
            let Some(ruin) = cod.ruin_variant_building(
                clear.ruin_id,
                clear.fallback_uses_strand_table(draw_index),
                rand_value,
            ) else {
                continue;
            };
            let definition_offset = ruin
                .source_id
                .checked_sub(anno_formats::szs::INSELHAUS_SOURCE_ID_BASE)
                .and_then(|offset| u16::try_from(offset).ok());
            let (Some(definition_offset), Ok(x), Ok(y)) =
                (definition_offset, u8::try_from(x), u8::try_from(y))
            else {
                continue;
            };
            island.tiles.push(
                anno_sim::building::SourceBuildingCommand {
                    definition_offset,
                    orientation: clear.source_orientation,
                    variant: clear.source_variant,
                    metadata: 0,
                    map_owner_slot: clear.source_map_owner_slot,
                    random_seed: 0,
                    dynamic_object_owner: 0,
                }
                .to_island_tile(x, y),
            );
        }
    }
}

fn apply_tile_clear_event(islands: &mut [Island], cod: &CodFile, clear: TileClear) {
    let Some(island) = islands.iter_mut().find(|i| i.number == clear.island_id) else {
        return;
    };

    let right = clear.tile_x + clear.width as u16;
    let bottom = clear.tile_y + clear.height as u16;
    island.tiles.retain(|tile| {
        let in_footprint = tile.x as u16 >= clear.tile_x
            && (tile.x as u16) < right
            && tile.y as u16 >= clear.tile_y
            && (tile.y as u16) < bottom;
        !in_footprint
    });

    push_ruin_tiles(island, cod, clear);
}

/// Replay the root replacement emitted by `FUN_0047c080`. The source first
/// overwrites the old housing command at this anchor, then installs the
/// selected BGruppe command; retaining one INSELHAUS root per anchor gives
/// `IslandMap::from_island` the same final-footprint replay order.
fn apply_kind13_replacement_command(
    islands: &mut [Island],
    replacement: anno_sim::simulation::SourceKind13ReplacementCommand,
) -> bool {
    let Some(island) = islands
        .iter_mut()
        .find(|island| island.number == replacement.island_id)
    else {
        return false;
    };
    island
        .tiles
        .retain(|tile| (tile.x, tile.y) != (replacement.tile_x, replacement.tile_y));
    island.tiles.push(
        replacement
            .command
            .to_island_tile(replacement.tile_x, replacement.tile_y),
    );
    true
}


/// Materialize the roots rebuilt by `FUN_004641d0` after a `NORUINE`
/// terminal event. The static-cell table has already applied the source
/// writer's overwrite sequence; emitting only command anchors reconstructs
/// the INSELHAUS records the renderer consumes.
fn push_no_ruin_backing_tiles(
    islands: &mut [Island],
    static_cells: &[anno_sim::source_cell::SourceMapCellState],
    clear: &TileClear,
) {
    if clear.ruin_id != anno_sim::building::NO_RUIN_ID {
        return;
    }
    let Some(island) = islands
        .iter_mut()
        .find(|island| island.number == clear.island_id)
    else {
        return;
    };
    let right = clear.tile_x.saturating_add(u16::from(clear.width));
    let bottom = clear.tile_y.saturating_add(u16::from(clear.height));
    let mut roots: Vec<_> = static_cells
        .iter()
        .copied()
        .filter(|cell| {
            cell.island == clear.island_id
                && cell.source_command_anchor_x == cell.x
                && cell.source_command_anchor_y == cell.y
                && u16::from(cell.x) >= clear.tile_x
                && u16::from(cell.x) < right
                && u16::from(cell.y) >= clear.tile_y
                && u16::from(cell.y) < bottom
        })
        .collect();
    roots.sort_by_key(|cell| (cell.y, std::cmp::Reverse(cell.x)));
    island
        .tiles
        .extend(roots.into_iter().map(|cell| cell.to_source_island_tile()));
}

/// Rebuild the local path and terrain replay after a terminal map write has
/// changed an island's INSELHAUS command stream.
fn refresh_simulation_island_map(
    sim: &mut Simulation,
    islands: &[Island],
    cod: &CodFile,
    island_id: u8,
) {
    let Some(island) = islands.iter().find(|island| island.number == island_id) else {
        return;
    };
    let Some(map) = sim
        .island_maps
        .iter_mut()
        .find(|map| map.island_id == island_id)
    else {
        return;
    };
    let source_resource_state = map.source_resource_state();
    let source_runtime_classification = map.source_runtime_classification();
    *map = IslandMap::from_island(island, &cod.buildings)
        .with_source_runtime_classification(source_runtime_classification)
        .with_source_resource_state(source_resource_state);
}

fn main() {
    let base_dir = find_data_dir();

    // Load palette
    let col_data =
        std::fs::read(base_dir.join("TOOLGFX/STADTFLD.COL")).expect("Failed to read STADTFLD.COL");
    let palette = parse_col(&col_data).expect("Failed to parse palette");

    // Load sprites
    println!("Loading sprites...");
    let sprite_mgr = SpriteManager::load_from_dir(&base_dir);
    let sprites_by_zoom: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_sprites(&sprite_mgr, z, &palette))
        .collect();
    for (z, sprites) in sprites_by_zoom.iter().enumerate() {
        let label = ["GFX", "MGFX", "SGFX"][z];
        println!("  {label}: {} decoded sprites", sprites.len());
    }

    // Decode entity sprite sets (carriers, soldiers, ships)
    let carrier_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Traeger, z, &palette))
        .collect();
    let worker_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Maeher, z, &palette))
        .collect();
    let ship_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Ship, z, &palette))
        .collect();
    let soldier_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Soldat, z, &palette))
        .collect();
    let shadow_sprites: Vec<Vec<(u32, u32, Vec<u8>)>> = (0..3)
        .map(|z| decode_entity_sprites(&sprite_mgr, SpriteCategory::Schatten, z, &palette))
        .collect();
    println!(
        "Entity sprites: carriers={} workers={} ships={} soldiers={} shadows={}",
        carrier_sprites[0].len(),
        worker_sprites[0].len(),
        ship_sprites[0].len(),
        soldier_sprites[0].len(),
        shadow_sprites[0].len(),
    );

    // Load building definitions
    let cod_data = std::fs::read(base_dir.join("haeuser.cod")).expect("Failed to read haeuser.cod");
    let cod = CodFile::parse(&cod_data).expect("Failed to parse COD");
    let defs = data_bridge::load_building_defs(&cod);
    println!("Loaded {} building definitions", defs.len());

    // Load figure definitions (carriers, ships, soldiers, …) so sprite
    // indexing can use the real per-figure (gfx, rotate, anim) layout
    // instead of a one-size-fits-all heuristic.
    let figures_path = base_dir.join("figuren.cod");
    let figures = match std::fs::read(&figures_path) {
        Ok(bytes) => {
            let f = anno_formats::figuren::FiguresFile::parse(&bytes);
            println!(
                "Loaded {} figure definitions from {}",
                f.figures.len(),
                figures_path.display(),
            );
            f
        }
        Err(e) => {
            eprintln!("(figuren.cod not loaded: {e}) — using heuristic sprite indexing");
            anno_formats::figuren::FiguresFile {
                constants: Default::default(),
                figures: Vec::new(),
            }
        }
    };
    // Pull the layout bits we'll use during render. Falls back to the old
    // heuristic values if the figure can't be found.
    let traeger_def = figures.find("TRAEGER").cloned();
    let karren_def = figures.find("KARREN").cloned();
    let traeger2_def = figures.find("TRAEGER2").cloned();
    let handel1_def = figures
        .find(ShipClass::SmallTrader.source_figure_name())
        .cloned();
    let handel2_def = figures
        .find(ShipClass::LargeTrader.source_figure_name())
        .cloned();
    let handler_def = figures.find("HANDLER").cloned();
    let krieg1_def = figures
        .find(ShipClass::SmallWarship.source_figure_name())
        .cloned();
    let krieg2_def = figures
        .find(ShipClass::LargeWarship.source_figure_name())
        .cloned();
    let pirat_def = figures
        .find(ShipClass::PirateShip.source_figure_name())
        .cloned();
    let ship_cargo_config = anno_sim::trade::ShipCargoConfig::from_figures(&figures);
    let ship_sprite_layout = ShipSpriteLayout::from_figure_defs(
        handel1_def.as_ref(),
        handel2_def.as_ref(),
        handler_def.as_ref(),
        krieg1_def.as_ref(),
        krieg2_def.as_ref(),
        pirat_def.as_ref(),
    );
    let soldier_sprite_layout = SoldierSpriteLayout::from_figures(&figures);
    let carrier_walk_anz = traeger_def
        .as_ref()
        .and_then(|f| f.walk_anim())
        .map(|a| a.anim_anz as usize)
        .unwrap_or(8);
    let carrier_empty_anim_offs = traeger_def
        .as_ref()
        .and_then(|f| f.walk_anim())
        .and_then(|a| usize::try_from(a.anim_offs).ok())
        .unwrap_or(0);
    let carrier_loaded_anim_offs = traeger_def
        .as_ref()
        .and_then(|f| f.anim(1))
        .and_then(|a| usize::try_from(a.anim_offs).ok())
        .unwrap_or(carrier_walk_anz * 8);
    let carrier_shadow_layout = carrier_shadow_layout_from_figure(
        figures.find("SCHATTEN"),
        CarrierShadowLayout::normal_default(),
    );
    let city_cart_shadow_layout = carrier_shadow_layout_from_figure(
        figures.find("SCHATTENLANG"),
        CarrierShadowLayout::long_default(),
    );
    let carrier_shadow_y_offset = traeger_def
        .as_ref()
        .map(|figure| figure.position_offset.1)
        .unwrap_or(5);
    let city_cart_shadow_y_offset = karren_def
        .as_ref()
        .map(|figure| figure.position_offset.1)
        .unwrap_or(5);
    let civilian_shadow_y_offset = figures
        .find("ADELWEIBL")
        .map(|figure| figure.position_offset.1)
        .unwrap_or(5);
    let civilian_config = anno_sim::civilian::CivilianConfig::from_figures(&figures);
    let _ = &figures;

    // Parse CLI: positional scenario path + optional --host PORT /
    // --join HOST:PORT / --record FILE.
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let mut net_role: NetRole = NetRole::Solo;
    let mut record_path: Option<std::path::PathBuf> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw_args.len() {
        let a = &raw_args[i];
        if a == "--host" {
            i += 1;
            let port: u16 = raw_args
                .get(i)
                .and_then(|p| p.parse().ok())
                .expect("--host needs a port number");
            net_role = NetRole::Host { port };
        } else if a == "--join" {
            i += 1;
            let addr = raw_args.get(i).cloned().expect("--join needs HOST:PORT");
            net_role = NetRole::Client { addr };
        } else if a == "--record" {
            i += 1;
            let path = raw_args.get(i).cloned().expect("--record needs a file path");
            record_path = Some(std::path::PathBuf::from(path));
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }

    // Load scenario
    let scenario_path = positional.into_iter().next().unwrap_or_else(|| {
        let szenes = base_dir.join("Szenes");
        let mut entries: Vec<_> = std::fs::read_dir(&szenes)
            .expect("Failed to read Szenes/")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".szs"))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries
            .first()
            .map(|e| e.path().to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                eprintln!("No .szs files found");
                std::process::exit(1);
            })
    });

    let szs_data = std::fs::read(&scenario_path).expect("Failed to read scenario");
    let mut szs = SzsFile::parse(&szs_data).expect("Failed to parse scenario");
    anno_game::scenario::instantiate_stock_islands(&mut szs, std::path::Path::new("extracted"), 1);
    let szs = szs;
    let scenario_name = std::path::Path::new(&scenario_path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    println!(
        "Loaded scenario '{}': {} islands",
        scenario_name,
        szs.islands.len()
    );
    if let Some(p0) = szs.players.first() {
        if !p0.name.is_empty() {
            println!("Player: {}  (starting gold {})", p0.name, p0.starting_gold);
        }
    }
    {
        let m = &szs.scenario;
        if m.mission_nr.is_some() || m.ranking.is_some() {
            println!(
                "Scenario meta: mission #{} players {}-{} ranking {}",
                m.mission_nr
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                m.player_min
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                m.player_max
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                m.ranking
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
            );
        }
    }
    for island in &szs.islands {
        // Print every island with at least one fertility OR a
        // settled city — surfaces the new INSEL5 fertility map.
        let active = island.active_fertilities();
        let fert_names: Vec<String> = active.iter().map(|f| format!("{f:?}")).collect();
        if let Some(city) = island.city.as_ref() {
            if !city.name.is_empty() {
                let fert_tag = if fert_names.is_empty() {
                    String::new()
                } else {
                    format!("  fertilities=[{}]", fert_names.join(", "))
                };
                println!(
                    "  Island {}: city '{}' (owner_slot {}, island_index {}){fert_tag}",
                    island.number, city.name, city.owner_slot, city.island_index
                );
            }
        } else if !fert_names.is_empty() {
            println!(
                "  Island {} (uninhabited): fertilities=[{}]",
                island.number,
                fert_names.join(", ")
            );
        }
    }
    if !szs.ships.is_empty() {
        // Tally ship classes so the user sees the fleet shape
        // (warship vs trader vs pirate) at a glance, plus the
        // named ships separately.
        use anno_formats::szs::ShipClass;
        let mut tally: std::collections::BTreeMap<ShipClass, u32> = Default::default();
        for s in &szs.ships {
            if let Some(c) = s.class() {
                *tally.entry(c).or_default() += 1;
            }
        }
        let named: Vec<&str> = szs
            .ships
            .iter()
            .filter(|s| !s.name.is_empty())
            .map(|s| s.name.as_str())
            .collect();
        if !named.is_empty() {
            let class_summary: Vec<String> =
                tally.iter().map(|(c, n)| format!("{n}× {c:?}")).collect();
            println!(
                "Starting ships ({}): {} [{}]",
                named.len(),
                named.join(", "),
                class_summary.join(", ")
            );
        }
    }
    if let Some(mission) = szs.mission.as_ref() {
        if !mission.briefing.is_empty() {
            use anno_formats::szs::{
                MISSION_FLAG_COOPERATIVE, MISSION_FLAG_PIRATE, MISSION_FLAG_POPULATION,
                MISSION_FLAG_POPULATION2, MISSION_FLAG_POPULATION3, MISSION_FLAG_RANKING,
            };
            // Enumerate every set bit and label the known ones —
            // unrecognised bits print as `unknown(0x...)` so a new
            // scenario flips them into view at run time.
            let mut tags: Vec<String> = Vec::new();
            let mut seen = 0u32;
            let tag = |bit: u32, name: &str, seen: &mut u32, tags: &mut Vec<String>| {
                if mission.flags & bit != 0 {
                    tags.push(name.to_string());
                    *seen |= bit;
                }
            };
            tag(MISSION_FLAG_POPULATION, "population", &mut seen, &mut tags);
            tag(
                MISSION_FLAG_POPULATION2,
                "population2",
                &mut seen,
                &mut tags,
            );
            tag(
                MISSION_FLAG_POPULATION3,
                "population3",
                &mut seen,
                &mut tags,
            );
            tag(
                MISSION_FLAG_COOPERATIVE,
                "cooperative",
                &mut seen,
                &mut tags,
            );
            tag(MISSION_FLAG_RANKING, "ranking", &mut seen, &mut tags);
            tag(MISSION_FLAG_PIRATE, "pirate-combat", &mut seen, &mut tags);
            let leftover = mission.flags & !seen;
            for bit in 0..32 {
                let mask = 1u32 << bit;
                if leftover & mask != 0 {
                    tags.push(format!("unknown(0x{mask:X})"));
                }
            }
            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", tags.join(", "))
            };
            println!("Mission flags 0x{:04x}{}", mission.flags, tag_str);
            let goals = mission.goals();
            if let Some(pop) = goals.primary_population() {
                let tier = match goals.primary_tier() {
                    Some(0) => " of Pioneer tier",
                    Some(1) => " of Settler tier",
                    Some(2) => " of Citizen tier",
                    Some(3) => " of Merchant tier",
                    Some(4) => " at Aristocrat tier",
                    _ => "",
                };
                println!("Goal: reach {} inhabitants{}", pop, tier);
            }
            if let Some(coop) = goals.cooperative_population {
                let tier = match goals.cooperative_tier {
                    Some(0) => " of Pioneer tier",
                    Some(1) => " of Settler tier",
                    Some(2) => " of Citizen tier",
                    Some(3) => " of Merchant tier",
                    Some(4) => " at Aristocrat tier",
                    _ => "",
                };
                println!("Goal: assist neighbour to {} inhabitants{}", coop, tier);
            }
            println!("---");
            println!("{}", mission.briefing.trim_end());
            println!("---");
        }
    }

    // Initialize simulation
    let mut sim = init_simulation(&szs, &cod, &defs, ship_cargo_config);
    sim.seed_source_rand_from_get_tick_count();
    let mut last_source_map_cell_revision = sim.source_map_cell_revision;
    if let Some(traeger) = traeger_def.as_ref() {
        sim.carrier_config = anno_sim::carrier::CarrierConfig::from_figure_def(traeger);
    }
    if let Some(karren) = karren_def.as_ref() {
        sim.city_cart_config = anno_sim::carrier::CityCartConfig::from_figure_def(karren);
    }
    if let Some(traeger2) = traeger2_def.as_ref() {
        sim.city_cart_traeger2_config =
            anno_sim::carrier::CityCartConfig::from_figure_def(traeger2);
    }
    sim.civilian_config = civilian_config;
    // Command recorder (--record FILE): snapshots the fully-initialized sim
    // and logs every player command so the run can be replayed tick-exactly
    // (headless --replay, or replay_advancing) for lockstep comparison.
    let mut recorder: Option<anno_sim::replay::Recorder> = record_path
        .as_ref()
        .map(|_| anno_sim::replay::Recorder::start(&sim));
    if let Some(path) = &record_path {
        println!("Recording commands to {}", path.display());
    }
    println!(
        "Simulation initialized: {} buildings, {} warehouses, {} island maps",
        sim.buildings.len(),
        sim.warehouses.len(),
        sim.island_maps.len()
    );

    // Load scenario objectives from AUFTRAG4. Scenarios with no
    // flagged goals stay objective-free instead of receiving a
    // generated tutorial checklist.
    if let Some(mission) = szs.mission.as_ref() {
        let g = mission.goals();
        sim.objectives = anno_sim::objectives::ObjectiveSet::from_mission_goals(&g);
    }

    // Initialize building placer
    let mut placer = BuildingPlacer::new(&cod, &defs);
    println!(
        "Building placer: {} buildable types",
        placer.buildable.len()
    );

    // Entity walk cycles are visual-only. Map-cell sprites take their frame
    // directly from the source cell-state selector in the STADTFLD draw loop.
    let mut entity_visual_elapsed_ms = 0u32;
    let mut last_entity_visual_gen = 0u32;

    // Mutable copy of islands for adding placed building tiles
    let mut islands = szs.islands.clone();

    // Initialize audio engine
    let audio_dirs = vec![
        base_dir.join("MUSIC8"),
        base_dir.join("SPEECH8"),
        base_dir.clone(),
    ];
    let mut audio = AudioEngine::new(audio_dirs);
    audio.set_screen_size(WINDOW_W, WINDOW_H);

    // Discover music tracks
    let music_dir = base_dir.join("MUSIC8");
    let mut music_files: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&music_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().ends_with(".wav") {
                music_files.push(name);
            }
        }
    }
    music_files.sort();
    println!("Found {} music tracks", music_files.len());

    // Load and start first music track
    let mut music_enabled = true;
    let mut music_volume: f32 = 0.4;
    let mut current_track: usize = 0;
    let mut music_slot: Option<usize> = None;

    if !music_files.is_empty() {
        if let Some(slot) = audio.streams.create(&music_files[0], 0) {
            if let Some(ref handle) = audio.stream_handle {
                audio.streams.play(slot, music_volume, handle);
                println!("Playing: {}", music_files[0]);
            }
            music_slot = Some(slot);
        }
    }

    // Load sound effects. Slots are reused from SPEECH8/MUSIC8 pools.
    let place_sound_slot = audio
        .waves
        .load("SPEECH8/1000.WAV")
        .or_else(|| audio.waves.load("1000.WAV"));
    let event_destroy_slot = audio
        .waves
        .load("SPEECH8/1010.WAV")
        .or_else(|| audio.waves.load("1010.WAV"))
        .or_else(|| audio.waves.load("SPEECH8/1000.WAV"));
    let event_obj_done_slot = audio
        .waves
        .load("SPEECH8/1020.WAV")
        .or_else(|| audio.waves.load("1020.WAV"))
        .or_else(|| audio.waves.load("SPEECH8/1000.WAV"));
    // Per-event sample slots. The original loads named WAV files from
    // `SAMPLES/` via `_MaxwaveLoad@4` at startup
    // (`decompiled/1602_exe.c:106397-106479`); the relevant ones are:
    //   - `event.wav`    → `DAT_005b5e4c` (generic alert ping; line 106460)
    //   - `piraten.wav`  → `_DAT_005b5eac` (pirate sighting / hostile,  106441)
    //   - `triumph.wav`  → `_DAT_005b5ebc` (success / treaty / victory, 106444)
    // These are the actual event audio cues the original used.
    // The numbered SPEECH8 WAVs are voice-line speech (e.g. citizens
    // demanding new goods); the per-event playback uses SAMPLES/*.wav.
    let voice_stockpile_slot = audio
        .waves
        .load("SAMPLES/event.wav")
        .or_else(|| audio.waves.load("SAMPLES/Event.wav"));
    let voice_treasury_slot = voice_stockpile_slot;
    let voice_trader_slot = audio
        .waves
        .load("SAMPLES/triumph.wav")
        .or_else(|| audio.waves.load("SAMPLES/Triumph.wav"))
        .or_else(|| voice_stockpile_slot);
    let voice_attack_slot = audio
        .waves
        .load("SAMPLES/piraten.wav")
        .or_else(|| audio.waves.load("SAMPLES/Piraten.wav"))
        .or_else(|| voice_stockpile_slot);
    // Volcano eruption — RE: `1602_exe.c:106445-447` loads
    // vulkan1.wav .. vulkan3.wav into DAT_005b5ed4..5edc; the
    // figuren.cod VULKAN figure plays one of them via `WAV_VULKAN1, 3`
    // (the `, 3` is the random-pick range). We pick #1 deterministically
    // since our event_log doesn't yet propagate the per-event RNG.
    let voice_volcano_slot = audio
        .waves
        .load("SAMPLES/vulkan1.wav")
        .or_else(|| audio.waves.load("SAMPLES/Vulkan1.wav"))
        .or_else(|| audio.waves.load("SAMPLES/vulkan2.wav"))
        .or_else(|| voice_stockpile_slot);
    // Fire routing is retained for future source-backed event logs. The
    // compiled `BRANDMARKT` definition is the inactive-production symbol,
    // not evidence for a fire event.
    let voice_fire_slot = voice_stockpile_slot;

    // SDL2 setup
    let sdl = sdl2::init().expect("SDL2 init failed");
    let video = sdl.video().expect("SDL2 video init failed");

    let window = video
        .window("Anno 1602 — Game", WINDOW_W, WINDOW_H)
        .position_centered()
        .resizable()
        .build()
        .expect("window creation failed");

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .expect("canvas creation failed");

    // Pin the logical drawing surface to WINDOW_W × WINDOW_H so panels
    // and HUD positions remain stable even when the user resizes the OS
    // window. SDL letterboxes / scales for us.
    canvas
        .set_logical_size(WINDOW_W, WINDOW_H)
        .expect("set_logical_size failed");

    let texture_creator = canvas.texture_creator();
    let mut event_pump = sdl.event_pump().expect("event pump failed");

    let mut current_island: usize = 0;
    let mut scroll_x: i32 = 0;
    let mut scroll_y: i32 = 0;
    let mut display_zoom: i32 = 1;
    let mut sprite_zoom: usize = 0;
    let mut needs_redraw = true;
    let mut world_mode = false;
    let mut dragging = false;
    let mut drag_start = (0i32, 0i32);

    let mut rendered: Option<RenderState> = None;

    let timer = sdl.timer().expect("timer init failed");
    let mut last_tick = timer.ticks();

    // Networking setup
    let mut net_host: Option<anno_net::transport::NetHost> = None;
    let mut net_client: Option<anno_net::transport::NetClient> = None;
    let mut net_status = String::from("solo");
    let mut last_broadcast_ms: u32 = 0;
    let broadcast_interval_ms: u32 = 1000;
    match net_role {
        NetRole::Host { port } => {
            let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
            match anno_net::transport::NetHost::bind(addr, "anno-game") {
                Ok(h) => {
                    net_host = Some(h);
                    net_status = format!("HOST :{port}");
                    println!("Hosting on port {port}");
                }
                Err(e) => {
                    eprintln!("Failed to host on {port}: {e}");
                    std::process::exit(1);
                }
            }
        }
        NetRole::Client { ref addr } => {
            let parsed: std::net::SocketAddr = addr.parse().unwrap_or_else(|e| {
                eprintln!("Bad --join addr '{addr}': {e}");
                std::process::exit(1);
            });
            match anno_net::transport::NetClient::connect(parsed, "anno-game") {
                Ok(c) => {
                    net_client = Some(c);
                    net_status = format!("CLIENT → {addr}");
                    sim.paused = true; // client doesn't tick locally
                    println!("Connected to {addr} (waiting for state…)");
                }
                Err(e) => {
                    eprintln!("Failed to connect to {addr}: {e}");
                    std::process::exit(1);
                }
            }
        }
        NetRole::Solo => {}
    }

    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut minimap_clicked = false;
    let mut minimap_click_x: i32 = 0;
    let mut minimap_click_y: i32 = 0;
    // Anno-style "click a building to see its info window". Left-click
    // on a non-empty tile sets this; the renderer paints a small info
    // card at top-left.
    let mut selected_building_idx: Option<usize> = None;
    // Mass-place: while LMB is held in placer mode, every new tile the
    // cursor enters gets a fresh placement attempt. Matches the
    // original's drag-place for roads and fields. `drag_placing` is
    // set on a successful initial click so we don't place during the
    // drag-pan-on-empty-space gesture; `last_drag_place_tile` is the
    // tile we most-recently placed on, so we don't re-fire on the
    // same tile during a held click.
    let mut drag_placing = false;
    let mut last_drag_place_tile: Option<(i32, i32)> = None;

    /// Inspection state — what the player has right-clicked on.
    struct Inspection {
        /// Tile coordinates of the inspected location.
        tile_x: i32,
        tile_y: i32,
        /// Building instance index in sim.buildings (if any).
        building_idx: Option<usize>,
        /// Warehouse index in sim.warehouses (if any).
        warehouse_idx: Option<usize>,
        /// Description lines for the title bar.
        info: String,
    }
    let mut inspection: Option<Inspection> = None;
    let mut demolish_mode = false;
    let mut demolish_hover: Option<usize> = None; // building index under cursor
    let mut diplomacy_panel = false;
    let mut diplomacy_target: u8 = 1; // selected counterpart (1..6) for player 0
    let mut info_mode = false;
    let mut combat_mode = false;
    let mut cities_panel = false;
    let mut ship_panel = false;
    let mut video_speech_panel = false;
    let mut video_speech_sel: usize = 0;
    let mut video_sequences_enabled = true;
    let mut speech_enabled = true;
    let mut options_panel = false;
    let mut options_sel: usize = 0;
    let mut chat_active = false;
    let mut chat_input = String::new();
    // Recently received chat lines (oldest first) with timestamp for TTL.
    let mut chat_log: std::collections::VecDeque<(String, std::time::Instant)> =
        std::collections::VecDeque::new();

    let mut selected_units: Vec<usize> = Vec::new();
    let mut selected_trade_ship_idx: Option<usize> = None;
    let mut troop_assemblies: [Vec<usize>; 9] = std::array::from_fn(|_| Vec::new());
    let mut warehouse_cycle_idx: Option<usize> = None;
    let mut shift_held = false;
    let mut ctrl_held = false;
    let mut save_banner: Option<(String, std::time::Instant)> = None;
    let save_dir = std::path::PathBuf::from("saves");
    let mut save_panel = false;
    let mut save_sel: usize = 0;
    let slot_path = |slot: usize| -> std::path::PathBuf {
        save_dir.join(format!("{}.slot{}.bin", scenario_name, slot))
    };

    'main: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main,

                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    if chat_active {
                        chat_active = false;
                        chat_input.clear();
                    } else if save_panel {
                        save_panel = false;
                    } else if options_panel {
                        options_panel = false;
                    } else if video_speech_panel {
                        video_speech_panel = false;
                    } else if ship_panel {
                        ship_panel = false;
                    } else if placer.active {
                        placer.active = false;
                    } else if demolish_mode {
                        demolish_mode = false;
                    } else if cities_panel {
                        cities_panel = false;
                    } else if info_mode {
                        info_mode = false;
                        selected_building_idx = None;
                    } else if combat_mode {
                        combat_mode = false;
                        selected_units.clear();
                        selected_trade_ship_idx = None;
                    } else if !selected_units.is_empty() || selected_trade_ship_idx.is_some() {
                        selected_units.clear();
                        selected_trade_ship_idx = None;
                    } else if selected_building_idx.is_some() {
                        selected_building_idx = None;
                    } else if inspection.is_some() {
                        inspection = None;
                    } else if sim.paused {
                        sim.paused = false;
                    } else {
                        break 'main;
                    }
                }

                Event::KeyDown {
                    keycode: Some(key), ..
                } => {
                    if matches!(key, Keycode::LShift | Keycode::RShift) {
                        shift_held = true;
                    }
                    if matches!(key, Keycode::LCtrl | Keycode::RCtrl) {
                        ctrl_held = true;
                    }
                    if save_panel {
                        match key {
                            Keycode::Up => {
                                if save_sel > 0 {
                                    save_sel -= 1;
                                }
                            }
                            Keycode::Down => {
                                if save_sel + 1 < 10 {
                                    save_sel += 1;
                                }
                            }
                            Keycode::S => {
                                let path = slot_path(save_sel);
                                let snap = sim.snapshot();
                                let msg = match anno_sim::save::save_to_file(&path, &snap) {
                                    Ok(()) => {
                                        format!("saved slot {} → {}", save_sel, path.display(),)
                                    }
                                    Err(e) => format!("save FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                            }
                            Keycode::L => {
                                let path = slot_path(save_sel);
                                let msg = match anno_sim::save::load_from_file(&path) {
                                    Ok(state) => {
                                        let bldgs = state.buildings.len();
                                        let gold =
                                            state.players.first().map(|p| p.gold).unwrap_or(0);
                                        sim.apply_snapshot(state);
                                        needs_redraw = true;
                                        format!(
                                            "loaded slot {} ({} bldg, {} gold)",
                                            save_sel, bldgs, gold,
                                        )
                                    }
                                    Err(e) => format!("load FAILED: {e}"),
                                };
                                println!("{msg}");
                                save_banner = Some((msg, std::time::Instant::now()));
                                save_panel = false;
                            }
                            Keycode::Escape => {
                                save_panel = false;
                            }
                            Keycode::Pause => {
                                sim.paused = true;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if options_panel {
                        match key {
                            Keycode::Up => {
                                if options_sel > 0 {
                                    options_sel -= 1;
                                }
                            }
                            Keycode::Down => {
                                if options_sel < 3 {
                                    options_sel += 1;
                                }
                            }
                            Keycode::Left | Keycode::Right | Keycode::Return | Keycode::KpEnter => {
                                match options_sel {
                                    0 => {
                                        music_enabled = !music_enabled;
                                        if music_enabled {
                                            if let Some(slot) = music_slot {
                                                audio.streams.resume(slot);
                                            }
                                            println!("Music ON");
                                        } else {
                                            if let Some(slot) = music_slot {
                                                audio.streams.stop(slot);
                                            }
                                            println!("Music OFF");
                                        }
                                    }
                                    1 => {
                                        music_volume = match key {
                                            Keycode::Left => (music_volume - 0.2).max(0.0),
                                            Keycode::Right => (music_volume + 0.2).min(1.0),
                                            _ if music_volume >= 0.95 => 0.0,
                                            _ => (music_volume + 0.2).min(1.0),
                                        };
                                        if let Some(slot) = music_slot {
                                            audio.streams.set_volume(slot, music_volume);
                                        }
                                        println!("Music volume: {:.0}%", music_volume * 100.0);
                                    }
                                    2 => {
                                        video_sequences_enabled = !video_sequences_enabled;
                                    }
                                    3 => {
                                        speech_enabled = !speech_enabled;
                                    }
                                    _ => {}
                                }
                            }
                            Keycode::O | Keycode::Escape => {
                                options_panel = false;
                            }
                            Keycode::Pause => {
                                sim.paused = true;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if video_speech_panel {
                        match key {
                            Keycode::Up => {
                                if video_speech_sel > 0 {
                                    video_speech_sel -= 1;
                                }
                            }
                            Keycode::Down => {
                                if video_speech_sel < 1 {
                                    video_speech_sel += 1;
                                }
                            }
                            Keycode::Left | Keycode::Right | Keycode::Return | Keycode::KpEnter => {
                                if video_speech_sel == 0 {
                                    video_sequences_enabled = !video_sequences_enabled;
                                } else {
                                    speech_enabled = !speech_enabled;
                                }
                            }
                            Keycode::F | Keycode::Escape => {
                                video_speech_panel = false;
                            }
                            Keycode::Pause => {
                                sim.paused = true;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if chat_active {
                        // While the chat input is open, swallow keys we
                        // care about and drop the rest (the typed text
                        // arrives via TextInput events).
                        match key {
                            Keycode::Backspace => {
                                chat_input.pop();
                            }
                            Keycode::Return | Keycode::KpEnter => {
                                let text = chat_input.trim().to_string();
                                if !text.is_empty() {
                                    let local_line = format!("you: {text}");
                                    chat_log.push_back((local_line, std::time::Instant::now()));
                                    if chat_log.len() > 8 {
                                        chat_log.pop_front();
                                    }
                                    let msg = anno_net::protocol::NetMessage::chat(&text);
                                    if let Some(host) = net_host.as_mut() {
                                        host.send_to_all(&msg);
                                    } else if let Some(client) = net_client.as_mut() {
                                        let _ = client.send(&msg);
                                    }
                                }
                                chat_active = false;
                                chat_input.clear();
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if placer.active {
                        // Build mode keys
                        match key {
                            Keycode::Num1 => placer.select_on_page(0),
                            Keycode::Num2 => placer.select_on_page(1),
                            Keycode::Num3 => placer.select_on_page(2),
                            Keycode::Num4 => placer.select_on_page(3),
                            Keycode::Num5 => placer.select_on_page(4),
                            Keycode::Num6 => placer.select_on_page(5),
                            Keycode::Num7 => placer.select_on_page(6),
                            Keycode::Num8 => placer.select_on_page(7),
                            Keycode::Num9 => placer.select_on_page(8),
                            Keycode::PageUp => placer.prev_page(),
                            Keycode::PageDown => placer.next_page(),
                            Keycode::LeftBracket => placer.prev_category(),
                            Keycode::RightBracket => placer.next_category(),
                            Keycode::Z | Keycode::X => {
                                if let Some(b) = placer.selected_building() {
                                    let rot = cod.buildings[b.def_idx].rotate.max(1) as u8;
                                    if matches!(key, Keycode::Z) {
                                        placer.orientation = (placer.orientation + 1) % rot;
                                    } else {
                                        placer.orientation = (placer.orientation + rot - 1) % rot;
                                    }
                                }
                            }
                            Keycode::B => placer.toggle(),
                            // Still allow scrolling in build mode
                            Keycode::Left => scroll_x += 48,
                            Keycode::Right => scroll_x -= 48,
                            Keycode::Up => scroll_y += 48,
                            Keycode::Down => scroll_y -= 48,
                            _ => {}
                        }
                    } else if diplomacy_panel {
                        // Diplomacy panel keys
                        match key {
                            Keycode::Up => {
                                if diplomacy_target > 1 {
                                    diplomacy_target -= 1;
                                }
                            }
                            Keycode::Down => {
                                if diplomacy_target < 6 {
                                    diplomacy_target += 1;
                                }
                            }
                            Keycode::Left | Keycode::Right => {
                                use anno_sim::combat::Diplomacy;
                                let cur = sim.diplomacy.get(0, diplomacy_target);
                                let next = if matches!(key, Keycode::Right) {
                                    match cur {
                                        Diplomacy::Allied => Diplomacy::Neutral,
                                        Diplomacy::Neutral => Diplomacy::War,
                                        Diplomacy::War => Diplomacy::Allied,
                                    }
                                } else {
                                    match cur {
                                        Diplomacy::Allied => Diplomacy::War,
                                        Diplomacy::Neutral => Diplomacy::Allied,
                                        Diplomacy::War => Diplomacy::Neutral,
                                    }
                                };
                                // Route through apply_command so war stays
                                // unilateral while peace/alliance wait for the
                                // source diplomacy acceptance path.
                                let cmd = anno_sim::commands::Command::SetDiplomacy {
                                    a: 0,
                                    b: diplomacy_target,
                                    state: next,
                                };
                                if sim.apply_command(&cmd) {
                                    if let Some(rec) = recorder.as_mut() {
                                        rec.record(sim.game_clock, cmd);
                                    }
                                }
                            }
                            Keycode::D | Keycode::Escape => {
                                diplomacy_panel = false;
                            }
                            Keycode::Pause => {
                                sim.paused = true;
                            }
                            _ => {}
                        }
                    } else {
                        // Normal mode keys
                        let scroll_speed = 48;
                        match key {
                            Keycode::Left => scroll_x += scroll_speed,
                            Keycode::Right => scroll_x -= scroll_speed,
                            Keycode::Up => scroll_y += scroll_speed,
                            Keycode::Down => scroll_y -= scroll_speed,
                            Keycode::Tab => {
                                if !world_mode && !islands.is_empty() {
                                    let start = current_island;
                                    loop {
                                        current_island = (current_island + 1) % islands.len();
                                        if !islands[current_island].tiles.is_empty()
                                            || current_island == start
                                        {
                                            break;
                                        }
                                    }
                                    needs_redraw = true;
                                    scroll_x = 0;
                                    scroll_y = 0;
                                }
                            }
                            Keycode::W => {
                                let pirate_slot = anno_sim::free_trader::PIRATE_SLOT;
                                let mut surrendered = 0usize;

                                for &ui in &selected_units {
                                    if let Some(unit) = sim.military_units.get_mut(ui) {
                                        if unit.owner == 0
                                            && unit.is_alive()
                                            && unit.unit_type.stats().is_naval
                                        {
                                            unit.owner = pirate_slot;
                                            unit.target_x = unit.tile_x;
                                            unit.target_y = unit.tile_y;
                                            unit.combat_target = -1;
                                            unit.escort_ship = -1;
                                            unit.patrol.clear();
                                            surrendered += 1;
                                        }
                                    }
                                }
                                selected_units.retain(|&ui| {
                                    sim.military_units
                                        .get(ui)
                                        .map(|u| u.owner == 0 && u.is_alive())
                                        .unwrap_or(false)
                                });

                                if let Some(si) = selected_trade_ship_idx.take() {
                                    if let Some(ship) = sim.trade_ships.get_mut(si) {
                                        if ship.owner == 0 && ship.active {
                                            ship.owner = pirate_slot;
                                            ship.route_id = u16::MAX;
                                            ship.state = anno_sim::trade::ShipState::Idle;
                                            ship.path.clear();
                                            ship.path_idx = 0;
                                            surrendered += 1;
                                        }
                                    }
                                }

                                if surrendered > 0 {
                                    sim.diplomacy.set(pirate_slot, 0, Diplomacy::War);
                                    println!(
                                        "{surrendered} selected ship(s) surrendered to pirates",
                                    );
                                }
                            }
                            Keycode::Pause => {
                                sim.paused = true;
                            }
                            Keycode::F2 => {
                                if sprite_zoom != 2 && !sprites_by_zoom[2].is_empty() {
                                    sprite_zoom = 2;
                                    needs_redraw = true;
                                }
                                display_zoom = 1;
                            }
                            Keycode::F3 => {
                                if sprite_zoom != 1 && !sprites_by_zoom[1].is_empty() {
                                    sprite_zoom = 1;
                                    needs_redraw = true;
                                }
                                display_zoom = 1;
                            }
                            Keycode::F4 => {
                                if sprite_zoom != 0 {
                                    sprite_zoom = 0;
                                    needs_redraw = true;
                                }
                                display_zoom = 1;
                            }
                            Keycode::F5 => {
                                sim.speed_multiplier = 1;
                            }
                            Keycode::F6 => {
                                sim.speed_multiplier = 2;
                            }
                            Keycode::F7 => {
                                sim.speed_multiplier = 4;
                            }
                            Keycode::F => {
                                video_speech_panel = !video_speech_panel;
                                if video_speech_panel {
                                    placer.active = false;
                                    demolish_mode = false;
                                    diplomacy_panel = false;
                                    info_mode = false;
                                    combat_mode = false;
                                    ship_panel = false;
                                    cities_panel = false;
                                    save_panel = false;
                                    selected_building_idx = None;
                                    selected_units.clear();
                                    selected_trade_ship_idx = None;
                                }
                            }
                            Keycode::J => {
                                let mut target: Option<(usize, i32, i32, &'static str)> = None;

                                if let Some(si) = selected_trade_ship_idx {
                                    if let Some(ship) = sim.trade_ships.get(si) {
                                        if ship.active {
                                            target = Some((
                                                current_island,
                                                ship.world_x,
                                                ship.world_y,
                                                "trade ship",
                                            ));
                                        }
                                    }
                                }

                                if target.is_none() {
                                    if let Some(&ui) = selected_units.iter().find(|&&ui| {
                                        sim.military_units
                                            .get(ui)
                                            .map(|u| u.is_alive())
                                            .unwrap_or(false)
                                    }) {
                                        if let Some(unit) = sim.military_units.get(ui) {
                                            target = Some((
                                                current_island,
                                                unit.tile_x,
                                                unit.tile_y,
                                                "unit",
                                            ));
                                        }
                                    }
                                }

                                if target.is_none() {
                                    if let Some(bi) = selected_building_idx {
                                        if let Some(b) = sim.buildings.get(bi) {
                                            if b.active {
                                                let def = &defs[b.def_id as usize];
                                                if let Some(island_idx) = islands
                                                    .iter()
                                                    .position(|i| i.number == b.island_id)
                                                {
                                                    target = Some((
                                                        island_idx,
                                                        b.tile_x as i32 + def.width as i32 / 2,
                                                        b.tile_y as i32 + def.height as i32 / 2,
                                                        "building",
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }

                                if target.is_none() {
                                    if let Some(ref insp) = inspection {
                                        if let Some(bi) = insp.building_idx {
                                            if let Some(b) = sim.buildings.get(bi) {
                                                if b.active {
                                                    let def = &defs[b.def_id as usize];
                                                    if let Some(island_idx) = islands
                                                        .iter()
                                                        .position(|i| i.number == b.island_id)
                                                    {
                                                        target = Some((
                                                            island_idx,
                                                            b.tile_x as i32 + def.width as i32 / 2,
                                                            b.tile_y as i32 + def.height as i32 / 2,
                                                            "building",
                                                        ));
                                                    }
                                                }
                                            }
                                        } else if let Some(wi) = insp.warehouse_idx {
                                            if let Some(wh) = sim.warehouses.get(wi) {
                                                if wh.active {
                                                    if let Some(island_idx) = islands
                                                        .iter()
                                                        .position(|i| i.number == wh.island_id)
                                                    {
                                                        target = Some((
                                                            island_idx,
                                                            wh.tile_x as i32,
                                                            wh.tile_y as i32,
                                                            "warehouse",
                                                        ));
                                                    }
                                                }
                                            }
                                        } else {
                                            target = Some((
                                                current_island,
                                                insp.tile_x,
                                                insp.tile_y,
                                                "tile",
                                            ));
                                        }
                                    }
                                }

                                if let Some((island_idx, tile_x, tile_y, label)) = target {
                                    current_island = island_idx;
                                    world_mode = false;
                                    needs_redraw = true;
                                    let (sx, sy) = scroll_for_island_tile(
                                        &islands[current_island],
                                        sprite_zoom,
                                        display_zoom,
                                        tile_x,
                                        tile_y,
                                    );
                                    scroll_x = sx;
                                    scroll_y = sy;
                                    println!(
                                        "Jumped to active {label}: island {} ({tile_x},{tile_y})",
                                        islands[current_island].number,
                                    );
                                }
                            }
                            Keycode::B => {
                                if !world_mode {
                                    demolish_mode = false;
                                    info_mode = false;
                                    combat_mode = false;
                                    selected_building_idx = None;
                                    selected_units.clear();
                                    selected_trade_ship_idx = None;
                                    placer.toggle();
                                }
                            }
                            Keycode::D => {
                                diplomacy_panel = !diplomacy_panel;
                                if diplomacy_panel {
                                    placer.active = false;
                                    demolish_mode = false;
                                    info_mode = false;
                                    combat_mode = false;
                                    selected_units.clear();
                                    selected_trade_ship_idx = None;
                                }
                            }
                            Keycode::I => {
                                info_mode = !info_mode;
                                if info_mode {
                                    placer.active = false;
                                    demolish_mode = false;
                                    diplomacy_panel = false;
                                    combat_mode = false;
                                    selected_units.clear();
                                    selected_trade_ship_idx = None;
                                } else {
                                    selected_building_idx = None;
                                }
                            }
                            Keycode::K => {
                                combat_mode = !combat_mode;
                                if combat_mode {
                                    placer.active = false;
                                    demolish_mode = false;
                                    diplomacy_panel = false;
                                    info_mode = false;
                                    selected_building_idx = None;
                                } else {
                                    selected_units.clear();
                                    selected_trade_ship_idx = None;
                                }
                            }
                            Keycode::Return | Keycode::KpEnter => {
                                if !chat_active {
                                    chat_active = true;
                                    chat_input.clear();
                                }
                            }
                            Keycode::H => {
                                let owned: Vec<usize> = sim
                                    .warehouses
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, w)| w.active && w.owner == 0)
                                    .map(|(idx, _)| idx)
                                    .collect();
                                if !owned.is_empty() {
                                    let current_island_number =
                                        islands.get(current_island).map(|i| i.number).unwrap_or(0);
                                    let current_pos = warehouse_cycle_idx
                                        .and_then(|idx| owned.iter().position(|&i| i == idx))
                                        .or_else(|| {
                                            owned.iter().position(|&idx| {
                                                sim.warehouses[idx].island_id
                                                    == current_island_number
                                            })
                                        });
                                    let next_pos =
                                        current_pos.map_or(0, |pos| (pos + 1) % owned.len());
                                    let target_idx = owned[next_pos];
                                    warehouse_cycle_idx = Some(target_idx);
                                    let wh = &sim.warehouses[target_idx];
                                    if let Some(island_idx) =
                                        islands.iter().position(|i| i.number == wh.island_id)
                                    {
                                        current_island = island_idx;
                                        world_mode = false;
                                        needs_redraw = true;
                                        let (sx, sy) = scroll_for_island_tile(
                                            &islands[current_island],
                                            sprite_zoom,
                                            display_zoom,
                                            wh.tile_x as i32,
                                            wh.tile_y as i32,
                                        );
                                        scroll_x = sx;
                                        scroll_y = sy;
                                    }
                                }
                            }
                            Keycode::C => {
                                cities_panel = !cities_panel;
                            }
                            Keycode::S => {
                                ship_panel = !ship_panel;
                            }
                            Keycode::O => {
                                options_panel = !options_panel;
                                if options_panel {
                                    placer.active = false;
                                    demolish_mode = false;
                                    diplomacy_panel = false;
                                    info_mode = false;
                                    combat_mode = false;
                                    ship_panel = false;
                                    cities_panel = false;
                                    save_panel = false;
                                    video_speech_panel = false;
                                    selected_building_idx = None;
                                    selected_units.clear();
                                    selected_trade_ship_idx = None;
                                }
                            }
                            Keycode::Num1
                            | Keycode::Num2
                            | Keycode::Num3
                            | Keycode::Num4
                            | Keycode::Num5
                            | Keycode::Num6
                            | Keycode::Num7
                            | Keycode::Num8
                            | Keycode::Num9 => {
                                if let Some(slot) = troop_assembly_slot(key) {
                                    if ctrl_held {
                                        let stored: Vec<usize> = selected_units
                                            .iter()
                                            .copied()
                                            .filter(|&ui| {
                                                sim.military_units
                                                    .get(ui)
                                                    .map(|u| u.owner == 0 && u.is_alive())
                                                    .unwrap_or(false)
                                            })
                                            .collect();
                                        troop_assemblies[slot] = stored;
                                        println!(
                                            "Stored {} unit(s) in troop assembly {}",
                                            troop_assemblies[slot].len(),
                                            slot + 1,
                                        );
                                    } else {
                                        let recalled: Vec<usize> = troop_assemblies[slot]
                                            .iter()
                                            .copied()
                                            .filter(|&ui| {
                                                sim.military_units
                                                    .get(ui)
                                                    .map(|u| u.owner == 0 && u.is_alive())
                                                    .unwrap_or(false)
                                            })
                                            .collect();
                                        troop_assemblies[slot] = recalled.clone();
                                        if !recalled.is_empty() {
                                            selected_units = recalled;
                                            selected_trade_ship_idx = None;
                                            selected_building_idx = None;
                                            inspection = None;
                                            println!(
                                                "Recalled {} unit(s) from troop assembly {}",
                                                selected_units.len(),
                                                slot + 1,
                                            );
                                        }
                                    }
                                }
                            }
                            Keycode::L => {
                                save_panel = !save_panel;
                            }
                            _ => {}
                        }
                    }
                }

                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    x,
                    y,
                    ..
                } => {
                    // Check if click is on the minimap (bottom-right corner)
                    let on_minimap = if let Some(ref rs) = rendered {
                        let sx = 200.0 / rs.width as f64;
                        let sy = 150.0 / rs.height as f64;
                        let ms = sx.min(sy).min(1.0);
                        let mw = (rs.width as f64 * ms) as i32;
                        let mh = (rs.height as f64 * ms) as i32;
                        let mx = WINDOW_W as i32 - mw - 8;
                        let my = WINDOW_H as i32 - mh - 8;
                        x >= mx && x < mx + mw && y >= my && y < my + mh
                    } else {
                        false
                    };

                    if on_minimap {
                        minimap_clicked = true;
                        minimap_click_x = x;
                        minimap_click_y = y;
                    } else if placer.active && !world_mode {
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x,
                                tex_y,
                                rs.origin_x,
                                rs.origin_y,
                                rs.tile_w,
                                rs.tile_h,
                            );
                            let outcome = try_place_building(
                                &mut sim,
                                &mut islands,
                                current_island,
                                &defs,
                                &cod,
                                &placer,
                                tile_x,
                                tile_y,
                                &mut recorder,
                            );
                            match outcome {
                                PlaceOutcome::Placed => {
                                    println!(
                                        "Placed {} at ({},{})",
                                        &placer.buildable[placer.selected].name, tile_x, tile_y,
                                    );
                                    if let (Some(sfx), Some(handle)) =
                                        (place_sound_slot, &audio.stream_handle)
                                    {
                                        audio.waves.play_once(
                                            sfx,
                                            WINDOW_W as i32 / 2,
                                            WINDOW_H as i32 / 2,
                                            handle,
                                        );
                                    }
                                    needs_redraw = true;
                                    last_drag_place_tile = Some((tile_x, tile_y));
                                    drag_placing = true;
                                }
                                PlaceOutcome::NotEnoughGold { need, have } => {
                                    save_banner = Some((
                                        format!("Not enough gold (need {need}, have {have})"),
                                        std::time::Instant::now(),
                                    ));
                                }
                                PlaceOutcome::NotCoastal => {
                                    save_banner = Some((
                                        "build FAILED: Fisheries must be placed on the coast"
                                            .into(),
                                        std::time::Instant::now(),
                                    ));
                                }
                                PlaceOutcome::BlockedByTerrain => {
                                    // Silent — common case while
                                    // dragging across mixed terrain.
                                }
                                PlaceOutcome::NotUnlocked { infra } => {
                                    // Name the rung and quote its
                                    // `DAT_0061fbc0` threshold — the
                                    // cumulative residents of that
                                    // BGruppe and every tier above it
                                    // the settlement still needs.
                                    let idx = usize::from(infra);
                                    let name = data_bridge::INFRA_NAMES
                                        .get(idx)
                                        .copied()
                                        .unwrap_or("INFRA_?");
                                    let (group, minwohn) = data_bridge::BAUINFRA_LADDER
                                        .get(idx)
                                        .copied()
                                        .unwrap_or((0, 0));
                                    let tier_name = |t: u8| match t {
                                        0 => "Pioneer",
                                        1 => "Settler",
                                        2 => "Citizen",
                                        3 => "Merchant",
                                        _ => "Aristocrat",
                                    };
                                    save_banner = Some((
                                        format!(
                                            "build FAILED: {name} locked (needs {minwohn} \
                                             {}+ residents)",
                                            tier_name(group),
                                        ),
                                        std::time::Instant::now(),
                                    ));
                                }
                                PlaceOutcome::NoIslandMap | PlaceOutcome::NoBuildingSelected => {}
                            }
                        }
                    } else if demolish_mode && !world_mode {
                        // Try to demolish a building
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x,
                                tex_y,
                                rs.origin_x,
                                rs.origin_y,
                                rs.tile_w,
                                rs.tile_h,
                            );

                            let island = &islands[current_island];
                            let island_id = island.number;

                            if let Some(demolished) =
                                demolish_building(&mut sim, &defs, island_id, tile_x, tile_y, 0)
                            {
                                if let Some(rec) = recorder.as_mut() {
                                    rec.record(
                                        sim.game_clock,
                                        anno_sim::commands::Command::DemolishBuilding {
                                            player: 0,
                                            island: island_id,
                                            tile_x: tile_x as u16,
                                            tile_y: tile_y as u16,
                                        },
                                    );
                                }
                                let name = cod.buildings[demolished.def_id as usize]
                                    .properties
                                    .get("Name")
                                    .cloned()
                                    .unwrap_or_else(|| format!("Bldg#{}", demolished.def_id));
                                println!(
                                    "Demolished {} at ({},{}) on island {} [refund: {} gold]",
                                    name,
                                    demolished.tile_x,
                                    demolished.tile_y,
                                    island_id,
                                    demolished.refund,
                                );
                                needs_redraw = true;
                                // Clear inspection if it was pointing at this building
                                if let Some(ref insp) = inspection {
                                    if insp.building_idx == Some(demolished.building_index) {
                                        inspection = None;
                                    }
                                }
                            }
                        }
                    } else {
                        // Combat mode owns unit selection. Outside it,
                        // left-click keeps its map drag / info behavior.
                        let mut hit_unit: Option<usize> = None;
                        let mut hit_trade_ship: Option<usize> = None;
                        if combat_mode && !world_mode {
                            if let Some(ref rs) = rendered {
                                let dst_w = rs.width as i32 * display_zoom;
                                let dst_h = rs.height as i32 * display_zoom;
                                let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                                let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                                let tex_x = (x - dst_x) / display_zoom;
                                let tex_y = (y - dst_y) / display_zoom;
                                let (tile_x, tile_y) = screen_to_tile(
                                    tex_x,
                                    tex_y,
                                    rs.origin_x,
                                    rs.origin_y,
                                    rs.tile_w,
                                    rs.tile_h,
                                );
                                let active_island_id = islands[current_island].number;
                                hit_unit = sim.military_units.iter().position(|u| {
                                    if !u.is_alive() || u.owner != 0 {
                                        return false;
                                    }
                                    if let Some(island_id) = u.source_island_id {
                                        return island_id == active_island_id
                                            && sim
                                                .island_maps
                                                .iter()
                                                .find(|map| map.island_id == island_id)
                                                .and_then(|map| {
                                                    map.source_world_to_local((u.tile_x, u.tile_y))
                                                })
                                                .is_some_and(|(x, y)| {
                                                    (x - tile_x).abs() <= 1
                                                        && (y - tile_y).abs() <= 1
                                                });
                                    }
                                    (u.tile_x - tile_x).abs() <= 1 && (u.tile_y - tile_y).abs() <= 1
                                });
                                hit_trade_ship = sim.trade_ships.iter().position(|s| {
                                    s.active
                                        && s.owner == 0
                                        && (s.world_x - tile_x).abs() <= 1
                                        && (s.world_y - tile_y).abs() <= 1
                                });
                            }
                        }
                        if let Some(ui) = hit_unit {
                            if shift_held {
                                if !selected_units.contains(&ui) {
                                    selected_units.push(ui);
                                }
                            } else {
                                selected_units.clear();
                                selected_units.push(ui);
                            }
                            selected_building_idx = None;
                            selected_trade_ship_idx = None;
                            println!("Selected {} unit(s)", selected_units.len(),);
                        } else if let Some(si) = hit_trade_ship {
                            selected_units.clear();
                            selected_trade_ship_idx = Some(si);
                            selected_building_idx = None;
                            println!("Selected trade ship #{si}",);
                        } else {
                            // In info mode, clicks open the Anno-style
                            // object card. Outside it, empty-map clicks
                            // keep their drag-pan behavior.
                            let mut hit_building: Option<usize> = None;
                            if !world_mode {
                                if let Some(ref rs) = rendered {
                                    let dst_w = rs.width as i32 * display_zoom;
                                    let dst_h = rs.height as i32 * display_zoom;
                                    let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                                    let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                                    let tex_x = (x - dst_x) / display_zoom;
                                    let tex_y = (y - dst_y) / display_zoom;
                                    let (tile_x, tile_y) = screen_to_tile(
                                        tex_x,
                                        tex_y,
                                        rs.origin_x,
                                        rs.origin_y,
                                        rs.tile_w,
                                        rs.tile_h,
                                    );
                                    let island_id = islands[current_island].number;
                                    hit_building = sim.buildings.iter().position(|b| {
                                        b.active && b.island_id == island_id && {
                                            let def = &defs[b.def_id as usize];
                                            let bx = b.tile_x as i32;
                                            let by = b.tile_y as i32;
                                            tile_x >= bx
                                                && tile_x < bx + def.width as i32
                                                && tile_y >= by
                                                && tile_y < by + def.height as i32
                                        }
                                    });
                                }
                            }
                            if info_mode {
                                selected_building_idx = hit_building;
                                if !selected_units.is_empty() {
                                    selected_units.clear();
                                }
                                selected_trade_ship_idx = None;
                            } else {
                                selected_building_idx = None;
                                if !selected_units.is_empty() {
                                    selected_units.clear();
                                }
                                selected_trade_ship_idx = None;
                                dragging = true;
                                drag_start = (x - scroll_x, y - scroll_y);
                            }
                        }
                    }
                }

                Event::MouseButtonDown {
                    mouse_btn: MouseButton::Right,
                    x,
                    y,
                    ..
                } => {
                    if sim.paused {
                        sim.paused = false;
                        continue;
                    }
                    // If units are selected: issue a move order to that tile.
                    if !world_mode && !selected_units.is_empty() {
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x,
                                tex_y,
                                rs.origin_x,
                                rs.origin_y,
                                rs.tile_w,
                                rs.tile_h,
                            );
                            let mut moved = 0;
                            let active_island_id = islands[current_island].number;
                            for &ui in &selected_units {
                                let cmd = anno_sim::commands::Command::MoveUnit {
                                    player: 0,
                                    unit_index: ui as u32,
                                    island: active_island_id,
                                    tile_x,
                                    tile_y,
                                };
                                if sim.apply_command(&cmd) {
                                    if let Some(rec) = recorder.as_mut() {
                                        rec.record(sim.game_clock, cmd);
                                    }
                                    moved += 1;
                                }
                            }
                            println!("Move order → ({},{}) for {moved} unit(s)", tile_x, tile_y,);
                        }
                        continue;
                    }
                    // Right-click: inspect tile
                    if !world_mode {
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x,
                                tex_y,
                                rs.origin_x,
                                rs.origin_y,
                                rs.tile_w,
                                rs.tile_h,
                            );

                            let island = &islands[current_island];
                            let island_id = island.number;

                            // Find building at this tile
                            let building_idx = sim.buildings.iter().position(|b| {
                                b.island_id == island_id && {
                                    let def = &defs[b.def_id as usize];
                                    let bx = b.tile_x as i32;
                                    let by = b.tile_y as i32;
                                    tile_x >= bx
                                        && tile_x < bx + def.width as i32
                                        && tile_y >= by
                                        && tile_y < by + def.height as i32
                                }
                            });

                            // Find warehouse at this tile
                            let warehouse_idx = sim.warehouses.iter().position(|w| {
                                w.island_id == island_id
                                    && (w.tile_x as i32 - tile_x).abs() <= 2
                                    && (w.tile_y as i32 - tile_y).abs() <= 2
                            });

                            // Build info string
                            let mut info = format!("Tile ({},{}) ", tile_x, tile_y);

                            if let Some(bi) = building_idx {
                                let b = &sim.buildings[bi];
                                let def = &defs[b.def_id as usize];
                                let name = cod.buildings[b.def_id as usize]
                                    .properties
                                    .get("Name")
                                    .cloned()
                                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                                info.push_str(&format!("| {} ", name));
                                if def.output_good != Good::None {
                                    info.push_str(&format!(
                                        "| out:{:?}={}/{} ",
                                        def.output_good, b.output_stock, def.storage_capacity
                                    ));
                                    if def.input_good_1 != Good::None {
                                        info.push_str(&format!(
                                            "in1:{:?}={} ",
                                            def.input_good_1, b.input_1_stock
                                        ));
                                    }
                                    if def.input_good_2 != Good::None {
                                        info.push_str(&format!(
                                            "in2:{:?}={} ",
                                            def.input_good_2, b.input_2_stock
                                        ));
                                    }
                                    info.push_str(&format!(
                                        "eff:{}% ",
                                        b.efficiency as u32 * 100 / 128
                                    ));
                                }
                            }

                            if let Some(wi) = warehouse_idx {
                                let w = &sim.warehouses[wi];
                                info.push_str("| WH: ");
                                let mut goods_shown = 0;
                                let all_goods = [
                                    Good::Wood,
                                    Good::Iron,
                                    Good::Ore,
                                    Good::Gold,
                                    Good::Wool,
                                    Good::Sugar,
                                    Good::Tobacco,
                                    Good::Cattle,
                                    Good::Grain,
                                    Good::Flour,
                                    Good::Food,
                                    Good::Alcohol,
                                    Good::Cloth,
                                    Good::Clothing,
                                    Good::Jewelry,
                                    Good::Tools,
                                    Good::Bricks,
                                    Good::Swords,
                                    Good::Cannons,
                                    Good::Muskets,
                                    Good::Stone,
                                    Good::Cocoa,
                                    Good::Spices,
                                    Good::WildGame,
                                    Good::Cotton,
                                    Good::Silk,
                                    Good::Fish,
                                    Good::Grapes,
                                    Good::TobaccoProducts,
                                ];
                                for &g in &all_goods {
                                    let qty = w.stock(g);
                                    if qty > 0 {
                                        info.push_str(&format!("{:?}={} ", g, qty));
                                        goods_shown += 1;
                                        if goods_shown >= 8 {
                                            info.push_str("...");
                                            break;
                                        }
                                    }
                                }
                                if goods_shown == 0 {
                                    info.push_str("(empty)");
                                }
                            }

                            if building_idx.is_none() && warehouse_idx.is_none() {
                                // Check what tile sprite is here
                                if let Some(tile) = island
                                    .tiles
                                    .iter()
                                    .find(|t| t.x as i32 == tile_x && t.y as i32 == tile_y)
                                {
                                    info.push_str(&format!("| definition#{}", tile.source_id()));
                                } else {
                                    info.push_str("| (empty)");
                                }
                            }

                            inspection = Some(Inspection {
                                tile_x,
                                tile_y,
                                building_idx,
                                warehouse_idx,
                                info,
                            });
                        }
                    } else {
                        inspection = None;
                    }
                }

                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    dragging = false;
                    drag_placing = false;
                    last_drag_place_tile = None;
                }

                Event::KeyUp {
                    keycode: Some(k), ..
                } => match k {
                    Keycode::LShift | Keycode::RShift => {
                        shift_held = false;
                    }
                    Keycode::LCtrl | Keycode::RCtrl => {
                        ctrl_held = false;
                    }
                    _ => {}
                },

                Event::TextInput { ref text, .. } if chat_active => {
                    if chat_input.len() < 200 {
                        chat_input.push_str(text);
                    }
                }

                Event::MouseMotion { x, y, .. } => {
                    mouse_x = x;
                    mouse_y = y;
                    if dragging && !placer.active {
                        scroll_x = x - drag_start.0;
                        scroll_y = y - drag_start.1;
                    } else if drag_placing && placer.active && !world_mode {
                        // Mass-place: each new tile under the cursor
                        // gets a fresh placement attempt. Same gates
                        // as a click; failures are silent (skipped
                        // tiles let the player paint over mixed
                        // terrain without spamming banners).
                        if let Some(ref rs) = rendered {
                            let dst_w = rs.width as i32 * display_zoom;
                            let dst_h = rs.height as i32 * display_zoom;
                            let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                            let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                            let tex_x = (x - dst_x) / display_zoom;
                            let tex_y = (y - dst_y) / display_zoom;
                            let (tile_x, tile_y) = screen_to_tile(
                                tex_x,
                                tex_y,
                                rs.origin_x,
                                rs.origin_y,
                                rs.tile_w,
                                rs.tile_h,
                            );
                            if last_drag_place_tile != Some((tile_x, tile_y)) {
                                let outcome = try_place_building(
                                    &mut sim,
                                    &mut islands,
                                    current_island,
                                    &defs,
                                    &cod,
                                    &placer,
                                    tile_x,
                                    tile_y,
                                    &mut recorder,
                                );
                                if matches!(outcome, PlaceOutcome::Placed) {
                                    needs_redraw = true;
                                    last_drag_place_tile = Some((tile_x, tile_y));
                                } else {
                                    // Track the cursor anyway so we
                                    // don't keep retrying the same
                                    // failed tile every motion event.
                                    last_drag_place_tile = Some((tile_x, tile_y));
                                }
                            }
                        }
                    }
                }

                _ => {}
            }
        }

        // Update hover tile for build/demolish mode cursor
        if (placer.active || demolish_mode) && !world_mode {
            if let Some(ref rs) = rendered {
                let dst_w = rs.width as i32 * display_zoom;
                let dst_h = rs.height as i32 * display_zoom;
                let dst_x = (WINDOW_W as i32 - dst_w) / 2 + scroll_x;
                let dst_y = (WINDOW_H as i32 - dst_h) / 2 + scroll_y;
                let tex_x = (mouse_x - dst_x) / display_zoom;
                let tex_y = (mouse_y - dst_y) / display_zoom;

                let (tx, ty) =
                    screen_to_tile(tex_x, tex_y, rs.origin_x, rs.origin_y, rs.tile_w, rs.tile_h);
                if placer.active {
                    placer.hover_tile = Some((tx, ty));
                }
                if demolish_mode {
                    let island_id = islands[current_island].number;
                    demolish_hover = sim.buildings.iter().position(|b| {
                        b.owner == 0 && b.island_id == island_id && {
                            let def = &defs[b.def_id as usize];
                            let bx = b.tile_x as i32;
                            let by = b.tile_y as i32;
                            tx >= bx
                                && tx < bx + def.width as i32
                                && ty >= by
                                && ty < by + def.height as i32
                        }
                    });
                }
            }
        } else {
            placer.hover_tile = None;
            demolish_hover = None;
        }

        // Simulation tick
        let now = timer.ticks();
        let dt_ms = now.wrapping_sub(last_tick);
        last_tick = now;

        // Net poll: drain any messages first.
        if let Some(host) = net_host.as_mut() {
            let evs = host.poll();
            for ev in evs {
                match ev {
                    anno_net::session::SessionEvent::PlayerJoined {
                        slot,
                        player_id,
                        name,
                    } => {
                        println!("[net] joined slot={slot} id={player_id} name={name}");
                        sim.source_kind4_dispatch.single_player = false;
                        net_status = format!(
                            "HOST :{} ({} peers)",
                            net_role_port(&net_role),
                            host.session().player_count - 1,
                        );
                    }
                    anno_net::session::SessionEvent::PlayerLeft { slot, player_id } => {
                        println!("[net] left slot={slot} id={player_id}");
                        sim.source_kind4_dispatch.single_player = host.session().player_count < 2;
                        net_status = format!(
                            "HOST :{} ({} peers)",
                            net_role_port(&net_role),
                            host.session().player_count.saturating_sub(1),
                        );
                    }
                    anno_net::session::SessionEvent::Chat { from_player, text } => {
                        let line = format!("p{from_player}: {text}");
                        println!("[chat] {line}");
                        chat_log.push_back((line, std::time::Instant::now()));
                        if chat_log.len() > 8 {
                            chat_log.pop_front();
                        }
                        // Re-broadcast to peers so everyone sees client chats.
                        let msg = anno_net::protocol::NetMessage::chat(&text);
                        host.send_to_all(&msg);
                    }
                    anno_net::session::SessionEvent::GameData { from_player, data } => {
                        // Tag-prefixed payloads are commands from clients;
                        // anything else is a stray broadcast we ignore.
                        if let Some(cmd) = anno_sim::commands::Command::decode(&data) {
                            let applied = sim.apply_command(&cmd);
                            if applied {
                                if let Some(rec) = recorder.as_mut() {
                                    rec.record(sim.game_clock, cmd.clone());
                                }
                            }
                            println!("[cmd] from p{from_player}: {:?} (applied={applied})", cmd,);
                        }
                    }
                    _ => {}
                }
            }
            // Periodic state broadcast.
            last_broadcast_ms = last_broadcast_ms.saturating_add(dt_ms);
            if last_broadcast_ms >= broadcast_interval_ms {
                last_broadcast_ms = 0;
                let snap = sim.snapshot();
                if let Ok(payload) = bincode::serialize(&snap) {
                    let msg = anno_net::protocol::NetMessage::game_data(payload);
                    host.send_to_all(&msg);
                }
            }
        }
        if let Some(client) = net_client.as_mut() {
            let evs = client.poll();
            for ev in evs {
                match ev {
                    anno_net::session::SessionEvent::GameData { data, .. } => {
                        // Defensive: ignore command-tagged payloads at the
                        // client (the host never broadcasts those, but if
                        // they arrived bincode would garble the SaveState).
                        if data.first().copied() == Some(anno_sim::commands::COMMAND_TAG) {
                            continue;
                        }
                        if let Ok(snap) = bincode::deserialize::<anno_sim::save::SaveState>(&data) {
                            sim.apply_snapshot(snap);
                            needs_redraw = true;
                        } else {
                            eprintln!(
                                "[net] failed to deserialize snapshot ({} bytes)",
                                data.len()
                            );
                        }
                    }
                    anno_net::session::SessionEvent::Chat { from_player, text } => {
                        let line = format!("p{from_player}: {text}");
                        println!("[chat] {line}");
                        chat_log.push_back((line, std::time::Instant::now()));
                        if chat_log.len() > 8 {
                            chat_log.pop_front();
                        }
                    }
                    _ => {}
                }
            }
        }

        // Anno 1602 keeps the simulation running while menus are open;
        // the player has to hit Pause manually. No auto-pause.

        if dt_ms > 0 && dt_ms < 1000 {
            if net_client.is_none() {
                sim.tick(dt_ms);
            }
            if sim.source_map_cell_revision != last_source_map_cell_revision {
                last_source_map_cell_revision = sim.source_map_cell_revision;
                needs_redraw = true;
            }
            entity_visual_elapsed_ms = entity_visual_elapsed_ms.wrapping_add(dt_ms);
            let entity_visual_gen = entity_visual_elapsed_ms / 20;
            if entity_visual_gen != last_entity_visual_gen {
                last_entity_visual_gen = entity_visual_gen;
                needs_redraw = true;
            }
            // Auto-save: matches `1602_exe.c:98061` —
            //   if (DAT_005bafc8 != 0 && (DAT_005b1b68 += DAT_005b315c, 599999 < DAT_005b1b68))
            //     FUN_00488b50("lastgame", path); reset counter
            // i.e. when not paused, accumulate frame time and on
            // crossing 599 999 ms (~10 min) write to "lastgame.bin".
            if !sim.paused && sim.autosave_timer_ms >= anno_sim::simulation::AUTOSAVE_INTERVAL_MS {
                sim.autosave_timer_ms = 0;
                let path = save_dir.join(format!("{}.lastgame.bin", scenario_name));
                let snap = sim.snapshot();
                let msg = match anno_sim::save::save_to_file(&path, &snap) {
                    Ok(()) => format!("auto-saved → {}", path.display()),
                    Err(e) => format!("auto-save FAILED: {e}"),
                };
                println!("{msg}");
                save_banner = Some((msg, std::time::Instant::now()));
            }
            // Drain objective completions for the completion cue.
            if !sim.objective_completions.is_empty() {
                sim.objective_completions.clear();
                if speech_enabled {
                    if let (Some(sfx), Some(handle)) = (event_obj_done_slot, &audio.stream_handle) {
                        audio.waves.play_once(
                            sfx,
                            WINDOW_W as i32 / 2,
                            WINDOW_H as i32 / 2,
                            handle,
                        );
                    }
                }
            }
            // Wake-up alarms do not use a generic SFX slot; the routed
            // announcement categories below own speech playback.

            // Drain sim event log lines. Voice announcements key off
            // the line prefix so we play one slot per announcement
            // category - Anno used spoken cues, not SFX.
            if !sim.event_log.is_empty() {
                for line in sim.event_log.drain(..) {
                    if !speech_enabled {
                        continue;
                    }
                    let voice_slot = if line.starts_with("[trader]") {
                        voice_trader_slot
                    } else if line.starts_with("[combat]") || line.contains("attack") {
                        voice_attack_slot
                    } else if line.starts_with("[volcano]") {
                        voice_volcano_slot
                    } else if line.starts_with("[fire]") {
                        voice_fire_slot
                    } else if line.contains("treasury") || line.contains("bankrupt") {
                        voice_treasury_slot
                    } else if line.starts_with("[supply]") || line.contains("low on") {
                        voice_stockpile_slot
                    } else if line.starts_with("[diplo]")
                        || line.starts_with("[outcome]")
                        || line.starts_with("[obj]")
                        || line.starts_with("[victory]")
                    {
                        voice_trader_slot // triumph.wav for positive
                                          // diplomacy / objective /
                                          // victory events
                    } else if line.starts_with("[defeat]") {
                        voice_attack_slot
                    } else {
                        None
                    };
                    if let (Some(sfx), Some(handle)) = (voice_slot, &audio.stream_handle) {
                        audio.waves.play_once(
                            sfx,
                            WINDOW_W as i32 / 2,
                            WINDOW_H as i32 / 2,
                            handle,
                        );
                    }
                }
            }

            // Drain combat-destroyed buildings: replace the static
            // footprint with the source `Ruinenr` tile(s), or clear it
            // when haeuser.cod says `NORUINE`.
            if !sim.tile_clears.is_empty() {
                let drained: Vec<_> = sim.tile_clears.drain(..).collect();
                for clear in drained {
                    sim.apply_source_terminal_static_replacement(&cod, &clear);
                    apply_tile_clear_event(&mut islands, &cod, clear.clone());
                    push_no_ruin_backing_tiles(&mut islands, &sim.source_static_map_roots, &clear);
                    refresh_simulation_island_map(&mut sim, &islands, &cod, clear.island_id);
                }
                if speech_enabled {
                    if let (Some(sfx), Some(handle)) = (
                        voice_attack_slot.or(event_destroy_slot),
                        &audio.stream_handle,
                    ) {
                        audio.waves.play_once(
                            sfx,
                            WINDOW_W as i32 / 2,
                            WINDOW_H as i32 / 2,
                            handle,
                        );
                    }
                }
                needs_redraw = true;
            }

            // `FUN_0047c080` has already changed the source city and
            // kind-13 location tables. The simulation applies the static
            // map-writer half; this frontend additionally patches its
            // scenario-tile overlay for the renderer.
            let replacements = sim.drain_source_kind13_replacements(&cod);
            if !replacements.is_empty() {
                for replacement in replacements {
                    if apply_kind13_replacement_command(&mut islands, replacement) {
                        refresh_simulation_island_map(
                            &mut sim,
                            &islands,
                            &cod,
                            replacement.island_id,
                        );
                    }
                }
                needs_redraw = true;
            }
        }

        // Audio tick: cleanup finished sounds, auto-advance music
        audio.work_events();
        if music_enabled && !music_files.is_empty() {
            // Check if current track finished, advance to next
            if let Some(slot) = music_slot {
                if audio.streams.status(slot) == anno_audio::stream::StreamStatus::Stopped {
                    // Track might have finished naturally (sink empty)
                    audio.streams.destroy(slot);
                    current_track = (current_track + 1) % music_files.len();
                    if let Some(new_slot) = audio.streams.create(&music_files[current_track], 0) {
                        if let Some(ref handle) = audio.stream_handle {
                            audio.streams.play(new_slot, music_volume, handle);
                        }
                        music_slot = Some(new_slot);
                    }
                }
            }
        }

        // Re-render terrain when needed
        if needs_redraw && !islands.is_empty() {
            let sprites = &sprites_by_zoom[sprite_zoom];
            let num_sprites = sprites.len();
            let tile_w = ZOOM_TILE_W[sprite_zoom];
            let tile_h = ZOOM_TILE_H[sprite_zoom];
            if world_mode {
                let (rgba, w, h, ox, oy) = render_world(
                    &islands,
                    sprites,
                    num_sprites,
                    tile_w,
                    tile_h,
                    &sim.buildings,
                    &sim.source_map_cell_states,
                    &cod,
                );
                rendered = Some(RenderState {
                    rgba,
                    width: w,
                    height: h,
                    origin_x: ox,
                    origin_y: oy,
                    tile_w,
                    tile_h,
                });
            } else {
                let island = &islands[current_island];
                let (rgba, w, h, ox, oy) = render_island(
                    island,
                    sprites,
                    num_sprites,
                    tile_w,
                    tile_h,
                    &sim.buildings,
                    &sim.source_map_cell_states,
                    &cod,
                );
                rendered = Some(RenderState {
                    rgba,
                    width: w,
                    height: h,
                    origin_x: ox,
                    origin_y: oy,
                    tile_w,
                    tile_h,
                });
            }
            needs_redraw = false;
        }

        // Draw
        canvas.set_draw_color(sdl2::pixels::Color::RGB(BG_COLOR.0, BG_COLOR.1, BG_COLOR.2));
        canvas.clear();

        if let Some(ref rs) = rendered {
            if rs.width > 0 && rs.height > 0 {
                let mut texture = texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGBA32, rs.width, rs.height)
                    .expect("texture creation failed");

                // Copy terrain to a mutable buffer, then overlay dynamic entities
                let mut frame = rs.rgba.clone();
                overlay_entities(
                    &mut frame,
                    rs.width,
                    rs.height,
                    rs.origin_x,
                    rs.origin_y,
                    rs.tile_w,
                    rs.tile_h,
                    &sim,
                    world_mode,
                    if world_mode {
                        None
                    } else {
                        Some(&islands[current_island])
                    },
                    &islands,
                    &carrier_sprites[sprite_zoom],
                    &worker_sprites[sprite_zoom],
                    &shadow_sprites[sprite_zoom],
                    &ship_sprites[sprite_zoom],
                    &soldier_sprites[sprite_zoom],
                    &selected_units,
                    selected_trade_ship_idx,
                    carrier_walk_anz,
                    carrier_empty_anim_offs,
                    carrier_loaded_anim_offs,
                    carrier_shadow_layout,
                    city_cart_shadow_layout,
                    carrier_shadow_y_offset,
                    city_cart_shadow_y_offset,
                    civilian_shadow_y_offset,
                    ship_sprite_layout,
                    soldier_sprite_layout,
                    entity_visual_elapsed_ms,
                );

                // Selected-building service-radius preview. Anno
                // surfaced this by clicking a public building (church
                // / tavern / school / marketplace) — the info window
                // showed the served-tiles diamond. We draw it as a
                // ring-outline on the iso grid.
                if !world_mode {
                    if let Some(bi) = selected_building_idx {
                        if bi < sim.buildings.len() {
                            let b = &sim.buildings[bi];
                            let def = &defs[b.def_id as usize];
                            // Towers / castles use defensive_cannons
                            // range; everything else uses def.radius.
                            // RE: combat::tick_tower_defense uses
                            // `range = 4 + defensive_cannons`.
                            let effective_radius = if def.defensive_cannons > 0 {
                                4 + def.defensive_cannons as u16
                            } else {
                                def.radius
                            };
                            if effective_radius > 0 && b.island_id == islands[current_island].number
                            {
                                let half_tw = rs.tile_w / 2;
                                let half_th = rs.tile_h / 2;
                                let cx = b.tile_x as i32 + def.width as i32 / 2;
                                let cy = b.tile_y as i32 + def.height as i32 / 2;
                                let r = effective_radius as i32;
                                let outline = if def.defensive_cannons > 0 {
                                    [0xFF, 0x40, 0x20, 0xFF] // tower-red
                                } else {
                                    match def.prod_kind.as_str() {
                                        "MARKT" | "KONTOR" => [0xFF, 0xE0, 0x40, 0xFF],
                                        "KIRCHE" | "KAPELLE" => [0xFF, 0xCC, 0xCC, 0xFF],
                                        "WIRT" => [0xFF, 0x88, 0x40, 0xFF],
                                        "SCHULE" | "HOCHSCHULE" => [0x80, 0xC0, 0xFF, 0xFF],
                                        "KLINIK" => [0xFF, 0x60, 0x60, 0xFF],
                                        "THEATER" | "BADEHAUS" => [0xC0, 0x80, 0xFF, 0xFF],
                                        _ => [0x80, 0xFF, 0xC0, 0xFF],
                                    }
                                };
                                // Manhattan-distance ring (matches the
                                // coverage::apply_radius diamond).
                                for dy in -r..=r {
                                    let dx_at_dy = r - dy.abs();
                                    for &dx in &[-dx_at_dy, dx_at_dy] {
                                        let tx = cx + dx;
                                        let ty = cy + dy;
                                        let sx = rs.origin_x + (tx - ty) * half_tw;
                                        let sy = rs.origin_y + (tx + ty) * half_th;
                                        let cx_pix = sx + half_tw;
                                        let cy_pix = sy + half_th;
                                        for offset in 0..=2 {
                                            for sign in [-1i32, 1] {
                                                let px = cx_pix + offset * sign;
                                                let py = cy_pix;
                                                if px < 0
                                                    || py < 0
                                                    || (px as u32) >= rs.width
                                                    || (py as u32) >= rs.height
                                                {
                                                    continue;
                                                }
                                                let off = ((py as u32 * rs.width + px as u32) * 4)
                                                    as usize;
                                                if off + 3 < frame.len() {
                                                    frame[off] = outline[0];
                                                    frame[off + 1] = outline[1];
                                                    frame[off + 2] = outline[2];
                                                    frame[off + 3] = 0xFF;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw build mode cursor
                if placer.active && !world_mode {
                    if let Some((hover_tx, hover_ty)) = placer.hover_tile {
                        if let Some(bb) = placer.selected_building() {
                            let def = &defs[bb.def_idx];
                            let island = &islands[current_island];
                            let island_map_idx = sim
                                .island_maps
                                .iter()
                                .position(|m| m.island_id == island.number);
                            let can_place = island_map_idx.map_or(false, |idx| {
                                can_place_building(
                                    island,
                                    &sim.island_maps[idx],
                                    hover_tx,
                                    hover_ty,
                                    def.width,
                                    def.height,
                                )
                            });
                            let color = if can_place {
                                [0x00, 0xFF, 0x00, 0x80] // Green: valid
                            } else {
                                [0xFF, 0x00, 0x00, 0x80] // Red: invalid
                            };

                            let half_tw = rs.tile_w / 2;
                            let half_th = rs.tile_h / 2;
                            let sprites = &sprites_by_zoom[sprite_zoom];
                            let cod_b = &cod.buildings[bb.def_idx];
                            let stride = (cod_b.anim_anz.max(1) * cod_b.anim_add.max(1)) as usize;
                            let rot_offset = placer.orientation as usize * stride;
                            // First pass: blit each tile of the building
                            // sprite at 50% alpha so the player sees what
                            // they're about to place, animated frame and
                            // current rotation included.
                            for dy in 0..def.height as i32 {
                                for dx in 0..def.width as i32 {
                                    let tx = hover_tx + dx;
                                    let ty = hover_ty + dy;
                                    let sx = rs.origin_x + (tx - ty) * half_tw;
                                    let sy = rs.origin_y + (tx + ty) * half_th;
                                    let static_idx = bb.sprite_idx
                                        + rot_offset
                                        + dy as usize * def.width as usize
                                        + dx as usize;
                                    if let Some(sp) = sprites.get(static_idx) {
                                        let bw = sp.0 as i32;
                                        let bh = sp.1 as i32;
                                        let data = &sp.2;
                                        let dst_x = sx + (rs.tile_w - bw) / 2;
                                        let dst_y = sy - (bh - rs.tile_h);
                                        for py in 0..bh {
                                            for px in 0..bw {
                                                let off_src = ((py * bw + px) * 4) as usize;
                                                if off_src + 3 >= data.len() {
                                                    continue;
                                                }
                                                if data[off_src + 3] == 0 {
                                                    continue;
                                                }
                                                let fx = dst_x + px;
                                                let fy = dst_y + py;
                                                if fx < 0 || fy < 0 {
                                                    continue;
                                                }
                                                if (fx as u32) >= rs.width
                                                    || (fy as u32) >= rs.height
                                                {
                                                    continue;
                                                }
                                                let off_dst = ((fy as u32 * rs.width + fx as u32)
                                                    * 4)
                                                    as usize;
                                                if off_dst + 3 >= frame.len() {
                                                    continue;
                                                }
                                                frame[off_dst] = ((data[off_src] as u16
                                                    + frame[off_dst] as u16)
                                                    / 2)
                                                    as u8;
                                                frame[off_dst + 1] = ((data[off_src + 1] as u16
                                                    + frame[off_dst + 1] as u16)
                                                    / 2)
                                                    as u8;
                                                frame[off_dst + 2] = ((data[off_src + 2] as u16
                                                    + frame[off_dst + 2] as u16)
                                                    / 2)
                                                    as u8;
                                                frame[off_dst + 3] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                            // Second pass: green/red footprint overlay so
                            // validity is unambiguous.
                            for dy in 0..def.height as i32 {
                                for dx in 0..def.width as i32 {
                                    let tx = hover_tx + dx;
                                    let ty = hover_ty + dy;
                                    let sx = rs.origin_x + (tx - ty) * half_tw;
                                    let sy = rs.origin_y + (tx + ty) * half_th;
                                    for py in 0..rs.tile_h {
                                        for px in 0..rs.tile_w {
                                            let fx = sx + px;
                                            let fy = sy + py;
                                            if fx >= 0
                                                && fy >= 0
                                                && (fx as u32) < rs.width
                                                && (fy as u32) < rs.height
                                            {
                                                let off = ((fy as u32 * rs.width + fx as u32) * 4)
                                                    as usize;
                                                if off + 3 < frame.len() {
                                                    let a = color[3] as u16;
                                                    let inv_a = 255 - a;
                                                    frame[off] = ((color[0] as u16 * a
                                                        + frame[off] as u16 * inv_a)
                                                        / 255)
                                                        as u8;
                                                    frame[off + 1] = ((color[1] as u16 * a
                                                        + frame[off + 1] as u16 * inv_a)
                                                        / 255)
                                                        as u8;
                                                    frame[off + 2] = ((color[2] as u16 * a
                                                        + frame[off + 2] as u16 * inv_a)
                                                        / 255)
                                                        as u8;
                                                    frame[off + 3] = 255;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw inspection highlight
                if let Some(ref insp) = inspection {
                    let half_tw = rs.tile_w / 2;
                    let half_th = rs.tile_h / 2;
                    let (highlight_w, highlight_h) = if let Some(bi) = insp.building_idx {
                        let def = &defs[sim.buildings[bi].def_id as usize];
                        (def.width as i32, def.height as i32)
                    } else {
                        (1, 1)
                    };
                    let base_tx = if let Some(bi) = insp.building_idx {
                        sim.buildings[bi].tile_x as i32
                    } else {
                        insp.tile_x
                    };
                    let base_ty = if let Some(bi) = insp.building_idx {
                        sim.buildings[bi].tile_y as i32
                    } else {
                        insp.tile_y
                    };
                    let highlight_color = [0xFF, 0xFF, 0x00, 0x60]; // Yellow semi-transparent
                    for dy in 0..highlight_h {
                        for dx in 0..highlight_w {
                            let tx = base_tx + dx;
                            let ty = base_ty + dy;
                            let sx = rs.origin_x + (tx - ty) * half_tw;
                            let sy = rs.origin_y + (tx + ty) * half_th;
                            // Draw diamond shape for isometric tile
                            for py in 0..rs.tile_h {
                                let row_half = if py <= rs.tile_h / 2 {
                                    py * half_tw / half_th.max(1)
                                } else {
                                    (rs.tile_h - py) * half_tw / half_th.max(1)
                                };
                                let row_start = half_tw - row_half;
                                let row_end = half_tw + row_half;
                                for px in row_start..row_end {
                                    let fx = sx + px;
                                    let fy = sy + py;
                                    if fx >= 0
                                        && fy >= 0
                                        && (fx as u32) < rs.width
                                        && (fy as u32) < rs.height
                                    {
                                        let off = ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                        if off + 3 < frame.len() {
                                            let a = highlight_color[3] as u16;
                                            let inv_a = 255 - a;
                                            frame[off] = ((highlight_color[0] as u16 * a
                                                + frame[off] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 1] = ((highlight_color[1] as u16 * a
                                                + frame[off + 1] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 2] = ((highlight_color[2] as u16 * a
                                                + frame[off + 2] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 3] = 255;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw status tints for dried-up plantations
                // (yellow) and depleted mines (gray), so the player
                // can spot non-functional buildings at a glance.
                if !world_mode {
                    let island_id = islands[current_island].number;
                    let half_tw = rs.tile_w / 2;
                    let half_th = rs.tile_h / 2;
                    for b in sim.buildings.iter() {
                        if b.island_id != island_id {
                            continue;
                        }
                        let def = &defs[b.def_id as usize];
                        let mut tint: Option<[u8; 4]> = None;
                        if def.can_dry_up && !b.active {
                            tint = Some([0xC0, 0xA0, 0x40, 0x80]); // yellow
                        } else if def.ore_deposit != anno_sim::building::OreDeposit::None
                            && b.remaining_ore == 0
                        {
                            tint = Some([0x80, 0x80, 0x80, 0x80]); // gray
                        }
                        let Some(tint) = tint else {
                            continue;
                        };
                        let bw = def.width as i32;
                        let bh = def.height as i32;
                        let bx = b.tile_x as i32;
                        let by = b.tile_y as i32;
                        for dy in 0..bh {
                            for dx in 0..bw {
                                let tx = bx + dx;
                                let ty = by + dy;
                                let sx = rs.origin_x + (tx - ty) * half_tw;
                                let sy = rs.origin_y + (tx + ty) * half_th;
                                for py in 0..rs.tile_h {
                                    let row_half = if py <= rs.tile_h / 2 {
                                        py * half_tw / half_th.max(1)
                                    } else {
                                        (rs.tile_h - py) * half_tw / half_th.max(1)
                                    };
                                    for px in (half_tw - row_half)..(half_tw + row_half) {
                                        let fx = sx + px;
                                        let fy = sy + py;
                                        if fx < 0 || fy < 0 {
                                            continue;
                                        }
                                        if (fx as u32) >= rs.width || (fy as u32) >= rs.height {
                                            continue;
                                        }
                                        let off = ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                        if off + 3 < frame.len() {
                                            let a = tint[3] as u16;
                                            let inv_a = 255 - a;
                                            frame[off] = ((tint[0] as u16 * a
                                                + frame[off] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 1] = ((tint[1] as u16 * a
                                                + frame[off + 1] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 2] = ((tint[2] as u16 * a
                                                + frame[off + 2] as u16 * inv_a)
                                                / 255)
                                                as u8;
                                            frame[off + 3] = 255;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Draw construction overlay (blue tint + progress bar) for
                // any building still being built on the visible island.
                if !world_mode {
                    let island_id = islands[current_island].number;
                    let half_tw = rs.tile_w / 2;
                    let half_th = rs.tile_h / 2;
                    for b in sim.buildings.iter() {
                        if b.is_built() || b.island_id != island_id {
                            continue;
                        }
                        let def = &defs[b.def_id as usize];
                        let bw = def.width as i32;
                        let bh = def.height as i32;
                        let bx = b.tile_x as i32;
                        let by = b.tile_y as i32;
                        // Blue tint per tile (semi-transparent diamond).
                        let tint: [u8; 4] = [0x40, 0x80, 0xFF, 0x60];
                        for dy in 0..bh {
                            for dx in 0..bw {
                                let tx = bx + dx;
                                let ty = by + dy;
                                let sx = rs.origin_x + (tx - ty) * half_tw;
                                let sy = rs.origin_y + (tx + ty) * half_th;
                                for py in 0..rs.tile_h {
                                    let row_half = if py <= rs.tile_h / 2 {
                                        py * half_tw / half_th.max(1)
                                    } else {
                                        (rs.tile_h - py) * half_tw / half_th.max(1)
                                    };
                                    let row_start = half_tw - row_half;
                                    let row_end = half_tw + row_half;
                                    for px in row_start..row_end {
                                        let fx = sx + px;
                                        let fy = sy + py;
                                        if fx >= 0
                                            && fy >= 0
                                            && (fx as u32) < rs.width
                                            && (fy as u32) < rs.height
                                        {
                                            let off =
                                                ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                            if off + 3 < frame.len() {
                                                let a = tint[3] as u16;
                                                let inv_a = 255 - a;
                                                frame[off] = ((tint[0] as u16 * a
                                                    + frame[off] as u16 * inv_a)
                                                    / 255)
                                                    as u8;
                                                frame[off + 1] = ((tint[1] as u16 * a
                                                    + frame[off + 1] as u16 * inv_a)
                                                    / 255)
                                                    as u8;
                                                frame[off + 2] = ((tint[2] as u16 * a
                                                    + frame[off + 2] as u16 * inv_a)
                                                    / 255)
                                                    as u8;
                                                frame[off + 3] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // Progress bar: above the building (top-back of footprint).
                        let cx_tile = bx;
                        let cy_tile = by;
                        let bar_sx = rs.origin_x + (cx_tile - cy_tile) * half_tw + half_tw
                            - bw * half_tw / 2;
                        let bar_sy = rs.origin_y + (cx_tile + cy_tile) * half_th - 4;
                        let bar_w = (bw + bh) * half_tw / 2;
                        let bar_h = 3i32;
                        let prog = b.construction_progress_128() as i32;
                        let filled = bar_w * prog / 128;
                        for by2 in 0..bar_h {
                            for bx2 in 0..bar_w {
                                let fx = bar_sx + bx2;
                                let fy = bar_sy + by2;
                                if fx < 0 || fy < 0 {
                                    continue;
                                }
                                if (fx as u32) >= rs.width || (fy as u32) >= rs.height {
                                    continue;
                                }
                                let off = ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                if off + 3 >= frame.len() {
                                    continue;
                                }
                                let color = if bx2 < filled {
                                    [0x40, 0xFF, 0x40, 0xFF]
                                } else {
                                    [0x20, 0x20, 0x20, 0xFF]
                                };
                                frame[off] = color[0];
                                frame[off + 1] = color[1];
                                frame[off + 2] = color[2];
                                frame[off + 3] = color[3];
                            }
                        }
                    }
                }

                // Draw demolish mode hover highlight (red)
                if demolish_mode {
                    if let Some(bi) = demolish_hover {
                        let b = &sim.buildings[bi];
                        let def = &defs[b.def_id as usize];
                        let half_tw = rs.tile_w / 2;
                        let half_th = rs.tile_h / 2;
                        let demo_color = [0xFF, 0x20, 0x20, 0x70]; // Red semi-transparent
                        for dy in 0..def.height as i32 {
                            for dx in 0..def.width as i32 {
                                let tx = b.tile_x as i32 + dx;
                                let ty = b.tile_y as i32 + dy;
                                let sx = rs.origin_x + (tx - ty) * half_tw;
                                let sy = rs.origin_y + (tx + ty) * half_th;
                                for py in 0..rs.tile_h {
                                    for px in 0..rs.tile_w {
                                        let fx = sx + px;
                                        let fy = sy + py;
                                        if fx >= 0
                                            && fy >= 0
                                            && (fx as u32) < rs.width
                                            && (fy as u32) < rs.height
                                        {
                                            let off =
                                                ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                            if off + 3 < frame.len() {
                                                let a = demo_color[3] as u16;
                                                let inv_a = 255 - a;
                                                frame[off] = ((demo_color[0] as u16 * a
                                                    + frame[off] as u16 * inv_a)
                                                    / 255)
                                                    as u8;
                                                frame[off + 1] = ((demo_color[1] as u16 * a
                                                    + frame[off + 1] as u16 * inv_a)
                                                    / 255)
                                                    as u8;
                                                frame[off + 2] = ((demo_color[2] as u16 * a
                                                    + frame[off + 2] as u16 * inv_a)
                                                    / 255)
                                                    as u8;
                                                frame[off + 3] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Fog-of-war: dim every island tile that hasn't been
                // explored yet. Pulled from `sim.exploration` (per-island
                // bitmap). Only relevant in single-island mode.
                if !world_mode {
                    let island = &islands[current_island];
                    let island_id = island.number;
                    if let Some(em) = sim.exploration.iter().find(|e| e.island_id == island_id) {
                        let half_tw = rs.tile_w / 2;
                        let half_th = rs.tile_h / 2;
                        for tile in &island.tiles {
                            if em.is_explored(tile.x as u16, tile.y as u16) {
                                continue;
                            }
                            let tx = tile.x as i32;
                            let ty = tile.y as i32;
                            let sx = rs.origin_x + (tx - ty) * half_tw;
                            let sy = rs.origin_y + (tx + ty) * half_th;
                            for py in 0..rs.tile_h {
                                let row_half = if py <= rs.tile_h / 2 {
                                    py * half_tw / half_th.max(1)
                                } else {
                                    (rs.tile_h - py) * half_tw / half_th.max(1)
                                };
                                let row_start = half_tw - row_half;
                                let row_end = half_tw + row_half;
                                for px in row_start..row_end {
                                    let fx = sx + px;
                                    let fy = sy + py;
                                    if fx < 0 || fy < 0 {
                                        continue;
                                    }
                                    if (fx as u32) >= rs.width || (fy as u32) >= rs.height {
                                        continue;
                                    }
                                    let off = ((fy as u32 * rs.width + fx as u32) * 4) as usize;
                                    if off + 3 >= frame.len() {
                                        continue;
                                    }
                                    // 60% darken toward dark gray.
                                    frame[off] = (frame[off] as u16 * 80 / 255) as u8;
                                    frame[off + 1] = (frame[off + 1] as u16 * 80 / 255) as u8;
                                    frame[off + 2] = (frame[off + 2] as u16 * 90 / 255) as u8;
                                }
                            }
                        }
                    }
                }

                texture
                    .update(None, &frame, (rs.width * 4) as usize)
                    .expect("texture update failed");

                let dst_w = (rs.width as i32 * display_zoom) as u32;
                let dst_h = (rs.height as i32 * display_zoom) as u32;
                let dst_x = (WINDOW_W as i32 - dst_w as i32) / 2 + scroll_x;
                let dst_y = (WINDOW_H as i32 - dst_h as i32) / 2 + scroll_y;

                canvas
                    .copy(&texture, None, Some(Rect::new(dst_x, dst_y, dst_w, dst_h)))
                    .ok();

                // Draw minimap in the bottom-right corner
                let minimap_max_w = 200u32;
                let minimap_max_h = 150u32;
                let minimap_margin = 8i32;

                // Scale to fit minimap bounds while preserving aspect ratio
                let scale_x = minimap_max_w as f64 / rs.width as f64;
                let scale_y = minimap_max_h as f64 / rs.height as f64;
                let mini_scale = scale_x.min(scale_y).min(1.0);
                let mini_w = (rs.width as f64 * mini_scale) as u32;
                let mini_h = (rs.height as f64 * mini_scale) as u32;

                if mini_w > 0 && mini_h > 0 {
                    // Render downscaled minimap RGBA
                    let mut mini_rgba = vec![0x20u8; (mini_w * mini_h * 4) as usize];
                    for my in 0..mini_h {
                        for mx in 0..mini_w {
                            let src_x = (mx as f64 / mini_scale) as u32;
                            let src_y = (my as f64 / mini_scale) as u32;
                            if src_x < rs.width && src_y < rs.height {
                                let src_off = ((src_y * rs.width + src_x) * 4) as usize;
                                let dst_off = ((my * mini_w + mx) * 4) as usize;
                                if src_off + 3 < frame.len() && dst_off + 3 < mini_rgba.len() {
                                    mini_rgba[dst_off] = frame[src_off];
                                    mini_rgba[dst_off + 1] = frame[src_off + 1];
                                    mini_rgba[dst_off + 2] = frame[src_off + 2];
                                    mini_rgba[dst_off + 3] =
                                        if frame[src_off + 3] > 0 { 220 } else { 80 };
                                }
                            }
                        }
                    }

                    // Building dots: stamp a small per-owner-coloured
                    // pixel cluster at each building's iso position.
                    // Helps players locate their settlements at a
                    // glance the way the original minimap showed them.
                    if !world_mode {
                        let half_tw = rs.tile_w / 2;
                        let half_th = rs.tile_h / 2;
                        for b in &sim.buildings {
                            if !b.active {
                                continue;
                            }
                            if b.island_id != islands[current_island].number {
                                continue;
                            }
                            let tx = b.tile_x as i32;
                            let ty = b.tile_y as i32;
                            let sx = rs.origin_x + (tx - ty) * half_tw;
                            let sy = rs.origin_y + (tx + ty) * half_th;
                            // Project to minimap pixel.
                            let mx = (sx as f64 * mini_scale) as i32;
                            let my = (sy as f64 * mini_scale) as i32;
                            let color = match b.owner {
                                0 => [0x40, 0xFF, 0x80, 0xFF], // human green
                                1 => [0xFF, 0x60, 0x60, 0xFF], // red
                                2 => [0x60, 0x80, 0xFF, 0xFF], // blue
                                3 => [0xFF, 0xE0, 0x40, 0xFF], // yellow
                                _ => [0xCC, 0xCC, 0xCC, 0xFF],
                            };
                            // 2x2 dot for visibility.
                            for dy in 0..2 {
                                for dx in 0..2 {
                                    let px = mx + dx;
                                    let py = my + dy;
                                    if px < 0
                                        || py < 0
                                        || px >= mini_w as i32
                                        || py >= mini_h as i32
                                    {
                                        continue;
                                    }
                                    let off = ((py as u32 * mini_w + px as u32) * 4) as usize;
                                    if off + 3 < mini_rgba.len() {
                                        mini_rgba[off..off + 4].copy_from_slice(&color);
                                    }
                                }
                            }
                        }
                    }

                    // Draw viewport rectangle on minimap
                    // The viewport in texture coords:
                    let vp_left = ((-scroll_x) as f64 / display_zoom as f64 * mini_scale) as i32;
                    let vp_top = ((-scroll_y) as f64 / display_zoom as f64 * mini_scale) as i32;
                    let vp_w = (WINDOW_W as f64 / display_zoom as f64 * mini_scale) as i32;
                    let vp_h = (WINDOW_H as f64 / display_zoom as f64 * mini_scale) as i32;

                    // Adjust for centering offset
                    let center_off_x = ((WINDOW_W as i32 - dst_w as i32) / 2) as f64
                        / display_zoom as f64
                        * mini_scale;
                    let center_off_y = ((WINDOW_H as i32 - dst_h as i32) / 2) as f64
                        / display_zoom as f64
                        * mini_scale;
                    let vp_x = vp_left - center_off_x as i32;
                    let vp_y = vp_top - center_off_y as i32;

                    // Draw viewport rect border (white)
                    let white = [0xFF, 0xFF, 0xFF, 0xFF];
                    for px in vp_x.max(0)..=(vp_x + vp_w).min(mini_w as i32 - 1) {
                        for &py in &[vp_y, vp_y + vp_h] {
                            if py >= 0 && py < mini_h as i32 {
                                let off = ((py as u32 * mini_w + px as u32) * 4) as usize;
                                if off + 3 < mini_rgba.len() {
                                    mini_rgba[off..off + 4].copy_from_slice(&white);
                                }
                            }
                        }
                    }
                    for py in vp_y.max(0)..=(vp_y + vp_h).min(mini_h as i32 - 1) {
                        for &px in &[vp_x, vp_x + vp_w] {
                            if px >= 0 && px < mini_w as i32 {
                                let off = ((py as u32 * mini_w + px as u32) * 4) as usize;
                                if off + 3 < mini_rgba.len() {
                                    mini_rgba[off..off + 4].copy_from_slice(&white);
                                }
                            }
                        }
                    }

                    // Blit minimap to a texture and draw it
                    if let Ok(mut mini_tex) = texture_creator.create_texture_streaming(
                        PixelFormatEnum::RGBA32,
                        mini_w,
                        mini_h,
                    ) {
                        mini_tex
                            .update(None, &mini_rgba, (mini_w * 4) as usize)
                            .ok();
                        mini_tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                        let mini_x = WINDOW_W as i32 - mini_w as i32 - minimap_margin;
                        let mini_y = WINDOW_H as i32 - mini_h as i32 - minimap_margin;

                        // Draw dark background behind minimap
                        canvas.set_draw_color(sdl2::pixels::Color::RGBA(0, 0, 0, 180));
                        canvas
                            .fill_rect(Rect::new(mini_x - 2, mini_y - 2, mini_w + 4, mini_h + 4))
                            .ok();

                        canvas
                            .copy(
                                &mini_tex,
                                None,
                                Some(Rect::new(mini_x, mini_y, mini_w, mini_h)),
                            )
                            .ok();

                        // Handle minimap clicks — clicking the minimap scrolls the main view
                        if minimap_clicked {
                            // Convert minimap click to texture coordinates
                            let click_tex_x = (minimap_click_x - mini_x) as f64 / mini_scale;
                            let click_tex_y = (minimap_click_y - mini_y) as f64 / mini_scale;
                            // Center the viewport on the clicked point
                            scroll_x = -(click_tex_x as i32 * display_zoom) + WINDOW_W as i32 / 2;
                            scroll_y = -(click_tex_y as i32 * display_zoom) + WINDOW_H as i32 / 2;
                            minimap_clicked = false;
                        }
                    }
                }
            }
        }

        // Draw population/economy HUD in top-left corner
        if !placer.active {
            if let Some(ref player) = sim.players.first() {
                let tier_names = ["Pioneer", "Settler", "Citizen", "Merchant", "Aristocrat"];
                let hud_scale = 2u32;
                let line_h = (tiny_font::measure("X", hud_scale) + hud_scale) as i32 + 2;
                let hud_w = 220u32;
                let mut lines: Vec<String> = Vec::new();

                // Population tiers
                let total_pop: u32 = player.population.iter().sum();
                lines.push(format!("POP {}", total_pop));
                for i in 0..5 {
                    let pop = player.population[i];
                    if pop > 0 {
                        let sat = player.satisfaction[i] as u32 * 100 / 128;
                        let tax = player.tax_rates[i] as u32 * 100 / 128;
                        lines.push(format!(
                            " {}:{} S{}% T{}%",
                            &tier_names[i][..3],
                            pop,
                            sat,
                            tax
                        ));
                    }
                }
                // Economy
                lines.push(format!("GOLD {}", player.gold));

                let hud_h = (lines.len() as u32 * line_h as u32) + 8;
                let mut hud_buf = vec![0u8; (hud_w * hud_h * 4) as usize];
                // Fill with semi-transparent dark background
                for i in 0..(hud_w * hud_h) as usize {
                    hud_buf[i * 4] = 0;
                    hud_buf[i * 4 + 1] = 0;
                    hud_buf[i * 4 + 2] = 0x10;
                    hud_buf[i * 4 + 3] = 180;
                }
                // Render text lines
                for (li, line) in lines.iter().enumerate() {
                    let color = if line.starts_with("GOLD") {
                        [0xFF, 0xD7, 0x00, 0xFF] // Gold color
                    } else if line.starts_with("POP") {
                        [0xFF, 0xFF, 0xFF, 0xFF] // White
                    } else {
                        [0xCC, 0xCC, 0xCC, 0xFF] // Light gray
                    };
                    tiny_font::draw_str(
                        &mut hud_buf,
                        hud_w,
                        hud_h,
                        4,
                        4 + li as i32 * line_h,
                        line,
                        color,
                        hud_scale,
                    );
                }

                if let Ok(mut hud_tex) =
                    texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, hud_w, hud_h)
                {
                    hud_tex.update(None, &hud_buf, (hud_w * 4) as usize).ok();
                    hud_tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                    canvas
                        .copy(&hud_tex, None, Some(Rect::new(8, 8, hud_w, hud_h)))
                        .ok();
                }
            }
        }

        // Inspection detail panel (top-right). Multi-line read-out for
        // whatever the player right-clicked on — fed by the same
        // `inspection` state that drives the title-bar summary.
        if let Some(ref insp) = inspection {
            let mut lines: Vec<(String, [u8; 4])> = Vec::new();
            // Fertility badge for the active island so the player can
            // see what plantations are buildable here.
            if !world_mode {
                let isl = &islands[current_island];
                let has_fertility = !isl.active_fertilities().is_empty();
                lines.push((
                    format!("Fertility: {}", fertility_list_label(isl)),
                    if has_fertility {
                        [0xAA, 0xFF, 0xAA, 0xFF]
                    } else {
                        [0xCC, 0xCC, 0xCC, 0xFF]
                    },
                ));
            }
            // Header.
            if let Some(bi) = insp.building_idx {
                let b = &sim.buildings[bi];
                let def = &defs[b.def_id as usize];
                let name = cod.buildings[b.def_id as usize]
                    .properties
                    .get("Name")
                    .cloned()
                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                lines.push((name, [0xFF, 0xD7, 0x00, 0xFF]));
                lines.push((
                    format!(
                        "Tile ({},{}) {}x{}",
                        b.tile_x, b.tile_y, def.width, def.height
                    ),
                    [0xCC, 0xCC, 0xCC, 0xFF],
                ));
                lines.push((
                    format!("Owner: p{}  Kind: {}", b.owner, def.kind),
                    [0xCC, 0xCC, 0xCC, 0xFF],
                ));
                if !b.is_built() {
                    let pct = b.construction_progress_128() as u32 * 100 / 128;
                    lines.push((format!("Construction: {pct}%"), [0x66, 0xCC, 0xFF, 0xFF]));
                }
                if def.output_good != Good::None {
                    lines.push((
                        format!(
                            "Out: {:?} {}/{}",
                            def.output_good, b.output_stock, def.storage_capacity
                        ),
                        [0xCC, 0xFF, 0xCC, 0xFF],
                    ));
                    if def.input_good_1 != Good::None {
                        lines.push((
                            format!("In1: {:?} {}", def.input_good_1, b.input_1_stock),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                    if def.input_good_2 != Good::None {
                        lines.push((
                            format!("In2: {:?} {}", def.input_good_2, b.input_2_stock),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                    let eff_pct = b.efficiency as u32 * 100 / 128;
                    lines.push((format!("Efficiency: {eff_pct}%"), [0xCC, 0xCC, 0xCC, 0xFF]));
                }
                if def.maintenance_cost > 0 {
                    lines.push((
                        format!("Upkeep: {}/tick", def.maintenance_cost),
                        [0xCC, 0xCC, 0xCC, 0xFF],
                    ));
                }
                // Residence: surface per-good demand fulfillment for this
                // tier so the player can spot which need is dragging
                // satisfaction down.
                let is_residence = def.kind == "WOHN" || def.prod_kind == "WOHN";
                if is_residence {
                    let tier =
                        (b.house_tier as usize).min(anno_sim::population::TIER_DEMANDS.len() - 1);
                    if let Some(p) = sim.players.first() {
                        for &g in anno_sim::population::TIER_DEMANDS[tier] {
                            // Find the demand slot index by good — match
                            // population::DEMAND_GOODS.
                            let slot_idx = anno_sim::population::DEMAND_GOODS
                                .iter()
                                .position(|&dg| dg == g);
                            let pct = match slot_idx {
                                Some(i) if p.demands[i].demand > 0 => {
                                    (p.demands[i].supply as u64 * 100 / p.demands[i].demand as u64)
                                        as u32
                                }
                                _ => 100,
                            };
                            let color = if pct < 50 {
                                [0xFF, 0x88, 0x66, 0xFF]
                            } else if pct < 90 {
                                [0xFF, 0xCC, 0x66, 0xFF]
                            } else {
                                [0xCC, 0xFF, 0xCC, 0xFF]
                            };
                            lines.push((format!("  {:?}: {}%", g, pct), color));
                        }
                    }
                }
            } else {
                lines.push((
                    format!("Tile ({},{})", insp.tile_x, insp.tile_y),
                    [0xFF, 0xD7, 0x00, 0xFF],
                ));
                // Show stored orientation when this is a non-default tile
                // (loaded from SZS, etc.) so authors can debug rotation.
                let tile = islands[current_island]
                    .tiles
                    .iter()
                    .find(|t| t.x as i32 == insp.tile_x && t.y as i32 == insp.tile_y);
                if let Some(t) = tile {
                    if t.orientation != 0 {
                        lines.push((
                            format!("Orientation: {}", t.orientation),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                }
            }
            if let Some(wi) = insp.warehouse_idx {
                let wh = &sim.warehouses[wi];
                lines.push((
                    format!(
                        "Warehouse @ ({},{})  owner p{}",
                        wh.tile_x, wh.tile_y, wh.owner
                    ),
                    [0xFF, 0xD7, 0x00, 0xFF],
                ));
                let stocks = wh.all_stock();
                if stocks.is_empty() {
                    lines.push(("(empty)".to_string(), [0x88, 0x88, 0x88, 0xFF]));
                } else {
                    for (g, qty, cap) in stocks.iter().take(8) {
                        lines.push((
                            format!("  {:?}: {}/{}", g, qty, cap),
                            [0xCC, 0xCC, 0xCC, 0xFF],
                        ));
                    }
                    if stocks.len() > 8 {
                        lines.push((
                            format!("  +{} more…", stocks.len() - 8),
                            [0x88, 0x88, 0x88, 0xFF],
                        ));
                    }
                }
            }
            // Render panel.
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 280u32;
            let panel_h = (lines.len() as i32 * line_h + 8) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            for (i, (text, color)) in lines.iter().enumerate() {
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    4 + i as i32 * line_h,
                    text,
                    *color,
                    scale,
                );
            }
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = WINDOW_W as i32 - panel_w as i32 - 8;
                let ty = 8i32;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h)))
                    .ok();
            }
        }

        // Draw diplomacy panel (center-top)
        if diplomacy_panel {
            use anno_sim::combat::Diplomacy;
            let dscale = 2u32;
            let line_h = 14i32;
            let panel_w = 320u32;
            let rows = 6u32; // counterparts 1..6
            let panel_h = (rows + 3) * line_h as u32 + 12;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 210;
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4,
                "DIPLOMACY",
                [0xFF, 0xD7, 0x00, 0xFF],
                dscale,
            );
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4 + line_h,
                DIPLOMACY_PANEL_HELP,
                [0xAA, 0xAA, 0xAA, 0xFF],
                dscale,
            );
            // Per-counterpart rows (binary-verified slot layout):
            //   slot 4 = free trader (1 602 exe :83179, PLAYER4 1M gold)
            //   slot 5 = native faction (`Nativflg: 1` buildings;
            //                            manual sec. 7.5/8.6)
            //   slot 6 = pirates
            for tgt in 1u8..=6 {
                let y = 4 + (tgt as i32 + 1) * line_h;
                let rel = sim.diplomacy.get(0, tgt);
                let rel_str = match rel {
                    Diplomacy::Allied => "ALLIED",
                    Diplomacy::Neutral => "NEUTRAL",
                    Diplomacy::War => "WAR",
                };
                let selected = tgt == diplomacy_target;
                let arrow = if selected { ">" } else { " " };
                let alive = sim
                    .players
                    .get(tgt as usize)
                    .map(|p| {
                        p.state != anno_sim::player::PlayerState::Empty
                            && p.state != anno_sim::player::PlayerState::Defeated
                    })
                    .unwrap_or(false);
                let label = match tgt {
                    4 => "Free Trader",
                    5 => "Natives",
                    6 => "Pirates",
                    _ => "Player",
                };
                let suffix = if alive { "" } else { " (no player)" };
                let line = format!("{}{} {}: {}{}", arrow, label, tgt, rel_str, suffix);
                let color = if selected {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    match rel {
                        Diplomacy::War => [0xFF, 0x66, 0x66, 0xFF],
                        Diplomacy::Allied => [0x66, 0xFF, 0x66, 0xFF],
                        Diplomacy::Neutral => [0xCC, 0xCC, 0xCC, 0xFF],
                    }
                };
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &line, color, dscale);
            }
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, 8, panel_w, panel_h)))
                    .ok();
            }
        }

        // Video sequences / speech menu (F) - manual Appendix D.
        if video_speech_panel {
            let scale = 2u32;
            let line_h = 14i32;
            let panel_w = 360u32;
            let panel_h = (5 * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4,
                "VIDEO / SPEECH",
                [0xFF, 0xD7, 0x00, 0xFF],
                scale,
            );
            let rows = [
                (
                    "Video sequences",
                    if video_sequences_enabled { "ON" } else { "OFF" },
                ),
                (
                    "Speech announcements",
                    if speech_enabled { "ON" } else { "OFF" },
                ),
            ];
            for (idx, (label, state)) in rows.iter().enumerate() {
                let selected = idx == video_speech_sel;
                let arrow = if selected { ">" } else { " " };
                let line = format!("{arrow} {label}: {state}");
                let color = if selected {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    4 + line_h * (idx as i32 + 2),
                    &line,
                    color,
                    scale,
                );
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4 + line_h * 4,
                "Up/Dn pick  Lt/Rt/Enter toggle  F/Esc close",
                [0xAA, 0xAA, 0xAA, 0xFF],
                1,
            );
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, 60, panel_w, panel_h)))
                    .ok();
            }
        }

        // Options menu (O) - manual Appendix D.
        if options_panel {
            let scale = 2u32;
            let line_h = 14i32;
            let panel_w = 360u32;
            let panel_h = (7 * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4,
                "OPTIONS",
                [0xFF, 0xD7, 0x00, 0xFF],
                scale,
            );
            let music_volume_pct = (music_volume * 100.0).round() as u32;
            let rows = [
                (
                    "Music",
                    if music_enabled {
                        "ON".to_string()
                    } else {
                        "OFF".to_string()
                    },
                ),
                ("Music volume", format!("{music_volume_pct}%")),
                (
                    "Video sequences",
                    if video_sequences_enabled {
                        "ON".to_string()
                    } else {
                        "OFF".to_string()
                    },
                ),
                (
                    "Speech announcements",
                    if speech_enabled {
                        "ON".to_string()
                    } else {
                        "OFF".to_string()
                    },
                ),
            ];
            for (idx, (label, state)) in rows.iter().enumerate() {
                let selected = idx == options_sel;
                let arrow = if selected { ">" } else { " " };
                let line = format!("{arrow} {label}: {state}");
                let color = if selected {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    4 + line_h * (idx as i32 + 2),
                    &line,
                    color,
                    scale,
                );
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4 + line_h * 6,
                "Up/Dn pick  Lt/Rt/Enter change  O/Esc close",
                [0xAA, 0xAA, 0xAA, 0xFF],
                1,
            );
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, 60, panel_w, panel_h)))
                    .ok();
            }
        }

        // Own ships list (S) — manual Appendix D.
        if ship_panel {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 520u32;
            let header_h = 28i32;
            let rows = visible_ship_list_rows(&sim.trade_ships, &sim.military_units);

            let visible = rows.len().max(1);
            let panel_h = (header_h + visible as i32 * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4,
                "SHIPS (S/Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF],
                scale,
            );
            if rows.is_empty() {
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    header_h,
                    "(no ships)",
                    [0x88, 0x88, 0x88, 0xFF],
                    scale,
                );
            } else {
                for (row, ship) in rows.iter().enumerate() {
                    let color = if ship.warship {
                        [0xFF, 0xCC, 0xAA, 0xFF]
                    } else {
                        [0xAA, 0xDD, 0xFF, 0xFF]
                    };
                    let line = ship_list_line(ship);
                    tiny_font::draw_str(
                        &mut buf,
                        panel_w,
                        panel_h,
                        4,
                        header_h + row as i32 * line_h,
                        &line,
                        color,
                        scale,
                    );
                }
            }
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h)))
                    .ok();
            }
        }

        // Save / load slot picker (L). 10 named slots per scenario.
        if save_panel {
            let panel_w = 480u32;
            let header_h = 28i32;
            let line_h = 12i32;
            let panel_h = (header_h + 10 * line_h + 12) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4,
                "SAVE SLOTS (Up/Dn pick, S=save, L=load, Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF],
                1,
            );
            for slot in 0..10 {
                let path = slot_path(slot);
                let meta = std::fs::metadata(&path).ok();
                let exists = meta.is_some();
                let info = match meta {
                    Some(m) => {
                        let size_kb = (m.len() / 1024).max(1);
                        format!("{:>5} KiB", size_kb)
                    }
                    None => "(empty)".to_string(),
                };
                let arrow = if slot == save_sel { ">" } else { " " };
                let line = format!("{arrow} slot {}  {}", slot, info);
                let color = if slot == save_sel {
                    [0xFF, 0xFF, 0x00, 0xFF]
                } else if exists {
                    [0xCC, 0xFF, 0xCC, 0xFF]
                } else {
                    [0x88, 0x88, 0x88, 0xFF]
                };
                let y = header_h + slot as i32 * line_h;
                tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, &line, color, 1);
            }
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 60i32;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h)))
                    .ok();
            }
        }

        // Building info card. In info mode, clicking a building opens a
        // small floating window with name + production status. Closes
        // when the player clicks something else, presses Esc, or the
        // building is destroyed.
        if let Some(bi) = selected_building_idx {
            if bi >= sim.buildings.len() || !sim.buildings[bi].active {
                selected_building_idx = None;
            } else {
                let b = &sim.buildings[bi];
                let def = &defs[b.def_id as usize];
                let name = cod
                    .buildings
                    .get(b.def_id as usize)
                    .and_then(|p| p.properties.get("Name").cloned())
                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                let mut lines: Vec<String> = vec![
                    format!("{} (Esc close)", name),
                    format!(
                        "at ({},{}) island {} owner {}",
                        b.tile_x, b.tile_y, b.island_id, b.owner
                    ),
                    format!(
                        "size {}x{}  hp {}/{}",
                        def.width,
                        def.height,
                        b.health,
                        anno_sim::building::BUILDING_MAX_HEALTH
                    ),
                ];
                if !b.is_built() {
                    let pct = if def.cost_wood + def.cost_tools + def.cost_bricks == 0 {
                        100
                    } else {
                        let needed = (b.wood_needed + b.tools_needed + b.bricks_needed) as u32;
                        let total = (def.cost_wood + def.cost_tools + def.cost_bricks) as u32;
                        100 - (needed * 100 / total.max(1))
                    };
                    lines.push(format!("under construction ({}%)", pct));
                    lines.push(format!(
                        "needs wood:{} tools:{} bricks:{}",
                        b.wood_needed, b.tools_needed, b.bricks_needed
                    ));
                }
                if def.output_good != Good::None {
                    lines.push(format!(
                        "produces {:?}: {}/{}  eff {}%",
                        def.output_good,
                        b.output_stock,
                        def.storage_capacity,
                        b.efficiency as u32 * 100 / 128,
                    ));
                    if def.input_good_1 != Good::None {
                        lines.push(format!("in1 {:?}: {}", def.input_good_1, b.input_1_stock,));
                    }
                    if def.input_good_2 != Good::None {
                        lines.push(format!("in2 {:?}: {}", def.input_good_2, b.input_2_stock,));
                    }
                }
                if def.prod_kind == "WOHN" {
                    let tier_name = match b.house_tier {
                        0 => "Pioneer",
                        1 => "Settler",
                        2 => "Citizen",
                        3 => "Merchant",
                        _ => "Aristocrat",
                    };
                    lines.push(format!("residence tier: {}", tier_name));
                }
                if def.maintenance_cost > 0 {
                    lines.push(format!("upkeep: {}/tick", def.maintenance_cost));
                }
                // Mine deposit status (RE: haeuser.cod Erzbergnr).
                if def.ore_deposit != anno_sim::building::OreDeposit::None {
                    let total = def.ore_deposit.capacity();
                    lines.push(format!(
                        "ore deposit: {}/{} t remaining",
                        b.remaining_ore, total,
                    ));
                    if b.remaining_ore == 0 {
                        lines.push("DEPLETED".to_string());
                    }
                }
                // Defensive cannons (RE: haeuser.cod Kanon).
                if def.defensive_cannons > 0 {
                    lines.push(format!(
                        "defense: {} cannons (range {})",
                        def.defensive_cannons,
                        4 + def.defensive_cannons,
                    ));
                }
                // Drought status for plantations.
                if def.can_dry_up && !b.active {
                    lines.push("DRIED UP — bulldoze and replant".to_string());
                }
                let scale = 1u32;
                let line_h = (5 * scale + 3) as i32;
                let panel_w = 280u32;
                let panel_h = (8 + line_h * lines.len() as i32) as u32;
                let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
                for i in 0..(panel_w * panel_h) as usize {
                    buf[i * 4] = 0;
                    buf[i * 4 + 1] = 0;
                    buf[i * 4 + 2] = 0x18;
                    buf[i * 4 + 3] = 220;
                }
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    4,
                    &lines[0],
                    [0xFF, 0xD7, 0x00, 0xFF],
                    scale,
                );
                for (i, line) in lines.iter().enumerate().skip(1) {
                    tiny_font::draw_str(
                        &mut buf,
                        panel_w,
                        panel_h,
                        4,
                        4 + i as i32 * line_h,
                        line,
                        [0xCC, 0xCC, 0xCC, 0xFF],
                        scale,
                    );
                }
                if let Ok(mut tex) = texture_creator.create_texture_streaming(
                    PixelFormatEnum::RGBA32,
                    panel_w,
                    panel_h,
                ) {
                    tex.update(None, &buf, (panel_w * 4) as usize).ok();
                    tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                    canvas
                        .copy(&tex, None, Some(Rect::new(8, 60, panel_w, panel_h)))
                        .ok();
                }
            }
        }

        // Cities list (C) — manual Appendix D. Uses STADT4 city
        // records; other-player cities are only shown if a trade
        // agreement exists.
        if cities_panel {
            let scale = 1u32;
            let line_h = (5 * scale + 3) as i32;
            let panel_w = 430u32;
            let visible = visible_city_list_rows(&islands, &sim.diplomacy);
            let n = visible.len() as i32;
            let panel_h = (28 + (n + 1).max(2) * line_h + 8) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4,
                "CITIES (C/Esc close)",
                [0xFF, 0xD7, 0x00, 0xFF],
                scale,
            );
            tiny_font::draw_str(
                &mut buf,
                panel_w,
                panel_h,
                4,
                4 + line_h,
                "city                 owner   pop  island",
                [0xCC, 0xCC, 0xCC, 0xFF],
                scale,
            );
            for (row, city) in visible.iter().enumerate() {
                let line = city_list_line(city);
                let color = if city.owner == 0 {
                    [0x80, 0xFF, 0xC0, 0xFF]
                } else {
                    [0xCC, 0xCC, 0xCC, 0xFF]
                };
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    4 + line_h * (2 + row as i32),
                    &line,
                    color,
                    scale,
                );
            }
            if visible.is_empty() {
                tiny_font::draw_str(
                    &mut buf,
                    panel_w,
                    panel_h,
                    4,
                    4 + line_h * 2,
                    "(no known cities)",
                    [0x88, 0x88, 0x88, 0xFF],
                    scale,
                );
            }
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, 60, panel_w, panel_h)))
                    .ok();
            }
        }

        // Chat overlay (bottom-left). Each entry sticks for 10s, plus a
        // live input box while typing.
        {
            let chat_ttl = std::time::Duration::from_secs(10);
            while chat_log
                .front()
                .map(|(_, t)| t.elapsed() > chat_ttl)
                .unwrap_or(false)
            {
                chat_log.pop_front();
            }
            if !chat_log.is_empty() || chat_active {
                let scale = 1u32;
                let line_h = 8i32;
                let panel_w = 360u32;
                let n_lines = chat_log.len() as i32 + if chat_active { 1 } else { 0 };
                let panel_h = (n_lines.max(1) * line_h + 6) as u32;
                let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
                for i in 0..(panel_w * panel_h) as usize {
                    buf[i * 4] = 0;
                    buf[i * 4 + 1] = 0;
                    buf[i * 4 + 2] = 0x10;
                    buf[i * 4 + 3] = 180;
                }
                let mut y = 2;
                for (line, _) in chat_log.iter() {
                    let color = if line.starts_with("you:") {
                        [0xCC, 0xFF, 0xCC, 0xFF]
                    } else {
                        [0xFF, 0xFF, 0xFF, 0xFF]
                    };
                    tiny_font::draw_str(&mut buf, panel_w, panel_h, 4, y, line, color, scale);
                    y += line_h;
                }
                if chat_active {
                    let prompt = format!("> {}_", chat_input);
                    tiny_font::draw_str(
                        &mut buf,
                        panel_w,
                        panel_h,
                        4,
                        y,
                        &prompt,
                        [0xFF, 0xD7, 0x00, 0xFF],
                        scale,
                    );
                }
                if let Ok(mut tex) = texture_creator.create_texture_streaming(
                    PixelFormatEnum::RGBA32,
                    panel_w,
                    panel_h,
                ) {
                    tex.update(None, &buf, (panel_w * 4) as usize).ok();
                    tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                    let tx = 8i32;
                    let ty = WINDOW_H as i32 - panel_h as i32 - 8;
                    canvas
                        .copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h)))
                        .ok();
                }
            }
        }

        // Title bar with simulation status
        let (minutes, seconds) = sim.display_time();
        let speed_label = if sim.paused {
            "PAUSED".to_string()
        } else {
            format!("{}x", sim.speed_multiplier)
        };
        let carriers = sim.figures.iter().filter(|f| f.is_active()).count();
        let human_gold = sim.players.first().map(|p| p.gold).unwrap_or(0);
        let zoom_label = ["GFX", "MGFX", "SGFX"][sprite_zoom];

        let title = if options_panel {
            format!(
                "OPTIONS - music:{} volume:{:.0}% video:{} speech:{} - Up/Down=select Left/Right=change O/Esc=close",
                if music_enabled { "ON" } else { "OFF" },
                music_volume * 100.0,
                if video_sequences_enabled { "ON" } else { "OFF" },
                if speech_enabled { "ON" } else { "OFF" },
            )
        } else if video_speech_panel {
            format!(
                "VIDEO/SPEECH - video:{} speech:{} - Up/Down=select Left/Right=toggle F/Esc=close",
                if video_sequences_enabled { "ON" } else { "OFF" },
                if speech_enabled { "ON" } else { "OFF" },
            )
        } else if diplomacy_panel {
            use anno_sim::combat::Diplomacy;
            let cur = match sim.diplomacy.get(0, diplomacy_target) {
                Diplomacy::Allied => "ALLIED",
                Diplomacy::Neutral => "NEUTRAL",
                Diplomacy::War => "WAR",
            };
            format!(
                "DIPLOMACY — vs Player {} = {} — Up/Down=select Left/Right=cycle D/Esc=close",
                diplomacy_target, cur,
            )
        } else if demolish_mode {
            let hover_info = if let Some(bi) = demolish_hover {
                let b = &sim.buildings[bi];
                let def = &defs[b.def_id as usize];
                let name = cod.buildings[b.def_id as usize]
                    .properties
                    .get("Name")
                    .cloned()
                    .unwrap_or_else(|| format!("Bldg#{}", b.def_id));
                let refund = def.cost_gold / 2;
                format!("{} — refund: {} gold — click to demolish", name, refund)
            } else {
                "hover over a building to demolish".to_string()
            };
            format!("DEMOLISH MODE — {} — Esc=cancel", hover_info)
        } else if info_mode {
            "INFO MODE — click building or warehouse for status — I/Esc=close".to_string()
        } else if let Some(ref insp) = inspection {
            format!("INSPECT — {} — Esc=close", insp.info,)
        } else if placer.active {
            let page_indices = placer.page_index_slice();
            let build_list: String = page_indices
                .iter()
                .enumerate()
                .map(|(i, &b_idx)| {
                    let marker = if b_idx == placer.selected { ">" } else { " " };
                    let name = &placer.buildable[b_idx].name;
                    format!("{marker}{}:{}", i + 1, name)
                })
                .collect::<Vec<_>>()
                .join(" ");
            let sel_cost = placer
                .selected_building()
                .map(|b| defs[b.def_idx].cost_gold)
                .unwrap_or(0);
            let cat_label = placer.category.label();
            let pg_total = placer.page_count().max(1);
            let rot_count = placer
                .selected_building()
                .map(|b| cod.buildings[b.def_idx].rotate.max(1))
                .unwrap_or(1);
            let rot_label = if rot_count > 1 {
                format!(" rot:{}/{}", placer.orientation + 1, rot_count)
            } else {
                String::new()
            };
            format!(
                "BUILD MODE [{cat_label}] — gold:{} cost:{}{} — pg{}/{} — {} — [/]=cat PgUp/Dn=page Z/X=rot Esc=cancel",
                human_gold,
                sel_cost,
                rot_label,
                placer.page + 1,
                pg_total,
                build_list,
            )
        } else if combat_mode {
            if let Some(si) = selected_trade_ship_idx {
                format!("COMBAT MODE — selected trade ship #{si} — W=white flag K/Esc=close")
            } else if selected_units.is_empty() {
                "COMBAT MODE — click own ships/units to select — K/Esc=close".to_string()
            } else {
                format!(
                    "COMBAT MODE — selected {} unit(s) — RMB=move Ctrl+1-9=store 1-9=recall W=white flag K/Esc=close",
                    selected_units.len(),
                )
            }
        } else if !selected_units.is_empty() {
            format!(
                "Anno 1602 — selected {} unit(s) — RMB=move-here Esc=deselect — {:02}:{:02} {} — gold:{}",
                selected_units.len(),
                minutes,
                seconds,
                speed_label,
                human_gold,
            )
        } else {
            format!(
                "Anno 1602 [{}] — '{}' — {:02}:{:02} {} — carriers:{} ships:{} units:{} routes:{} gold:{} — {zoom_label} {}x — B=build I=info K=combat D=diplo L=save S=ships C=cities",
                net_status,
                scenario_name,
                minutes,
                seconds,
                speed_label,
                carriers,
                sim.trade_ships.iter().filter(|s| s.active).count(),
                sim.military_units.iter().filter(|u| u.is_alive()).count(),
                sim.trade_routes.iter().filter(|r| r.active).count(),
                human_gold,
                display_zoom,
            )
        };
        // Override with save/load banner for ~3s
        let title = if let Some((ref msg, ref t)) = save_banner {
            if t.elapsed() < std::time::Duration::from_millis(3000) {
                format!("[{msg}] {title}")
            } else {
                save_banner = None;
                title
            }
        } else {
            title
        };
        // Outcome takes precedence — sticks once decided.
        let title = match sim.outcome {
            anno_sim::simulation::GameOutcome::Victory => format!("[VICTORY] {title}"),
            anno_sim::simulation::GameOutcome::Defeat => format!("[DEFEAT] {title}"),
            _ => title,
        };
        canvas.window_mut().set_title(&title).ok();

        // Outcome banner overlay — drawn centred so the player can see
        // it in-window, not just in the title bar.
        if sim.outcome != anno_sim::simulation::GameOutcome::Pending {
            let (label, color) = match sim.outcome {
                anno_sim::simulation::GameOutcome::Victory => ("VICTORY", [0x40, 0xFF, 0x80, 0xFF]),
                anno_sim::simulation::GameOutcome::Defeat => ("DEFEAT", [0xFF, 0x40, 0x40, 0xFF]),
                _ => ("", [0xFF, 0xFF, 0xFF, 0xFF]),
            };
            let scale = 4u32;
            let panel_w = 240u32;
            let panel_h = (5 * scale + 16) as u32;
            let mut buf = vec![0u8; (panel_w * panel_h * 4) as usize];
            for i in 0..(panel_w * panel_h) as usize {
                buf[i * 4] = 0;
                buf[i * 4 + 1] = 0;
                buf[i * 4 + 2] = 0x18;
                buf[i * 4 + 3] = 220;
            }
            let text_x = (panel_w as i32 - (label.len() as i32) * 4 * scale as i32) / 2;
            tiny_font::draw_str(&mut buf, panel_w, panel_h, text_x, 8, label, color, scale);
            if let Ok(mut tex) =
                texture_creator.create_texture_streaming(PixelFormatEnum::RGBA32, panel_w, panel_h)
            {
                tex.update(None, &buf, (panel_w * 4) as usize).ok();
                tex.set_blend_mode(sdl2::render::BlendMode::Blend);
                let tx = (WINDOW_W as i32 - panel_w as i32) / 2;
                let ty = 80i32;
                canvas
                    .copy(&tex, None, Some(Rect::new(tx, ty, panel_w, panel_h)))
                    .ok();
            }
        }

        canvas.present();
    }

    if let (Some(rec), Some(path)) = (recorder.take(), record_path.as_ref()) {
        let recording = rec.finish();
        match anno_sim::replay::save_recording(path, &recording) {
            Ok(()) => println!(
                "Recorded {} command(s) to {}",
                recording.entries.len(),
                path.display()
            ),
            Err(error) => eprintln!("Failed to write recording: {error}"),
        }
    }
}

/// Cached terrain render with coordinate info for overlay.
struct RenderState {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    tile_w: i32,
    tile_h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShipSpriteLayout {
    small_trader_base: usize,
    large_trader_base: usize,
    free_trader_base: usize,
    small_warship_base: usize,
    large_warship_base: usize,
    pirate_ship_base: usize,
}

impl ShipSpriteLayout {
    fn from_figure_defs(
        handel1: Option<&anno_formats::figuren::FigureDef>,
        handel2: Option<&anno_formats::figuren::FigureDef>,
        handler: Option<&anno_formats::figuren::FigureDef>,
        krieg1: Option<&anno_formats::figuren::FigureDef>,
        krieg2: Option<&anno_formats::figuren::FigureDef>,
        pirat: Option<&anno_formats::figuren::FigureDef>,
    ) -> Self {
        Self {
            small_trader_base: figure_walk_sprite_base(handel1, 0),
            large_trader_base: figure_walk_sprite_base(handel2, 32),
            free_trader_base: figure_walk_sprite_base(handler, 16),
            small_warship_base: figure_walk_sprite_base(krieg1, 64),
            large_warship_base: figure_walk_sprite_base(krieg2, 48),
            pirate_ship_base: figure_walk_sprite_base(pirat, 80),
        }
    }

    fn trader_base(self, class: TradeShipClass) -> usize {
        match class {
            TradeShipClass::SmallTrader => self.small_trader_base,
            TradeShipClass::LargeTrader => self.large_trader_base,
        }
    }

    fn naval_base(self, unit_type: UnitType) -> Option<usize> {
        match unit_type {
            UnitType::SmallWarship => Some(self.small_warship_base),
            UnitType::LargeWarship => Some(self.large_warship_base),
            UnitType::PirateShip => Some(self.pirate_ship_base),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SoldierSpriteFamily {
    bases: [usize; 4],
    frames_per_dir: usize,
    frame_speed_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CarrierShadowLayout {
    base: usize,
    frames_per_dir: usize,
    directional: bool,
}

impl CarrierShadowLayout {
    const fn normal_default() -> Self {
        Self {
            base: 0,
            frames_per_dir: 8,
            directional: false,
        }
    }

    const fn long_default() -> Self {
        Self {
            base: 8,
            frames_per_dir: 8,
            directional: true,
        }
    }

    fn sprite_index(self, direction: u8, frame: usize) -> usize {
        let frames = self.frames_per_dir.max(1);
        self.base
            + if self.directional {
                usize::from(direction % 8) * frames
            } else {
                0
            }
            + frame % frames
    }
}

fn carrier_shadow_layout_from_figure(
    figure: Option<&anno_formats::figuren::FigureDef>,
    fallback: CarrierShadowLayout,
) -> CarrierShadowLayout {
    let Some(figure) = figure else {
        return fallback;
    };
    let Some(walk) = figure.walk_anim() else {
        return fallback;
    };
    let Some(base) = figure
        .gfx
        .checked_add(walk.anim_offs)
        .and_then(|base| usize::try_from(base).ok())
    else {
        return fallback;
    };
    let frames_per_dir = usize::try_from(walk.anim_anz)
        .ok()
        .filter(|&frames| frames > 0)
        .unwrap_or(fallback.frames_per_dir);
    CarrierShadowLayout {
        base,
        frames_per_dir,
        directional: figure.rotate > 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SoldierSpriteLayout {
    infantry: SoldierSpriteFamily,
    cavalry: SoldierSpriteFamily,
    cannon: SoldierSpriteFamily,
    musketeer: SoldierSpriteFamily,
    native_spearman: SoldierSpriteFamily,
}

impl SoldierSpriteLayout {
    fn from_figures(figures: &anno_formats::figuren::FiguresFile) -> Self {
        Self {
            infantry: soldier_family_from_figures(
                figures,
                ["SOLDAT1", "SOLDAT2", "SOLDAT3", "SOLDAT4"],
                [0, 280, 560, 840],
            ),
            cavalry: soldier_family_from_figures(
                figures,
                ["KAVALERIE1", "KAVALERIE2", "KAVALERIE3", "KAVALERIE4"],
                [1120, 1424, 1728, 2032],
            ),
            cannon: soldier_family_from_figures(
                figures,
                ["KANONIER1", "KANONIER2", "KANONIER3", "KANONIER4"],
                [2336, 2552, 2768, 2984],
            ),
            musketeer: soldier_family_from_figures(
                figures,
                ["MUSKETIER1", "MUSKETIER2", "MUSKETIER3", "MUSKETIER4"],
                [3200, 3336, 3472, 3608],
            ),
            native_spearman: soldier_family_from_figures(
                figures,
                ["SPEER1", "SPEER1", "SPEER1", "SPEER1"],
                [3744, 3744, 3744, 3744],
            ),
        }
    }

    fn family(self, unit_type: UnitType) -> Option<SoldierSpriteFamily> {
        match unit_type {
            UnitType::Infantry => Some(self.infantry),
            UnitType::Cavalry => Some(self.cavalry),
            UnitType::Cannon => Some(self.cannon),
            UnitType::Musketeer => Some(self.musketeer),
            UnitType::NativeSpearman => Some(self.native_spearman),
            _ => None,
        }
    }

    fn sprite_index(
        self,
        unit_type: UnitType,
        owner: u8,
        direction: u8,
        elapsed_ms: u32,
        moving: bool,
    ) -> Option<usize> {
        let family = self.family(unit_type)?;
        let variant = owner_sprite_variant(owner);
        let frame = if moving {
            ((elapsed_ms / family.frame_speed_ms.max(1)) as usize) % family.frames_per_dir.max(1)
        } else {
            0
        };
        Some(rotated_walk_sprite_index(
            family.bases[variant],
            family.frames_per_dir,
            direction,
            frame,
        ))
    }
}

fn soldier_family_from_figures(
    figures: &anno_formats::figuren::FiguresFile,
    names: [&str; 4],
    fallback_bases: [usize; 4],
) -> SoldierSpriteFamily {
    let mut bases = fallback_bases;
    let mut frames_per_dir = 8usize;
    let mut frame_speed_ms = 100u32;
    for (idx, name) in names.iter().enumerate() {
        if let Some(def) = figures.find(name) {
            bases[idx] = figure_walk_sprite_base(Some(def), fallback_bases[idx]);
            if let Some(walk) = def.walk_anim() {
                if let Some(frames) = usize::try_from(walk.anim_anz)
                    .ok()
                    .filter(|&frames| frames > 0)
                {
                    frames_per_dir = frames;
                }
                if let Some(speed) = u32::try_from(walk.anim_speed)
                    .ok()
                    .filter(|&speed| speed > 0)
                {
                    frame_speed_ms = speed;
                }
            }
        }
    }
    SoldierSpriteFamily {
        bases,
        frames_per_dir,
        frame_speed_ms,
    }
}

fn owner_sprite_variant(owner: u8) -> usize {
    usize::from(owner.min(3))
}

fn figure_walk_sprite_base(
    figure: Option<&anno_formats::figuren::FigureDef>,
    fallback: usize,
) -> usize {
    figure
        .and_then(|f| {
            let anim_offs = f.walk_anim().map(|a| a.anim_offs).unwrap_or(0);
            usize::try_from(f.gfx + anim_offs).ok()
        })
        .unwrap_or(fallback)
}

fn live_ship_sprite_index(base: usize, heading: u8) -> usize {
    base + usize::from(heading % 8)
}

fn entity_walk_sprite_index(
    base_sprite: u16,
    anim_offs: usize,
    frames_per_dir: usize,
    direction: u8,
    anim_frame: usize,
) -> usize {
    let frames = frames_per_dir.max(1);
    base_sprite as usize + anim_offs + usize::from(direction % 8) * frames + anim_frame % frames
}

fn source_walk_frame(source_frame: u8, frames_per_dir: usize) -> usize {
    let frames = frames_per_dir.max(1);
    usize::from(source_frame) % frames
}

fn source_shadow_y_offset(tile_width: i32, position_offset_y: i32) -> i32 {
    tile_width.saturating_mul(position_offset_y) / 64
}

fn source_terrain_z_lift(terrain_height: f32, tile_height: i32) -> i32 {
    (terrain_height * tile_height as f32).round() as i32
}

fn rotated_walk_sprite_index(
    base: usize,
    frames_per_dir: usize,
    direction: u8,
    frame: usize,
) -> usize {
    let frames = frames_per_dir.max(1);
    base + usize::from(direction % 8) * frames + frame % frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use anno_formats::figuren::{FigureAnim, FigureDef};

    fn test_building_def(
        required_fertility: Option<anno_formats::szs::Fertility>,
    ) -> anno_sim::building::BuildingDef {
        anno_sim::building::BuildingDef {
            id: 0,
            category: 0,
            width: 1,
            height: 1,
            production_type: anno_sim::types::ProductionType::Plantation,
            kind: "GEBAEUDE".into(),
            prod_kind: "PLANTAGE".into(),
            radius: 0,
            output_good: Good::Tobacco,
            input_good_1: Good::None,
            input_good_2: Good::None,
            output_rate: 0,
            input_1_rate: 0,
            input_2_rate: 0,
            storage_capacity: 0,
            cycle_time_ms: 0,
            cost_gold: 0,
            cost_tools: 0,
            cost_wood: 0,
            cost_bricks: 0,
            maintenance_cost: 0,
            native: false,
            bauinfra: 0,
            max_no_input_ticks: 6,
            can_dry_up: false,
            wegspeed: [100; 4],
            has_door: false,
            upgradeable: false,
            max_energy: 0,
            ore_deposit: anno_sim::building::OreDeposit::None,
            pirate_owned: false,
            defensive_cannons: 0,
            max_brand_damage_ticks: anno_sim::building::DEFAULT_MAX_BRAND_DAMAGE_TICKS,
            ruin_id: anno_sim::building::NO_RUIN_ID,
            required_fertility,
        }
    }

    fn test_processing_def() -> anno_sim::building::BuildingDef {
        let mut def = test_building_def(None);
        def.prod_kind = "HANDWERK".into();
        def.output_good = Good::Tools;
        def.input_good_1 = Good::Iron;
        def.input_good_2 = Good::Wood;
        def.input_1_rate = 1;
        def.input_2_rate = 1;
        def.storage_capacity = 50;
        def
    }

    fn test_cod_processing_building(gfx: i32) -> anno_formats::cod::BuildingDef {
        let mut building = anno_formats::cod::BuildingDef::default();
        building.nummer = 0;
        building.gfx = gfx;
        building.kind = "GEBAEUDE".into();
        building.rotate = 1;
        building.anim_anz = 1;
        building.anim_add = 1;
        building
            .properties
            .insert("ProdKind".into(), "HANDWERK".into());
        building
            .properties
            .insert("Name".into(), "Processor".into());
        building
    }

    fn test_hq_def() -> anno_sim::building::BuildingDef {
        let mut def = test_building_def(None);
        def.kind = "HQ".into();
        def.prod_kind = "KONTOR".into();
        def.output_good = Good::None;
        def
    }

    fn test_cod_hq_building(gfx: i32) -> anno_formats::cod::BuildingDef {
        let mut building = test_cod_processing_building(gfx);
        building.kind = "HQ".into();
        building
            .properties
            .insert("ProdKind".into(), "KONTOR".into());
        building.properties.insert("Name".into(), "Kontor".into());
        building
    }

    #[test]
    fn source_command_resolves_inselhaus_definition_ids_before_rendering() {
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
                gfx: 123,
                ..Default::default()
            }],
        };
        let source_tile = IslandTile {
            building_id: 3,
            x: 0,
            y: 0,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        };

        assert_eq!(
            source_command_gfx_tiles(
                0,
                0,
                0,
                anno_sim::building::SourceBuildingCommand::from_island_tile(source_tile),
                &cod,
            ),
            vec![(0, 0, 0, 123)]
        );
        assert_eq!(
            source_command_gfx_tiles(
                0,
                0,
                0,
                anno_sim::building::SourceBuildingCommand::from_island_tile(IslandTile {
                    building_id: 7,
                    ..source_tile
                }),
                &cod,
            ),
            vec![(0, 0, 0, 7)]
        );
    }

    #[test]
    fn source_command_applies_authored_variant_before_orientation() {
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
                gfx: 100,
                anim_anz: 2,
                anim_add: 3,
                ..Default::default()
            }],
        };
        let source_tile = IslandTile {
            building_id: 3,
            orientation: 0b0001_0110,
            x: 0,
            y: 0,
            anim_count: 0,
            flags: 0,
        };

        assert_eq!(
            source_command_gfx_tiles(
                0,
                0,
                0,
                anno_sim::building::SourceBuildingCommand::from_island_tile(source_tile),
                &cod,
            ),
            vec![(0, 0, 0, 115)]
        );
    }

    #[test]
    fn source_command_adds_definition_anim_frame_to_the_packed_variant() {
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
                gfx: 100,
                anim_anz: 4,
                anim_add: 3,
                anim_frame: 2,
                ..Default::default()
            }],
        };
        let command = anno_sim::building::SourceBuildingCommand {
            definition_offset: 3,
            orientation: 1,
            variant: 3,
            metadata: 0,
            map_owner_slot: 7,
            random_seed: 0,
            dynamic_object_owner: 0,
        };

        assert_eq!(
            source_command_gfx_tiles(0, 0, 0, command, &cod),
            vec![(0, 0, 0, 115)]
        );
    }

    #[test]
    fn source_command_initial_selector_matches_kind_specific_draw_branches() {
        let command = anno_sim::building::SourceBuildingCommand {
            definition_offset: 3,
            orientation: 1,
            variant: 3,
            metadata: 0,
            map_owner_slot: 7,
            random_seed: 0,
            dynamic_object_owner: 0,
        };
        // `(outer HAUS Kind, nested HAUS_PRODTYP Kind)` pairs taken from
        // haeuser.cod. Only the nested kind selects a draw branch
        // (`1602_exe.c:98270-98300`); the outer label is along for the ride.
        let building = |kind: &str, prod_kind: &str, anim_time| anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
            gfx: 100,
            kind: kind.into(),
            properties: [("ProdKind".into(), prod_kind.into())].into(),
            anim_anz: 4,
            anim_add: 3,
            anim_frame: 2,
            anim_time,
            ..Default::default()
        };

        let render = |definition| {
            source_command_gfx_tiles(
                0,
                0,
                0,
                command,
                &CodFile {
                    constants: Default::default(),
                    buildings: vec![definition],
                },
            )
        };
        // Production kinds 1 through 8 start their cleared record at frame 0.
        assert_eq!(
            render(building("GEBAEUDE", "HANDWERK", 0)),
            vec![(0, 0, 0, 112)]
        );
        assert_eq!(
            render(building("GEBAEUDE", "MARKT", 0)),
            vec![(0, 0, 0, 112)]
        );
        assert_eq!(render(building("HQ", "KONTOR", 0)), vec![(0, 0, 0, 112)]);
        // Production kind 10 takes the packed-variant branch while `anim_time`
        // is zero and the ordinary `variant + AnimFrame` branch otherwise.
        assert_eq!(
            render(building("BODEN", "ROHSTWACHS", 0)),
            vec![(0, 0, 0, 121)]
        );
        assert_eq!(
            render(building("BODEN", "ROHSTWACHS", 100)),
            vec![(0, 0, 0, 115)]
        );
        // Everything else, houses included, uses `variant + AnimFrame`.
        assert_eq!(
            render(building("GEBAEUDE", "WOHNUNG", 0)),
            vec![(0, 0, 0, 115)]
        );
    }

    #[test]
    fn source_command_uses_live_source_cell_frame_selectors() {
        let command = anno_sim::building::SourceBuildingCommand {
            definition_offset: 3,
            orientation: 0,
            variant: 0,
            metadata: 0,
            map_owner_slot: 7,
            random_seed: 0,
            dynamic_object_owner: 0,
        };
        // Shaped like haeuser.cod: every producer carries outer
        // `Kind: GEBAEUDE` and puts its production label in the nested
        // `HAUS_PRODTYP Kind`, which `FUN_00481450` reads at definition
        // offset `+0x1c` to decide whether a live cell record exists.
        let definition = |prod_kind: &str| anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
            gfx: 100,
            kind: "GEBAEUDE".into(),
            properties: [("ProdKind".into(), prod_kind.into())].into(),
            anim_anz: 3,
            anim_add: 4,
            ..Default::default()
        };

        let craft = definition("HANDWERK");
        let mut craft_state = anno_sim::source_cell::SourceMapCellState::new(0, 0, 0, &craft, 0)
            .expect("kind one source state");
        craft_state.frame_selector = 2;
        let craft_cod = CodFile {
            constants: Default::default(),
            buildings: vec![craft],
        };
        assert_eq!(
            source_command_gfx_tiles_with_state(0, 0, 0, command, Some(&craft_state), &craft_cod),
            vec![(0, 0, 0, 108)]
        );

        let mut storage_craft = definition("HANDWERK");
        storage_craft.storage_animation = true;
        storage_craft.storage_animation_capacity = 160;
        let storage_state = anno_sim::source_cell::SourceMapCellState {
            storage_fill: 80,
            ..anno_sim::source_cell::SourceMapCellState::new(0, 0, 0, &storage_craft, 0)
                .expect("kind one source state")
        };
        let storage_cod = CodFile {
            constants: Default::default(),
            buildings: vec![storage_craft],
        };
        assert_eq!(
            source_command_gfx_tiles_with_state(
                0,
                0,
                0,
                command,
                Some(&storage_state),
                &storage_cod,
            ),
            vec![(0, 0, 0, 104)]
        );

        let market = definition("MARKT");
        let mut market_state = anno_sim::source_cell::SourceMapCellState::new(0, 0, 0, &market, 0)
            .expect("kind seven source state");
        market_state.progress = 512;
        let market_cod = CodFile {
            constants: Default::default(),
            buildings: vec![market],
        };
        assert_eq!(
            source_command_gfx_tiles_with_state(0, 0, 0, command, Some(&market_state), &market_cod),
            vec![(0, 0, 0, 108)]
        );
    }

    #[test]
    fn runtime_building_tiles_apply_source_variant_before_rotation() {
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 7,
                gfx: 100,
                size: (2, 1),
                rotate: 4,
                anim_anz: 2,
                anim_add: 3,
                ..Default::default()
            }],
        };
        let mut building = BuildingInstance::new(0, 5, 12, 13, 0);
        building.source_placement_command = Some(anno_sim::building::SourceBuildingCommand {
            definition_offset: 7,
            orientation: 2,
            variant: 5,
            metadata: 5,
            map_owner_slot: 7,
            random_seed: 0,
            dynamic_object_owner: 0,
        });

        assert_eq!(
            runtime_building_gfx_tiles(&[building], &cod, &[]),
            vec![(5, 13, 13, 115), (5, 12, 13, 116)]
        );
    }

    #[test]
    fn source_command_uses_fun_00463b10_cell_order_for_all_orientations() {
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 3,
                gfx: 100,
                size: (2, 3),
                ..Default::default()
            }],
        };
        let command = anno_sim::building::SourceBuildingCommand {
            definition_offset: 3,
            orientation: 0,
            variant: 0,
            metadata: 0,
            map_owner_slot: 7,
            random_seed: 0,
            dynamic_object_owner: 0,
        };

        let tiles = |orientation| {
            source_command_gfx_tiles(
                0,
                10,
                20,
                anno_sim::building::SourceBuildingCommand {
                    orientation,
                    ..command
                },
                &cod,
            )
        };
        assert_eq!(
            tiles(0),
            vec![
                (0, 10, 20, 100),
                (0, 11, 20, 101),
                (0, 10, 21, 102),
                (0, 11, 21, 103),
                (0, 10, 22, 104),
                (0, 11, 22, 105),
            ]
        );
        assert_eq!(
            tiles(1),
            vec![
                (0, 12, 20, 100),
                (0, 12, 21, 101),
                (0, 11, 20, 102),
                (0, 11, 21, 103),
                (0, 10, 20, 104),
                (0, 10, 21, 105),
            ]
        );
        assert_eq!(
            tiles(2),
            vec![
                (0, 11, 22, 100),
                (0, 10, 22, 101),
                (0, 11, 21, 102),
                (0, 10, 21, 103),
                (0, 11, 20, 104),
                (0, 10, 20, 105),
            ]
        );
        assert_eq!(
            tiles(3),
            vec![
                (0, 10, 21, 100),
                (0, 10, 20, 101),
                (0, 11, 21, 102),
                (0, 11, 20, 103),
                (0, 12, 21, 104),
                (0, 12, 20, 105),
            ]
        );
    }

    #[test]
    fn authored_tiles_expand_source_footprints_with_later_command_overwrite() {
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                anno_formats::cod::BuildingDef {
                    source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 1,
                    gfx: 100,
                    size: (2, 1),
                    ..Default::default()
                },
                anno_formats::cod::BuildingDef {
                    source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 2,
                    gfx: 200,
                    ..Default::default()
                },
            ],
        };
        let island = Island {
            number: 3,
            width: 3,
            height: 1,
            x_pos: 0,
            y_pos: 0,
            fertilities: [7; 8],
            tiles: vec![
                IslandTile {
                    building_id: 1,
                    x: 0,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                },
                IslandTile {
                    building_id: 2,
                    x: 1,
                    y: 0,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                },
            ],
            city: None,
        };

        assert_eq!(
            authored_island_gfx_tiles(&island, &cod, &[]),
            vec![(0, 0, 100), (1, 0, 200)]
        );
    }

    fn flat_test_island(number: u8) -> Island {
        let mut tiles = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                tiles.push(IslandTile {
                    building_id: 9999,
                    x,
                    y,
                    orientation: 0,
                    anim_count: 0,
                    flags: 0,
                });
            }
        }
        Island {
            number,
            width: 4,
            height: 4,
            x_pos: 0,
            y_pos: 0,
            fertilities: [7; 8],
            tiles,
            city: None,
        }
    }

    fn city_test_island(
        number: u8,
        owner_slot: u8,
        name: &str,
        tier_population: [u32; 5],
    ) -> Island {
        Island {
            number,
            width: 4,
            height: 4,
            x_pos: 0,
            y_pos: 0,
            fertilities: [7; 8],
            tiles: Vec::new(),
            city: Some(anno_formats::szs::City {
                island_index: number,
                owner_slot,
                tier_population,
                name: name.into(),
            }),
        }
    }

    #[test]
    fn diplomacy_panel_help_omits_fixed_tribute_shortcut() {
        assert!(!DIPLOMACY_PANEL_HELP.contains('G'));
        assert!(!DIPLOMACY_PANEL_HELP
            .to_ascii_lowercase()
            .contains("tribute"));
    }

    #[test]
    fn city_list_uses_stadt4_names_and_trade_agreements() {
        let islands = vec![
            city_test_island(0, 0, "Larrach", [3, 5, 0, 0, 0]),
            city_test_island(1, 1, "Hidden", [11, 0, 0, 0, 0]),
            city_test_island(2, 2, "Partner", [0, 0, 17, 0, 0]),
            city_test_island(3, 0, "", [99, 0, 0, 0, 0]),
        ];
        let mut diplomacy = anno_sim::combat::DiplomacyMatrix::new();
        assert!(diplomacy.propose_trade_agreement(0, 2));

        let rows = visible_city_list_rows(&islands, &diplomacy);

        assert_eq!(
            rows,
            vec![
                CityListRow {
                    name: "Larrach".into(),
                    owner: 0,
                    island_number: 0,
                    population: 8,
                },
                CityListRow {
                    name: "Partner".into(),
                    owner: 2,
                    island_number: 2,
                    population: 17,
                },
            ]
        );
        assert_eq!(city_list_line(&rows[0]), "Larrach              p0     8 i0");
    }

    #[test]
    fn ship_list_uses_names_and_omits_route_debug_fields() {
        let mut trade_ship = anno_sim::trade::TradeShip::new_with_class(
            0,
            42,
            100,
            200,
            TradeShipClass::LargeTrader,
            60,
        )
        .with_name("Seehind".into());
        trade_ship.state = anno_sim::trade::ShipState::Sailing;
        trade_ship.cargo_total = 12;
        trade_ship.cargo.push((Good::Wood, 12));
        let other_ship = anno_sim::trade::TradeShip::new(1, 7, 10, 20).with_name("Hidden".into());
        let mut warship = anno_sim::combat::MilitaryUnit::with_name(
            UnitType::SmallWarship,
            0,
            5,
            6,
            "Defender".into(),
        );
        warship.cannons = 4;

        let rows = visible_ship_list_rows(&[trade_ship, other_ship], &[warship]);

        assert_eq!(
            rows,
            vec![
                ShipListRow {
                    name: "Seehind".into(),
                    kind: "large trader",
                    status: "sailing",
                    warship: false,
                },
                ShipListRow {
                    name: "Defender".into(),
                    kind: "small warship",
                    status: "ready",
                    warship: true,
                },
            ]
        );
        let line = ship_list_line(&rows[0]);
        assert!(line.starts_with("Seehind"));
        assert!(line.contains("large trader"));
        assert!(line.ends_with("sailing"));
        assert!(!line.contains("route"));
        assert!(!line.contains("cargo"));
        assert!(!line.contains("(100,200)"));
    }

    fn test_island(y_pos: u16, fertilities: [u8; 8]) -> Island {
        Island {
            number: 0,
            width: 10,
            height: 10,
            x_pos: 0,
            y_pos,
            fertilities,
            tiles: Vec::new(),
            city: None,
        }
    }

    /// Load the shipping haeuser.cod corpus, or `None` when `extracted/` is
    /// not present in this checkout (corpus tests silently skip, matching
    /// the data-corpus tests in anno-formats).
    fn load_test_cod() -> Option<CodFile> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("extracted/haeuser.cod");
        if !path.exists() {
            return None;
        }
        let data = std::fs::read(&path).expect("read haeuser.cod");
        Some(CodFile::parse(&data).expect("parse haeuser.cod"))
    }

    fn seeded_draw(seed: u32) -> u16 {
        let mut rng = anno_sim::source_rand::SourceRand::new(seed);
        rng.next()
    }

    #[test]
    fn tile_clear_event_places_land_ruin_from_ruinenr() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        let mut islands = vec![test_island(10, [7; 8])];
        islands[0].tiles.push(IslandTile {
            building_id: cod.constants["GFXBODEN"] as u16,
            x: 3,
            y: 4,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        });

        apply_tile_clear_event(
            &mut islands,
            &cod,
            TileClear {
                island_id: 0,
                tile_x: 3,
                tile_y: 4,
                width: 1,
                height: 1,
                source_orientation: 0,
                source_variant: 0,
                source_map_owner_slot: 0,
                ruin_id: 0,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![seeded_draw(3)],
            },
        );

        assert_eq!(islands[0].tiles.len(), 1);
        let expected = cod
            .ruin_variant_building(0, false, seeded_draw(3))
            .expect("ordinary land ruin variant");
        assert_eq!(islands[0].tiles[0].source_id(), expected.source_id);
        assert_eq!(
            authored_island_gfx_tiles(&islands[0], &cod, &[]),
            vec![(3, 4, expected.gfx as u16)]
        );
    }

    #[test]
    fn no_ruin_terminal_replays_backing_root_into_visible_island_tiles() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        // A no-ruin terminal event restores the backing ground root into the
        // visible INSELHAUS stream. A static root's INSELHAUS building_id is
        // its id-offset (`source_id - base`) — the key that resolves the
        // definition again — not the gfx sprite index that `new_static`
        // defaults `source_definition_offset` to. Real roots overwrite that
        // default via `set_source_command` (scenario load / placement), so
        // build the backing the same way; otherwise the reconstructed tile
        // carries a gfx that resolves to nothing and renders as non-walkable.
        let definition = cod
            .buildings
            .iter()
            .find(|b| {
                b.kind == "BODEN" && b.source_id > anno_formats::szs::INSELHAUS_SOURCE_ID_BASE
            })
            .expect("terrain BODEN definition");
        let definition_offset =
            (definition.source_id - anno_formats::szs::INSELHAUS_SOURCE_ID_BASE) as u16;
        let mut backing =
            anno_sim::source_cell::SourceMapCellState::new_static(0, 3, 4, definition, 0)
                .expect("static BODEN command");
        backing.set_source_command(anno_sim::building::SourceBuildingCommand {
            definition_offset,
            orientation: 3,
            variant: 0,
            metadata: 0,
            map_owner_slot: 5,
            random_seed: 0,
            dynamic_object_owner: 0,
        });
        let clear = TileClear {
            island_id: 0,
            tile_x: 3,
            tile_y: 4,
            width: 1,
            height: 1,
            source_orientation: 1,
            source_variant: 8,
            source_map_owner_slot: 5,
            ruin_id: anno_sim::building::NO_RUIN_ID,
            ruin_uses_strand_table: false,
            fallback_strand_cells: 0,
            source_ruin_draws: Vec::new(),
        };
        let mut islands = vec![test_island(10, [7; 8])];
        islands[0].tiles.push(
            anno_sim::building::SourceBuildingCommand {
                definition_offset: cod.constants["GFXHANDW"] as u16,
                orientation: 0,
                variant: 0,
                metadata: 0,
                map_owner_slot: 0,
                random_seed: 0,
                dynamic_object_owner: 0,
            }
            .to_island_tile(3, 4),
        );
        let mut sim = Simulation::new();
        sim.island_maps
            .push(IslandMap::from_island(&islands[0], &cod.buildings));
        assert!(!sim.island_maps[0].is_walkable(3, 4));

        apply_tile_clear_event(&mut islands, &cod, clear.clone());
        push_no_ruin_backing_tiles(&mut islands, &[backing], &clear);
        refresh_simulation_island_map(&mut sim, &islands, &cod, 0);

        assert_eq!(islands[0].tiles.len(), 1);
        assert!(sim.island_maps[0].is_walkable(3, 4));
        assert_eq!(
            anno_sim::building::SourceBuildingCommand::from_island_tile(islands[0].tiles[0]),
            anno_sim::building::SourceBuildingCommand {
                definition_offset,
                orientation: 3,
                variant: 0,
                metadata: 0,
                map_owner_slot: 5,
                random_seed: 0,
                dynamic_object_owner: 0,
            }
        );
    }

    #[test]
    fn tile_clear_event_uses_strand_ruin_table_shift() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        let strand_gfx = cod
            .buildings
            .iter()
            .find(|b| b.kind == "STRAND")
            .expect("STRAND building")
            .gfx as u16;
        let mut islands = vec![test_island(10, [7; 8])];
        islands[0].tiles.push(IslandTile {
            building_id: strand_gfx,
            x: 2,
            y: 2,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        });

        apply_tile_clear_event(
            &mut islands,
            &cod,
            TileClear {
                island_id: 0,
                tile_x: 2,
                tile_y: 2,
                width: 1,
                height: 1,
                source_orientation: 0,
                source_variant: 0,
                source_map_owner_slot: 0,
                ruin_id: 0,
                ruin_uses_strand_table: true,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![seeded_draw(3)],
            },
        );

        assert_eq!(islands[0].tiles.len(), 1);
        let expected = cod
            .ruin_variant_building(0, true, seeded_draw(3))
            .expect("strand land ruin variant");
        assert_eq!(islands[0].tiles[0].source_id(), expected.source_id);
        assert_eq!(
            authored_island_gfx_tiles(&islands[0], &cod, &[]),
            vec![(2, 2, expected.gfx as u16)]
        );
    }

    #[test]
    fn tile_clear_event_places_multitile_kontor_ruin_ladder() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        let mut islands = vec![test_island(10, [7; 8])];
        islands[0].tiles.push(IslandTile {
            building_id: (cod.constants["GFXKONTOR"] as u16),
            x: 1,
            y: 1,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        });

        apply_tile_clear_event(
            &mut islands,
            &cod,
            TileClear {
                island_id: 0,
                tile_x: 1,
                tile_y: 1,
                width: 2,
                height: 3,
                source_orientation: 0,
                source_variant: 0,
                source_map_owner_slot: 0,
                ruin_id: 8,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![seeded_draw(1)],
            },
        );

        assert_eq!(islands[0].tiles.len(), 1);
        let base = (cod.constants["GFXKONTOR"] + 144) as u16;
        let mut got = authored_island_gfx_tiles(&islands[0], &cod, &[]);
        got.sort_unstable();
        assert_eq!(
            got,
            vec![
                (1, 1, base),
                (1, 2, base + 2),
                (1, 3, base + 4),
                (2, 1, base + 1),
                (2, 2, base + 3),
                (2, 3, base + 5),
            ],
        );
    }

    #[test]
    fn tile_clear_event_uses_randanz_randadd_variant_definition() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        let mut islands = vec![test_island(10, [7; 8])];
        islands[0].tiles.push(IslandTile {
            building_id: cod.constants["GFXBODEN"] as u16,
            x: 4,
            y: 4,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        });

        apply_tile_clear_event(
            &mut islands,
            &cod,
            TileClear {
                island_id: 0,
                tile_x: 4,
                tile_y: 4,
                width: 1,
                height: 1,
                source_orientation: 0,
                source_variant: 0,
                source_map_owner_slot: 0,
                ruin_id: 4,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![seeded_draw(1)],
            },
        );

        assert_eq!(islands[0].tiles.len(), 1);
        let expected = cod
            .ruin_variant_building(4, false, seeded_draw(1))
            .expect("raw-material ruin variant");
        assert_eq!(islands[0].tiles[0].source_id(), expected.source_id);
        assert_eq!(
            authored_island_gfx_tiles(&islands[0], &cod, &[]),
            vec![(4, 4, expected.gfx as u16)]
        );
    }

    #[test]
    fn tile_clear_event_uses_per_cell_strand_table_in_fallback_order() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        let mut islands = vec![test_island(10, [7; 8])];
        let draws = vec![
            seeded_draw(1),
            seeded_draw(2),
            seeded_draw(3),
            seeded_draw(4),
        ];

        apply_tile_clear_event(
            &mut islands,
            &cod,
            TileClear {
                island_id: 0,
                tile_x: 1,
                tile_y: 1,
                width: 2,
                height: 2,
                source_orientation: 0,
                source_variant: 0,
                source_map_owner_slot: 0,
                ruin_id: 0,
                ruin_uses_strand_table: false,
                // The source visits (2,1), (1,1), (2,2), (1,2).
                fallback_strand_cells: 1,
                source_ruin_draws: draws.clone(),
            },
        );

        assert_eq!(islands[0].tiles.len(), 4);
        let first = islands[0]
            .tiles
            .iter()
            .find(|tile| (tile.x, tile.y) == (2, 1))
            .expect("first source-order fallback cell");
        let second = islands[0]
            .tiles
            .iter()
            .find(|tile| (tile.x, tile.y) == (1, 1))
            .expect("second source-order fallback cell");
        assert_eq!(
            first.source_id(),
            cod.ruin_variant_building(0, true, draws[0])
                .expect("strand fallback ruin")
                .source_id
        );
        assert_eq!(
            second.source_id(),
            cod.ruin_variant_building(0, false, draws[1])
                .expect("ordinary fallback ruin")
                .source_id
        );
    }

    #[test]
    fn tile_clear_event_preserves_orientation_for_kontor_ruin_command() {
        let Some(cod) = load_test_cod() else {
            return;
        };
        let mut islands = vec![test_island(10, [7; 8])];
        islands[0].tiles.push(IslandTile {
            building_id: cod.constants["GFXKONTOR"] as u16,
            x: 1,
            y: 1,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        });

        apply_tile_clear_event(
            &mut islands,
            &cod,
            TileClear {
                island_id: 0,
                tile_x: 1,
                tile_y: 1,
                width: 3,
                height: 2,
                source_orientation: 1,
                source_variant: 2,
                source_map_owner_slot: 5,
                ruin_id: 8,
                ruin_uses_strand_table: false,
                fallback_strand_cells: 0,
                source_ruin_draws: vec![seeded_draw(1)],
            },
        );

        assert_eq!(islands[0].tiles.len(), 1);
        let expected = cod
            .ruin_variant_building(8, false, seeded_draw(1))
            .expect("Kontor ruin variant");
        assert_eq!(islands[0].tiles[0].source_id(), expected.source_id);
        assert_eq!(islands[0].tiles[0].orientation & 3, 1);
        let command =
            anno_sim::building::SourceBuildingCommand::from_island_tile(islands[0].tiles[0]);
        assert_eq!(command.variant, 2);
        assert_eq!(command.map_owner_slot, 5);
        let mut got: Vec<_> = authored_island_gfx_tiles(&islands[0], &cod, &[])
            .into_iter()
            .map(|(x, y, _)| (x, y))
            .collect();
        got.sort_unstable();
        assert_eq!(got, vec![(1, 1), (1, 2), (2, 1), (2, 2), (3, 1), (3, 2)]);
    }

    #[test]
    fn island_fertility_reads_the_insel5_crop_mask_not_the_y_climate() {
        use anno_formats::szs::Fertility;

        // Fertility is authored per island in the `INSEL5[0x5C]` crop
        // bitmask (`FUN_0046b0a0`, `1602_exe.c:74701`). Which map half
        // an island sits in only chooses the `Nord\`/`Sued\` terrain
        // library it loads (`FUN_00469690`, `1602_exe.c:73731`) — it
        // never implies a crop.
        let south_barren = test_island(450, [7; 8]);
        let north_tobacco = test_island(10, [1, 7, 7, 7, 7, 7, 7, 7]);

        assert!(!south_barren.has_fertility(Fertility::Tobacco));
        assert!(north_tobacco.has_fertility(Fertility::Tobacco));
        assert_eq!(north_tobacco.crop_flags(), 1 << 1);
    }

    #[test]
    fn fertility_list_label_reports_scenario_fertilities() {
        let island = test_island(0, [1, 6, 7, 7, 7, 7, 7, 7]);

        assert_eq!(fertility_list_label(&island), "Tobacco, Cocoa");
        assert_eq!(fertility_list_label(&test_island(0, [7; 8])), "none");
    }

    #[test]
    fn live_ship_sprite_index_adds_wrapped_heading_to_base() {
        assert_eq!(live_ship_sprite_index(16, 0), 16);
        assert_eq!(live_ship_sprite_index(16, 7), 23);
        assert_eq!(live_ship_sprite_index(16, 8), 16);
    }

    #[test]
    fn entity_walk_sprite_index_uses_base_animation_direction_and_frame() {
        assert_eq!(entity_walk_sprite_index(0, 64, 8, 7, 9), 121);
        assert_eq!(
            entity_walk_sprite_index(anno_sim::civilian::sprite_base_for(2), 0, 8, 3, 10,),
            1426,
        );
    }

    #[test]
    fn source_walk_frame_uses_the_figure_selected_by_source_animation() {
        assert_eq!(source_walk_frame(0, 8), 0);
        assert_eq!(source_walk_frame(7, 8), 7);
        assert_eq!(source_walk_frame(10, 8), 2);
    }

    #[test]
    fn source_shadow_y_offset_scales_with_the_horizontal_tile_width() {
        assert_eq!(source_shadow_y_offset(64, 5), 5);
        assert_eq!(source_shadow_y_offset(32, 5), 2);
        assert_eq!(source_shadow_y_offset(16, 5), 1);
    }

    #[test]
    fn source_terrain_z_lift_matches_the_figure_projection_scale() {
        assert_eq!(source_terrain_z_lift(0.28, 32), 9);
        assert_eq!(source_terrain_z_lift(0.28, 16), 4);
        assert_eq!(source_terrain_z_lift(0.28, 8), 2);
    }

    #[test]
    fn carrier_shadow_layout_matches_source_normal_and_long_families() {
        let normal = FigureDef {
            gfx: 0,
            rotate: 0,
            anims: vec![FigureAnim {
                nummer: 0,
                anim_anz: 8,
                ..Default::default()
            }],
            ..Default::default()
        };
        let long = FigureDef {
            gfx: 8,
            rotate: 8,
            anims: vec![FigureAnim {
                nummer: 0,
                anim_anz: 8,
                ..Default::default()
            }],
            ..Default::default()
        };

        let normal_layout =
            carrier_shadow_layout_from_figure(Some(&normal), CarrierShadowLayout::normal_default());
        let long_layout =
            carrier_shadow_layout_from_figure(Some(&long), CarrierShadowLayout::long_default());
        assert_eq!(normal_layout.sprite_index(5, 3), 3);
        assert_eq!(long_layout.sprite_index(5, 3), 51);
    }

    #[test]
    fn shadow_mask_blit_uses_black_for_opaque_source_pixels() {
        let mut dst = vec![9; 8];
        let src = [20, 30, 40, 255, 50, 60, 70, 0];
        blit_rgba_mask_color(&mut dst, 2, 1, 0, 0, &src, 2, 1, [0, 0, 0, 255]);
        assert_eq!(&dst[..4], &[0, 0, 0, 255]);
        assert_eq!(&dst[4..], &[9, 9, 9, 9]);
    }

    #[test]
    fn init_simulation_uses_player4_relationships_without_forced_slot1_war_or_militia() {
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: vec![
                anno_formats::szs::PlayerSlotInit {
                    state_byte: 0x00,
                    ai_active: true,
                    relationships: [0, 0, 0, 0, 3, 3, 3],
                    ..Default::default()
                },
                anno_formats::szs::PlayerSlotInit {
                    state_byte: 0x0c,
                    ai_active: true,
                    relationships: [0, 0, 0, 0, 3, 3, 3],
                    ..Default::default()
                },
            ],
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert_eq!(sim.diplomacy.get(0, 1), Diplomacy::Neutral);
        assert!(sim.military_units.is_empty());
    }

    #[test]
    fn init_simulation_uses_player4_starting_gold_exactly() {
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: vec![
                anno_formats::szs::PlayerSlotInit {
                    state_byte: 0x00,
                    ai_active: true,
                    starting_gold: 0,
                    ..Default::default()
                },
                anno_formats::szs::PlayerSlotInit {
                    state_byte: 0x0c,
                    ai_active: true,
                    starting_gold: 1234,
                    ..Default::default()
                },
            ],
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert_eq!(sim.players[0].gold, 0);
        assert_eq!(sim.players[1].gold, 1234);
    }

    #[test]
    fn init_simulation_does_not_seed_processor_input_stock() {
        let mut island = flat_test_island(0);
        island.tiles.push(IslandTile {
            building_id: 0,
            x: 1,
            y: 1,
            orientation: 0,
            anim_count: 0,
            flags: 0,
        });
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![island],
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![anno_formats::cod::BuildingDef {
                source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE,
                ..test_cod_processing_building(0)
            }],
        };
        let defs = vec![test_processing_def()];

        let sim = init_simulation(
            &szs,
            &cod,
            &defs,
            anno_sim::trade::ShipCargoConfig::default(),
        );

        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(sim.buildings[0].input_1_stock, 0);
        assert_eq!(sim.buildings[0].input_2_stock, 0);
    }

    #[test]
    fn placement_does_not_seed_processor_input_stock() {
        let defs = vec![test_processing_def()];
        let ground = anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE,
            kind: "BODEN".into(),
            ..Default::default()
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![test_cod_processing_building(0), ground],
        };
        let mut islands = vec![flat_test_island(0)];
        for tile in &mut islands[0].tiles {
            tile.building_id = 0;
        }
        let island_map = IslandMap::from_island(&islands[0], &cod.buildings);
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.island_maps.push(island_map);
        let mut placer = BuildingPlacer::new(&cod, &defs);
        placer.active = true;

        let outcome =
            try_place_building(&mut sim, &mut islands, 0, &defs, &cod, &placer, 1, 1, &mut None);

        assert!(matches!(outcome, PlaceOutcome::Placed));
        assert_eq!(sim.buildings.len(), 1);
        assert_eq!(sim.buildings[0].input_1_stock, 0);
        assert_eq!(sim.buildings[0].input_2_stock, 0);
    }

    #[test]
    fn placement_allocates_a_source_dynamic_slot_for_hq() {
        let defs = vec![test_hq_def()];
        let ground = anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE,
            kind: "BODEN".into(),
            ..Default::default()
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![
                anno_formats::cod::BuildingDef {
                    source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 9,
                    ..test_cod_hq_building(0)
                },
                ground,
            ],
        };
        let mut islands = vec![flat_test_island(4)];
        for tile in &mut islands[0].tiles {
            tile.building_id = 0;
        }
        let island_map = IslandMap::from_island(&islands[0], &cod.buildings);
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.building_defs = defs.clone();
        sim.island_maps.push(island_map);
        // The fixture's ground tiles carry map-owner selector 0, which in the
        // executable can only mean "island settlement slot 0 exists and owns
        // this tile" — `island + 0xac + 0` is the record `FUN_0046aec0`
        // (`1602_exe.c:74607`) dereferences to read the settlement's player
        // byte. Placement joins that settlement, so the record has to be here
        // for the fixture to describe a reachable state; without it the
        // resolver correctly reports the unsettled selector 7.
        assert!(sim.source_cities.set_record(
            0,
            Some(data_bridge::SourceCityRecord {
                island_id: 4,
                source_owner: 0,
                owner_slot: 0,
                ..data_bridge::SourceCityRecord::default()
            })
        ));
        let mut placer = BuildingPlacer::new(&cod, &defs);
        placer.active = true;

        let outcome =
            try_place_building(&mut sim, &mut islands, 0, &defs, &cod, &placer, 1, 1, &mut None);

        assert!(matches!(outcome, PlaceOutcome::Placed));
        assert_eq!(sim.buildings[0].source_dynamic_object_slot, Some(0));
        assert_eq!(sim.source_static_map_roots.len(), 1);
        assert!(sim.source_static_map_roots[0].matches(4, 1, 1));
        assert_eq!(sim.source_static_map_roots[0].kind_code, 35);
        // The kontor-production HQ also enters the live selector-state
        // vector: `SourceMapCellState::new` admits production kinds 7|8|30
        // so city-transfer scheduling can find it (see the matching filter
        // in `source_map_cell_states_from_scenario`).
        assert_eq!(sim.source_map_cell_states.len(), 1);
        assert!(sim.source_map_cell_states[0].matches(4, 1, 1));
        assert!(sim.source_map_cell_states[0].is_type11_transfer_root());
        assert_eq!(
            sim.buildings[0].source_placement_command,
            Some(anno_sim::building::SourceBuildingCommand {
                definition_offset: 9,
                orientation: 0,
                variant: 0,
                metadata: 4,
                map_owner_slot: 0,
                random_seed: 9,
                dynamic_object_owner: 0,
            })
        );
        assert_eq!(
            sim.source_dynamic_map_object_table(4).object(0),
            Some(anno_sim::source_route::SourceDynamicMapObject {
                island: 4,
                slot: 0,
                owner: 0,
                local_position: (1, 1),
            })
        );
    }

    #[test]
    fn rotated_placement_expands_the_source_static_cell_footprint() {
        let mut def = test_building_def(None);
        def.width = 2;
        def.height = 3;
        let defs = vec![def];
        let ground = anno_formats::cod::BuildingDef {
            source_id: anno_formats::szs::INSELHAUS_SOURCE_ID_BASE,
            kind: "BODEN".into(),
            ..Default::default()
        };
        let mut source_building = test_cod_processing_building(0);
        source_building.source_id = anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + 1;
        source_building.size = (2, 3);
        source_building.rotate = 4;
        let cod = CodFile {
            constants: Default::default(),
            buildings: vec![source_building, ground],
        };
        let mut islands = vec![flat_test_island(4)];
        for tile in &mut islands[0].tiles {
            tile.building_id = 0;
        }
        let island_map = IslandMap::from_island(&islands[0], &cod.buildings);
        let mut sim = Simulation::new();
        sim.players.push(Player::new_human(0));
        sim.island_maps.push(island_map);
        let mut placer = BuildingPlacer::new(&cod, &defs);
        placer.active = true;
        placer.orientation = 1;

        let outcome =
            try_place_building(&mut sim, &mut islands, 0, &defs, &cod, &placer, 1, 1, &mut None);

        assert!(matches!(outcome, PlaceOutcome::Placed));
        assert_eq!(sim.source_static_map_roots.len(), 6);
        for y in 1..3 {
            for x in 1..4 {
                let cell = sim
                    .source_static_map_roots
                    .iter()
                    .find(|cell| cell.matches(4, x, y))
                    .expect("oriented source static cell");
                assert_eq!((cell.footprint_width, cell.footprint_height), (3, 2));
                assert_eq!(cell.source_orientation, 1);
            }
        }
    }

    #[test]
    fn init_simulation_does_not_generate_route_or_ship_without_ship4_trader() {
        let city = |island_index: u8, owner_slot: u8, name: &str| anno_formats::szs::City {
            island_index,
            owner_slot,
            tier_population: [0; 5],
            name: name.into(),
        };
        let island = |number: u8, x_pos: u16, y_pos: u16, city| Island {
            number,
            width: 8,
            height: 8,
            x_pos,
            y_pos,
            fertilities: [7; 8],
            tiles: Vec::new(),
            city: Some(city),
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![
                island(0, 10, 10, city(0, 0, "A")),
                island(1, 40, 40, city(1, 0, "B")),
            ],
            players: vec![anno_formats::szs::PlayerSlotInit {
                state_byte: 0x00,
                ai_active: true,
                ..Default::default()
            }],
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert_eq!(sim.warehouses.len(), 2);
        assert!(sim.trade_routes.is_empty());
        assert!(sim.trade_ships.is_empty());
        assert_eq!(sim.players[0].total_population, 0);
    }

    #[test]
    fn ship4_trader_does_not_generate_synthetic_route() {
        let city = |island_index: u8, owner_slot: u8, name: &str| anno_formats::szs::City {
            island_index,
            owner_slot,
            tier_population: [0; 5],
            name: name.into(),
        };
        let island = |number: u8, x_pos: u16, y_pos: u16, city| Island {
            number,
            width: 8,
            height: 8,
            x_pos,
            y_pos,
            fertilities: [7; 8],
            tiles: Vec::new(),
            city: Some(city),
        };
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![
                island(0, 10, 10, city(0, 0, "A")),
                island(1, 40, 40, city(1, 0, "B")),
            ],
            players: vec![anno_formats::szs::PlayerSlotInit {
                state_byte: 0x00,
                ai_active: true,
                ..Default::default()
            }],
            mission: None,
            scenario: Default::default(),
            ships: vec![anno_formats::szs::Ship {
                raw_record: [0; anno_formats::szs::SHIP4_RECORD_BYTES],
                name: "Seehind".into(),
                x: 12,
                y: 13,
                owner: 0,
                figure_definition_id: anno_formats::szs::ShipClass::SmallTrader as u16,
                ship_class: anno_formats::szs::ShipClass::SmallTrader as u8,
                stored_energy: 0,
                runtime_slot: 0,
                figure_kind: 0,
                candidate_list_key: 0,
                source_direction: 0,
                animation_state: 0,
                heading_byte: 4,
                cargo_slots: [0; 7],
            }],
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert_eq!(sim.warehouses.len(), 2);
        assert!(sim.trade_routes.is_empty());
        assert_eq!(sim.trade_ships.len(), 1);
        assert_eq!(
            sim.trade_ships[0].route_id,
            anno_sim::data_bridge::UNROUTED_TRADER_ROUTE_ID
        );
        assert_eq!(
            (sim.trade_ships[0].world_x, sim.trade_ships[0].world_y),
            (12, 13)
        );
    }

    #[test]
    fn ship4_native_warship_does_not_receive_synthetic_patrol() {
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: Vec::new(),
            mission: None,
            scenario: Default::default(),
            ships: vec![anno_formats::szs::Ship {
                raw_record: [0; anno_formats::szs::SHIP4_RECORD_BYTES],
                name: "Raider".into(),
                x: 120,
                y: 240,
                owner: 5,
                figure_definition_id: anno_formats::szs::ShipClass::PirateShip as u16,
                ship_class: anno_formats::szs::ShipClass::PirateShip as u8,
                stored_energy: 0,
                runtime_slot: 0,
                figure_kind: 0,
                candidate_list_key: 0,
                source_direction: 0,
                animation_state: 0,
                heading_byte: 0,
                cargo_slots: [0; 7],
            }],
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert_eq!(sim.military_units.len(), 1);
        let ship = &sim.military_units[0];
        assert_eq!((ship.tile_x, ship.tile_y), (120, 240));
        assert_eq!((ship.target_x, ship.target_y), (120, 240));
        assert!(ship.patrol.is_empty());
        assert_eq!(sim.diplomacy.get(0, 5), Diplomacy::Neutral);
    }

    #[test]
    fn init_simulation_has_no_generated_objectives_without_mission_goals() {
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: Vec::new(),
            players: vec![anno_formats::szs::PlayerSlotInit {
                state_byte: 0x00,
                ai_active: true,
                ..Default::default()
            }],
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert!(sim.objectives.items.is_empty());
        assert_eq!(sim.players[0].total_population, 0);
    }

    #[test]
    fn init_simulation_loads_population_only_from_stadt4() {
        let szs = SzsFile {
            chunks: Vec::new(),
            islands: vec![Island {
                number: 0,
                width: 8,
                height: 8,
                x_pos: 10,
                y_pos: 10,
                fertilities: [7; 8],
                tiles: Vec::new(),
                city: Some(anno_formats::szs::City {
                    island_index: 0,
                    owner_slot: 0,
                    tier_population: [3, 5, 7, 11, 13],
                    name: "A".into(),
                }),
            }],
            players: vec![anno_formats::szs::PlayerSlotInit {
                state_byte: 0x00,
                ai_active: true,
                ..Default::default()
            }],
            mission: None,
            scenario: Default::default(),
            ships: Vec::new(),
            land_figures: Vec::new(),
            kontors: Vec::new(),
            settler_houses: Vec::new(),
        };
        let cod = CodFile {
            constants: Default::default(),
            buildings: Vec::new(),
        };

        let sim = init_simulation(&szs, &cod, &[], anno_sim::trade::ShipCargoConfig::default());

        assert_eq!(sim.players[0].population, [3, 5, 7, 11, 13]);
        assert_eq!(sim.players[0].total_population, 39);
    }

    #[test]
    fn ship_sprite_layout_uses_source_gfx_and_anim_offset() {
        fn def(gfx: i32, anim_offs: i32) -> FigureDef {
            FigureDef {
                gfx,
                anims: vec![FigureAnim {
                    nummer: 0,
                    anim_offs,
                    ..Default::default()
                }],
                ..Default::default()
            }
        }

        let small = def(0, 0);
        let large = def(32, 0);
        let handler = def(16, 0);
        let small_warship = def(64, 0);
        let large_warship = def(48, 0);
        let pirate = def(80, 0);
        let layout = ShipSpriteLayout::from_figure_defs(
            Some(&small),
            Some(&large),
            Some(&handler),
            Some(&small_warship),
            Some(&large_warship),
            Some(&pirate),
        );

        assert_eq!(layout.trader_base(TradeShipClass::SmallTrader), 0);
        assert_eq!(layout.trader_base(TradeShipClass::LargeTrader), 32);
        assert_eq!(layout.free_trader_base, 16);
        assert_eq!(layout.naval_base(UnitType::SmallWarship), Some(64));
        assert_eq!(layout.naval_base(UnitType::LargeWarship), Some(48));
        assert_eq!(layout.naval_base(UnitType::PirateShip), Some(80));
        assert_eq!(layout.naval_base(UnitType::Infantry), None);
        assert_eq!(figure_walk_sprite_base(Some(&def(40, 3)), 0), 43);
    }

    #[test]
    fn soldier_sprite_layout_uses_source_bases_and_owner_variants() {
        fn def(name: &str, gfx: i32, anim_speed: i32) -> FigureDef {
            FigureDef {
                name: name.into(),
                gfx,
                anims: vec![FigureAnim {
                    nummer: 0,
                    anim_anz: 8,
                    anim_speed,
                    ..Default::default()
                }],
                ..Default::default()
            }
        }
        let figures = anno_formats::figuren::FiguresFile {
            constants: Default::default(),
            figures: vec![
                def("SOLDAT1", 0, 80),
                def("SOLDAT2", 280, 80),
                def("SOLDAT3", 560, 80),
                def("SOLDAT4", 840, 80),
                def("KAVALERIE1", 1120, 75),
                def("KAVALERIE2", 1424, 75),
                def("KAVALERIE3", 1728, 75),
                def("KAVALERIE4", 2032, 75),
                def("KANONIER1", 2336, 95),
                def("KANONIER2", 2552, 95),
                def("KANONIER3", 2768, 95),
                def("KANONIER4", 2984, 95),
                def("MUSKETIER1", 3200, 100),
                def("MUSKETIER2", 3336, 100),
                def("MUSKETIER3", 3472, 100),
                def("MUSKETIER4", 3608, 100),
                def("SPEER1", 3744, 80),
            ],
        };
        let layout = SoldierSpriteLayout::from_figures(&figures);

        assert_eq!(
            layout.sprite_index(UnitType::Infantry, 0, 2, 0, false),
            Some(16),
        );
        assert_eq!(
            layout.sprite_index(UnitType::Infantry, 1, 7, 240, true),
            Some(339),
        );
        assert_eq!(
            layout.sprite_index(UnitType::Cavalry, 2, 1, 150, true),
            Some(1738),
        );
        assert_eq!(
            layout.sprite_index(UnitType::Cannon, 3, 0, 95, true),
            Some(2985),
        );
        assert_eq!(
            layout.sprite_index(UnitType::Musketeer, 4, 1, 200, true),
            Some(3618),
        );
        assert_eq!(
            layout.sprite_index(UnitType::NativeSpearman, 6, 1, 160, true),
            Some(3754),
        );
        assert_eq!(
            layout.sprite_index(UnitType::SmallWarship, 0, 0, 0, false),
            None,
        );
        assert_eq!(owner_sprite_variant(6), 3);
    }
}

/// Draw simulation entities (carriers, civilians, ships, military) on top of terrain.
fn overlay_entities(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    origin_x: i32,
    origin_y: i32,
    tile_w: i32,
    tile_h: i32,
    sim: &Simulation,
    world_mode: bool,
    current_island: Option<&Island>,
    islands: &[Island],
    carrier_sprites: &[(u32, u32, Vec<u8>)],
    worker_sprites: &[(u32, u32, Vec<u8>)],
    shadow_sprites: &[(u32, u32, Vec<u8>)],
    ship_sprites: &[(u32, u32, Vec<u8>)],
    soldier_sprites: &[(u32, u32, Vec<u8>)],
    selected_units: &[usize],
    selected_trade_ship_idx: Option<usize>,
    carrier_walk_anz: usize,
    carrier_empty_anim_offs: usize,
    carrier_loaded_anim_offs: usize,
    carrier_shadow_layout: CarrierShadowLayout,
    city_cart_shadow_layout: CarrierShadowLayout,
    carrier_shadow_y_offset: i32,
    city_cart_shadow_y_offset: i32,
    civilian_shadow_y_offset: i32,
    ship_sprite_layout: ShipSpriteLayout,
    soldier_sprite_layout: SoldierSpriteLayout,
    anim_elapsed_ms: u32,
) {
    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;

    // Helper: convert tile coords to screen pixel
    let tile_to_screen = |tx: i32, ty: i32, island_x: i32, island_y: i32| -> (i32, i32) {
        let wx = island_x + tx;
        let wy = island_y + ty;
        let sx = origin_x + (wx - wy) * half_tw;
        let sy = origin_y + (wx + wy) * half_th;
        (sx, sy)
    };

    // Draw carrier/civilian figures from their source BSH sprite families.
    // TRAEGER and KARREN both use 8 rotations × `carrier_walk_anz` frames,
    // laid out as base + dir*anz + frame. base_sprite selects the figure's
    // resolved Gfx base (TRAEGER=0, KARREN=496, ESEL=192, etc.).
    let carrier_frames_per_dir = carrier_walk_anz.max(1);
    let civilian_frames_per_dir = usize::from(sim.civilian_config.frames_per_dir.max(1));
    for figure in &sim.figures {
        if !figure.is_active() {
            continue;
        }
        let is_carrier = matches!(
            figure.action,
            ActionType::CarryingGoods | ActionType::Returning
        );
        let is_kind12 = sim.civilian_config.is_kind12(figure);
        let is_worker = sim.civilian_config.is_worker(figure);
        if !is_carrier && !is_kind12 {
            continue;
        }

        // Find island position for this figure's island
        let (ix, iy) = if world_mode {
            if is_kind12 {
                island_offset_for(figure.origin_island, sim, current_island, islands)
            } else if (figure.building_idx as usize) < sim.buildings.len() {
                let bld = &sim.buildings[figure.building_idx as usize];
                island_offset_for(bld.island_id, sim, current_island, islands)
            } else {
                (0, 0)
            }
        } else if let Some(island) = current_island {
            if is_kind12 && figure.origin_island != island.number {
                continue;
            }
            if !is_kind12 && (figure.building_idx as usize) < sim.buildings.len() {
                let bld = &sim.buildings[figure.building_idx as usize];
                if bld.island_id != island.number {
                    continue;
                }
            }
            (0, 0)
        } else {
            continue;
        };

        let (sx, sy) = if (is_carrier || is_kind12) && figure.source_position_initialized {
            let world_x = ix as f32 + figure.source_position_x;
            let world_y = iy as f32 + figure.source_position_y;
            // `FUN_00451220` converts a source figure's Z coordinate with
            // the horizontal tile scale divided by two. Here that is tile_h.
            let z_lift = (figure.source_position_z * tile_h as f32).round() as i32;
            (
                (origin_x as f32 + (world_x - world_y) * half_tw as f32).round() as i32,
                (origin_y as f32 + (world_x + world_y) * half_th as f32).round() as i32 - z_lift,
            )
        } else {
            // Source save reconstruction obtains every civilian's Z from
            // FUN_00451180(island, current X, current Y) before constructing
            // the source figure. Re-evaluate that live map-cell height for
            // the rendered tile rather than treating civilian sprites as
            // terrain-flat.
            let civilian_z_lift = if is_kind12 {
                sim.island_maps
                    .iter()
                    .find(|map| map.island_id == figure.origin_island)
                    .and_then(|map| map.source_terrain_height((figure.tile_x, figure.tile_y)))
                    .map(|height| source_terrain_z_lift(height, tile_h))
                    .unwrap_or(0)
            } else {
                0
            };
            let (sx, sy) = tile_to_screen(figure.tile_x, figure.tile_y, ix, iy);
            (sx, sy - civilian_z_lift)
        };

        // TRAEGER and KARREN have matching empty/loaded walk layouts in
        // figuren.cod: 8 rotations × 8 frames at offsets 0 and 64.
        //   anim 0 = empty walking, AnimOffs 0   (8 rotations × 8 frames = 64 sprites)
        //   anim 1 = loaded walking, AnimOffs 64 (same shape)
        // We pick the animation by figure action: empty when Returning,
        // loaded when CarryingGoods. The original game does NOT carry
        // per-good sprites — the loaded silhouette is generic.
        //
        // Civilian figures already store the resolved GFXZIVIL base
        // (`ADELWEIBL`..`PILGER`) in `base_sprite`, and their ANIM blocks
        // start at offset 0, so no TRAEGER loaded/empty offset applies.
        let mut carrier_shadow = None;
        let (sprite_idx, fallback_color, fallback_radius) = if is_kind12 {
            let anim_frame = source_walk_frame(figure.anim_frame, civilian_frames_per_dir);
            carrier_shadow = Some((
                carrier_shadow_layout.sprite_index(figure.direction, anim_frame),
                civilian_shadow_y_offset,
            ));
            (
                entity_walk_sprite_index(
                    figure.base_sprite,
                    0,
                    civilian_frames_per_dir,
                    figure.direction,
                    anim_frame,
                ),
                [0xE8, 0xD8, 0xB0, 0xFF],
                2,
            )
        } else {
            let anim_offs = if figure.action == ActionType::CarryingGoods {
                carrier_loaded_anim_offs
            } else {
                carrier_empty_anim_offs
            };
            let anim_frame = source_walk_frame(figure.anim_frame, carrier_frames_per_dir);
            let (shadow_layout, shadow_y_offset) = if figure.cargo_route == CargoRoute::CityCart {
                (city_cart_shadow_layout, city_cart_shadow_y_offset)
            } else {
                (carrier_shadow_layout, carrier_shadow_y_offset)
            };
            carrier_shadow = Some((
                shadow_layout.sprite_index(figure.direction, anim_frame),
                shadow_y_offset,
            ));
            // These source transport figures have only empty/loaded walk
            // animations, not per-good sprites.
            (
                entity_walk_sprite_index(
                    figure.base_sprite,
                    anim_offs,
                    carrier_frames_per_dir,
                    figure.direction,
                    anim_frame,
                ),
                match figure.action {
                    ActionType::CarryingGoods => [0xFF, 0xDD, 0x00, 0xFF],
                    ActionType::Returning => [0x88, 0xAA, 0x00, 0xFF],
                    _ => [0xFF, 0xFF, 0xFF, 0xFF],
                },
                3,
            )
        };

        if let Some((shadow_idx, shadow_y_offset)) = carrier_shadow {
            if let Some((sw, sh, data)) = shadow_sprites.get(shadow_idx) {
                if *sw > 0 && *sh > 0 {
                    let shadow_y_offset = source_shadow_y_offset(tile_w, shadow_y_offset);
                    blit_rgba_mask_color(
                        rgba,
                        img_w,
                        img_h,
                        sx + half_tw - *sw as i32 / 2,
                        sy + half_th - shadow_y_offset - *sh as i32 / 2,
                        data,
                        *sw,
                        *sh,
                        [0, 0, 0, 255],
                    );
                }
            }
        }

        let mut drew_sprite = false;
        let figure_sprites = if is_worker {
            worker_sprites
        } else {
            carrier_sprites
        };
        if sprite_idx < figure_sprites.len() {
            let (sw, sh, ref data) = figure_sprites[sprite_idx];
            if sw > 0 && sh > 0 {
                let dy = sy + half_th - sh as i32;
                blit_rgba(
                    rgba,
                    img_w,
                    img_h,
                    sx + half_tw - sw as i32 / 2,
                    dy,
                    data,
                    sw,
                    sh,
                );
                drew_sprite = true;
            }
        }
        if !drew_sprite {
            draw_marker(rgba, img_w, img_h, sx, sy, fallback_radius, &fallback_color);
        }
    }

    // Draw warehouses (blue squares)
    for wh in &sim.warehouses {
        let (ix, iy) = if world_mode {
            island_offset_for(wh.island_id, sim, current_island, islands)
        } else if let Some(island) = current_island {
            if wh.island_id != island.number {
                continue;
            }
            (0, 0)
        } else {
            continue;
        };

        let (sx, sy) = tile_to_screen(wh.tile_x as i32, wh.tile_y as i32, ix, iy);
        draw_marker(rgba, img_w, img_h, sx, sy, 4, &[0x40, 0x80, 0xFF, 0xFF]);
    }

    // Draw military units (sprites if available, colored markers fallback)
    for (uidx, unit) in sim.military_units.iter().enumerate() {
        if !unit.is_alive() {
            continue;
        }
        let (unit_x, unit_y, ix, iy, target, source_position) =
            if let Some(island_id) = unit.source_island_id {
                let Some(map) = sim
                    .island_maps
                    .iter()
                    .find(|map| map.island_id == island_id)
                else {
                    continue;
                };
                let Some((unit_x, unit_y)) = map.source_world_to_local((unit.tile_x, unit.tile_y))
                else {
                    continue;
                };
                if !world_mode && current_island.map(|island| island.number) != Some(island_id) {
                    continue;
                }
                let (ix, iy) = if world_mode {
                    (map.source_world_origin.0 / 2, map.source_world_origin.1 / 2)
                } else {
                    (0, 0)
                };
                let target = map
                    .source_world_to_local((unit.target_x, unit.target_y))
                    .unwrap_or((unit_x, unit_y));
                let source_position = unit.source_position_initialized.then_some((
                    unit.source_position_x - map.source_world_origin.0 as f32 * 0.5 - 0.25,
                    unit.source_position_y - map.source_world_origin.1 as f32 * 0.5 - 0.25,
                ));
                (unit_x, unit_y, ix, iy, target, source_position)
            } else {
                (
                    unit.tile_x,
                    unit.tile_y,
                    0,
                    0,
                    (unit.target_x, unit.target_y),
                    None,
                )
            };
        let (sx, sy) = if let Some((unit_x, unit_y)) = source_position {
            let world_x = ix as f32 + unit_x;
            let world_y = iy as f32 + unit_y;
            (
                (origin_x as f32 + (world_x - world_y) * half_tw as f32).round() as i32,
                (origin_y as f32 + (world_x + world_y) * half_th as f32).round() as i32,
            )
        } else {
            tile_to_screen(unit_x, unit_y, ix, iy)
        };
        let is_selected = selected_units.contains(&uidx);

        // Selection ring (drawn behind sprite/marker)
        if is_selected {
            let cx = sx + half_tw;
            let cy = sy + half_th;
            let r = (half_tw + half_th) / 2 + 2;
            draw_ring(rgba, img_w, img_h, cx, cy, r, &[0xFF, 0xFF, 0x00, 0xFF]);
        }

        if let Some(base) = ship_sprite_layout.naval_base(unit.unit_type) {
            let sprite_idx = live_ship_sprite_index(base, unit.direction);
            if sprite_idx < ship_sprites.len() {
                let (sw, sh, ref data) = ship_sprites[sprite_idx];
                if sw > 0 && sh > 0 {
                    blit_rgba(
                        rgba,
                        img_w,
                        img_h,
                        sx + half_tw - sw as i32 / 2,
                        sy - sh as i32 + half_th,
                        data,
                        sw,
                        sh,
                    );
                    if is_selected && (unit.tile_x != unit.target_x || unit.tile_y != unit.target_y)
                    {
                        let (tsx, tsy) = tile_to_screen(target.0, target.1, ix, iy);
                        draw_marker(
                            rgba,
                            img_w,
                            img_h,
                            tsx + half_tw,
                            tsy + half_th,
                            3,
                            &[0xFF, 0xFF, 0x00, 0xFF],
                        );
                    }
                    continue;
                }
            }
        }

        if let Some(sprite_idx) = soldier_sprite_layout.sprite_index(
            unit.unit_type,
            unit.owner,
            unit.direction,
            anim_elapsed_ms,
            unit.tile_x != unit.target_x || unit.tile_y != unit.target_y,
        ) {
            if sprite_idx < soldier_sprites.len() {
                let (sw, sh, ref data) = soldier_sprites[sprite_idx];
                if sw > 0 && sh > 0 {
                    blit_rgba(
                        rgba,
                        img_w,
                        img_h,
                        sx + half_tw - sw as i32 / 2,
                        sy - sh as i32 + half_th,
                        data,
                        sw,
                        sh,
                    );
                    if is_selected && (unit.tile_x != unit.target_x || unit.tile_y != unit.target_y)
                    {
                        let (tsx, tsy) = tile_to_screen(target.0, target.1, ix, iy);
                        draw_marker(
                            rgba,
                            img_w,
                            img_h,
                            tsx + half_tw,
                            tsy + half_th,
                            3,
                            &[0xFF, 0xFF, 0x00, 0xFF],
                        );
                    }
                    continue;
                }
            }
        }

        let color = if is_selected {
            [0xFF, 0xFF, 0x00, 0xFF]
        } else if unit.owner == 0 {
            [0x00, 0xFF, 0x00, 0xFF]
        } else {
            [0xFF, 0x40, 0x40, 0xFF]
        };
        let size = if unit.unit_type.stats().is_ranged {
            4
        } else {
            3
        };
        draw_marker(rgba, img_w, img_h, sx, sy, size, &color);
    }

    // Draw trade ships (sprites if available, cyan diamonds fallback).
    // SHIP.BSH live hull groups use `figuren.cod` Gfx as the first of
    // eight heading sprites. The matching dead/sinking hull is `Gfx + 8`.
    for (sidx, ship) in sim.trade_ships.iter().enumerate() {
        if !ship.active {
            continue;
        }
        let (sx, sy) = tile_to_screen(ship.world_x, ship.world_y, 0, 0);
        let is_selected = selected_trade_ship_idx == Some(sidx);
        if is_selected {
            let cx = sx + half_tw;
            let cy = sy + half_th;
            let r = (half_tw + half_th) / 2 + 2;
            draw_ring(rgba, img_w, img_h, cx, cy, r, &[0xFF, 0xFF, 0x00, 0xFF]);
        }

        let sprite_idx =
            live_ship_sprite_index(ship_sprite_layout.trader_base(ship.class), ship.heading);
        if sprite_idx < ship_sprites.len() {
            let (sw, sh, ref data) = ship_sprites[sprite_idx];
            if sw > 0 && sh > 0 {
                blit_rgba(
                    rgba,
                    img_w,
                    img_h,
                    sx + half_tw - sw as i32 / 2,
                    sy - sh as i32 + half_th,
                    data,
                    sw,
                    sh,
                );
                continue;
            }
        }
        let color = if is_selected {
            [0xFF, 0xFF, 0x00, 0xFF]
        } else {
            [0x00, 0xFF, 0xFF, 0xFF]
        };
        draw_diamond(rgba, img_w, img_h, sx, sy, 5, &color);
    }

    // Free traders are figuren.cod `HANDLER` ships, not player
    // HANDEL1/HANDEL2 hulls.
    for trader in &sim.free_traders {
        if !trader.active {
            continue;
        }
        let (sx, sy) = tile_to_screen(trader.world_x, trader.world_y, 0, 0);
        let sprite_idx =
            live_ship_sprite_index(ship_sprite_layout.free_trader_base, trader.heading);
        if sprite_idx < ship_sprites.len() {
            let (sw, sh, ref data) = ship_sprites[sprite_idx];
            if sw > 0 && sh > 0 {
                blit_rgba(
                    rgba,
                    img_w,
                    img_h,
                    sx + half_tw - sw as i32 / 2,
                    sy - sh as i32 + half_th,
                    data,
                    sw,
                    sh,
                );
                continue;
            }
        }
        draw_diamond(rgba, img_w, img_h, sx, sy, 5, &[0xFF, 0xC0, 0x40, 0xFF]);
    }
}

/// World-space tile offset for island `island_id`. The SZS file
/// stores per-island `x_pos`/`y_pos` (the position of the island's
/// top-left tile within the world grid); world-mode rendering needs
/// these offsets so islands don't stack on top of each other.
fn island_offset_for(
    island_id: u8,
    _sim: &Simulation,
    _current_island: Option<&Island>,
    islands: &[Island],
) -> (i32, i32) {
    islands
        .iter()
        .find(|i| i.number == island_id)
        .map(|i| (i.x_pos as i32, i.y_pos as i32))
        .unwrap_or((0, 0))
}

/// Draw a filled square marker centered at (cx, cy).
fn draw_marker(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: &[u8; 4],
) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let px = cx + dx;
            let py = cy + dy;
            if px >= 0 && py >= 0 && (px as u32) < img_w && (py as u32) < img_h {
                let off = ((py as u32 * img_w + px as u32) * 4) as usize;
                if off + 3 < rgba.len() {
                    rgba[off] = color[0];
                    rgba[off + 1] = color[1];
                    rgba[off + 2] = color[2];
                    rgba[off + 3] = color[3];
                }
            }
        }
    }
}

/// Draw a hollow ring (1px stroke) centered at (cx, cy) with the given radius.
fn draw_ring(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: &[u8; 4],
) {
    if radius <= 0 {
        return;
    }
    let r2_outer = (radius * radius) as i32;
    let r2_inner = ((radius - 1).max(0) * (radius - 1).max(0)) as i32;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let d2 = dx * dx + dy * dy;
            if d2 <= r2_outer && d2 >= r2_inner {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as u32) < img_w && (py as u32) < img_h {
                    let off = ((py as u32 * img_w + px as u32) * 4) as usize;
                    if off + 3 < rgba.len() {
                        rgba[off] = color[0];
                        rgba[off + 1] = color[1];
                        rgba[off + 2] = color[2];
                        rgba[off + 3] = color[3];
                    }
                }
            }
        }
    }
}

/// Draw a diamond marker centered at (cx, cy).
fn draw_diamond(
    rgba: &mut [u8],
    img_w: u32,
    img_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: &[u8; 4],
) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx.abs() + dy.abs() <= radius {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as u32) < img_w && (py as u32) < img_h {
                    let off = ((py as u32 * img_w + px as u32) * 4) as usize;
                    if off + 3 < rgba.len() {
                        rgba[off] = color[0];
                        rgba[off + 1] = color[1];
                        rgba[off + 2] = color[2];
                        rgba[off + 3] = color[3];
                    }
                }
            }
        }
    }
}

fn decode_sprites(
    mgr: &SpriteManager,
    zoom: usize,
    palette: &[[u8; 3]; 256],
) -> Vec<(u32, u32, Vec<u8>)> {
    match mgr.get_set(SpriteCategory::Stadtfld, zoom) {
        Some(set) => set
            .bsh
            .sprites
            .iter()
            .map(|s| (s.width, s.height, s.decode(palette)))
            .collect(),
        None => Vec::new(),
    }
}

fn decode_entity_sprites(
    mgr: &SpriteManager,
    category: SpriteCategory,
    zoom: usize,
    palette: &[[u8; 3]; 256],
) -> Vec<(u32, u32, Vec<u8>)> {
    match mgr.get_set(category, zoom) {
        Some(set) => set
            .bsh
            .sprites
            .iter()
            .map(|s| (s.width, s.height, s.decode(palette)))
            .collect(),
        None => Vec::new(),
    }
}

/// Expand one source command into the STADTFLD cells that the original live
/// map writer creates before the draw loop consumes them.
fn source_command_gfx_tiles(
    island_id: u8,
    tile_x: u16,
    tile_y: u16,
    command: anno_sim::building::SourceBuildingCommand,
    cod: &CodFile,
) -> Vec<(u8, u16, u16, u16)> {
    source_command_gfx_tiles_with_state(island_id, tile_x, tile_y, command, None, cod)
}

fn source_command_gfx_tiles_with_state(
    island_id: u8,
    tile_x: u16,
    tile_y: u16,
    command: anno_sim::building::SourceBuildingCommand,
    state: Option<&anno_sim::source_cell::SourceMapCellState>,
    cod: &CodFile,
) -> Vec<(u8, u16, u16, u16)> {
    let source_id =
        anno_formats::szs::INSELHAUS_SOURCE_ID_BASE + i32::from(command.definition_offset);
    let Some(definition) = cod.building_by_source_id(source_id) else {
        return vec![(island_id, tile_x, tile_y, command.definition_offset)];
    };
    let (source_width, source_height) = definition.size;
    if source_width <= 0 || source_height <= 0 {
        return Vec::new();
    }
    let animation_frame_offset = if definition.anim_anz > 1 {
        source_command_frame_selector(definition, command.variant, state) * definition.anim_add
    } else {
        0
    };
    let rotation_stride = definition.anim_anz * definition.anim_add;
    let rotation_offset = i32::from(command.orientation & 3) * rotation_stride;
    let mut tiles = Vec::new();
    for (dx, dy, source_cell) in
        source_command_cell_order(source_width, source_height, command.orientation)
    {
        let sprite = definition.gfx + animation_frame_offset + rotation_offset + source_cell;
        let (Ok(x), Ok(y), Ok(sprite)) = (
            u16::try_from(i32::from(tile_x) + dx),
            u16::try_from(i32::from(tile_y) + dy),
            u16::try_from(sprite),
        ) else {
            continue;
        };
        tiles.push((island_id, x, y, sprite));
    }
    tiles
}

/// Select the frame consumed by the STADTFLD draw loop for one source command.
/// A missing record is the fresh INSELHAUS-load path; live records are keyed by
/// the command root and supply the kind-specific activity/progress selectors.
fn source_command_frame_selector(
    definition: &anno_formats::cod::BuildingDef,
    packed_variant: u8,
    state: Option<&anno_sim::source_cell::SourceMapCellState>,
) -> i32 {
    // The STADTFLD draw loop switches on the nested `HAUS_PRODTYP Kind` at
    // definition offset `+0x1c` (`switch(*(undefined4 *)(iVar4 + 0x1c))`,
    // `1602_exe.c:98270-98300`), the same selector that decides whether a
    // live cell record exists at all.
    if let Some(state) = state {
        match definition.source_production_kind_code() {
            Some(1..=6) if definition.storage_animation => {
                return state.storage_frame_selector(definition.anim_anz);
            }
            Some(1..=6) => return state.activity_frame_selector(definition.anim_anz),
            Some(7) => return state.market_frame_selector(definition.anim_anz),
            _ => {}
        }
    }
    source_command_initial_frame_selector(definition, packed_variant)
}

/// Reproduce the selector value used by the STADTFLD draw loop immediately
/// after an INSELHAUS command creates its live cell record. `FUN_00481fc0`
/// clears that record, so source kinds 1 through 8 start at frame zero;
/// their later state-machine transitions are handled by separate routines.
fn source_command_initial_frame_selector(
    definition: &anno_formats::cod::BuildingDef,
    packed_variant: u8,
) -> i32 {
    match definition.source_production_kind_code() {
        Some(1..=8) => 0,
        Some(10) if definition.anim_time == 0 => i32::from(packed_variant),
        _ => i32::from(packed_variant) + definition.anim_frame,
    }
    .rem_euclid(definition.anim_anz)
}

/// Enumerate map cells in the order `FUN_00463b10` assigns consecutive GFX
/// indices while writing an oriented source command into the live map.
fn source_command_cell_order(width: i32, height: i32, orientation: u8) -> Vec<(i32, i32, i32)> {
    let mut cells = Vec::with_capacity(usize::try_from(width * height).unwrap_or(0));
    let mut source_cell = 0;
    let mut push = |dx, dy| {
        cells.push((dx, dy, source_cell));
        source_cell += 1;
    };
    match orientation & 3 {
        0 => {
            for dy in 0..height {
                for dx in 0..width {
                    push(dx, dy);
                }
            }
        }
        1 => {
            for dx in (0..height).rev() {
                for dy in 0..width {
                    push(dx, dy);
                }
            }
        }
        2 => {
            for dy in (0..height).rev() {
                for dx in (0..width).rev() {
                    push(dx, dy);
                }
            }
        }
        3 => {
            for dx in 0..height {
                for dy in (0..width).rev() {
                    push(dx, dy);
                }
            }
        }
        _ => unreachable!(),
    }
    cells
}

/// Materialize an authored INSELHAUS command stream in source order. Later
/// commands replace earlier cells, matching the mutable map written by
/// `FUN_004653a0`.
fn authored_island_gfx_tiles(
    island: &Island,
    cod: &CodFile,
    states: &[anno_sim::source_cell::SourceMapCellState],
) -> Vec<(u16, u16, u16)> {
    let mut cells = std::collections::BTreeMap::new();
    for tile in &island.tiles {
        let command = anno_sim::building::SourceBuildingCommand::from_island_tile(*tile);
        let state = states
            .iter()
            .find(|state| state.matches(island.number, u16::from(tile.x), u16::from(tile.y)));
        for (_, x, y, sprite) in source_command_gfx_tiles_with_state(
            island.number,
            u16::from(tile.x),
            u16::from(tile.y),
            command,
            state,
            cod,
        ) {
            if x < u16::from(island.width) && y < u16::from(island.height) {
                cells.insert((x, y), sprite);
            }
        }
    }
    cells
        .into_iter()
        .map(|((x, y), sprite)| (x, y, sprite))
        .collect()
}

/// Render all islands; returns (rgba, width, height, origin_x, origin_y).
fn runtime_building_gfx_tiles(
    buildings: &[BuildingInstance],
    cod: &CodFile,
    states: &[anno_sim::source_cell::SourceMapCellState],
) -> Vec<(u8, u16, u16, u16)> {
    buildings
        .iter()
        .filter(|building| building.active)
        .filter_map(|building| {
            building
                .source_placement_command
                .map(|command| (building, command))
        })
        .flat_map(|(building, command)| {
            source_command_gfx_tiles_with_state(
                building.island_id,
                building.tile_x,
                building.tile_y,
                command,
                states.iter().find(|state| {
                    state.matches(building.island_id, building.tile_x, building.tile_y)
                }),
                cod,
            )
        })
        .collect()
}

fn render_world(
    islands: &[Island],
    sprites: &[(u32, u32, Vec<u8>)],
    num_sprites: usize,
    tile_w: i32,
    tile_h: i32,
    buildings: &[BuildingInstance],
    source_map_cell_states: &[anno_sim::source_cell::SourceMapCellState],
    cod: &CodFile,
) -> (Vec<u8>, u32, u32, i32, i32) {
    let max_world_x = islands
        .iter()
        .map(|i| i.x_pos as i32 + i.width as i32)
        .max()
        .unwrap_or(100);
    let max_world_y = islands
        .iter()
        .map(|i| i.y_pos as i32 + i.height as i32)
        .max()
        .unwrap_or(100);

    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;

    let img_w = ((max_world_x + max_world_y) * half_tw + tile_w) as u32;
    let img_h = ((max_world_x + max_world_y) * half_th + tile_h + 500) as u32;

    // Cap at reasonable size
    let scale = if img_w > 8192 || img_h > 8192 {
        8192.0 / img_w.max(img_h) as f64
    } else {
        1.0
    };

    let final_w = (img_w as f64 * scale) as u32;
    let final_h = (img_h as f64 * scale) as u32;

    let origin_x;
    let origin_y;
    let runtime_tiles = runtime_building_gfx_tiles(buildings, cod, source_map_cell_states);
    let mut world_tiles = Vec::new();
    for island in islands {
        for (x, y, sprite) in authored_island_gfx_tiles(island, cod, source_map_cell_states) {
            let wx = island.x_pos as i32 + i32::from(x);
            let wy = island.y_pos as i32 + i32::from(y);
            world_tiles.push((wx, wy, sprite));
        }
    }
    for (island_id, x, y, sprite) in &runtime_tiles {
        let Some(island) = islands.iter().find(|island| island.number == *island_id) else {
            continue;
        };
        let wx = island.x_pos as i32 + i32::from(*x);
        let wy = island.y_pos as i32 + i32::from(*y);
        world_tiles.push((wx, wy, *sprite));
    }
    world_tiles.sort_by_key(|&(x, y, _)| (x + y, y));

    if scale < 1.0 {
        let s_half_tw = (half_tw as f64 * scale) as i32;
        let s_half_th = (half_th as f64 * scale) as i32;

        let mut rgba = vec![0u8; (final_w * final_h * 4) as usize];

        origin_x = (max_world_y as f64 * s_half_tw as f64) as i32;
        origin_y = (100.0 * scale) as i32;

        for &(wx, wy, sprite) in &world_tiles {
            let sx = origin_x + (wx - wy) * s_half_tw;
            let sy = origin_y + (wx + wy) * s_half_th;

            let sprite_idx = sprite as usize;
            if sprite_idx < num_sprites {
                let (sw, sh, ref sdata) = sprites[sprite_idx];
                if sw > 0 && sh > 0 {
                    let cx = sw / 2;
                    let cy = sh / 2;
                    let off = ((cy * sw + cx) * 4) as usize;
                    if off + 3 < sdata.len() && sdata[off + 3] > 0 {
                        let r = sdata[off];
                        let g = sdata[off + 1];
                        let b = sdata[off + 2];
                        for dy in 0..s_half_th.max(1) {
                            for dx in 0..s_half_tw.max(1) {
                                let px = sx + dx;
                                let py = sy + dy;
                                if px >= 0
                                    && py >= 0
                                    && (px as u32) < final_w
                                    && (py as u32) < final_h
                                {
                                    let doff = ((py as u32 * final_w + px as u32) * 4) as usize;
                                    if doff + 3 < rgba.len() {
                                        rgba[doff] = r;
                                        rgba[doff + 1] = g;
                                        rgba[doff + 2] = b;
                                        rgba[doff + 3] = 255;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        return (rgba, final_w, final_h, origin_x, origin_y);
    }

    // Full resolution
    let mut rgba = vec![0u8; (img_w * img_h * 4) as usize];
    origin_x = max_world_y * half_tw;
    origin_y = 300;

    for &(wx, wy, sprite) in &world_tiles {
        let sx = origin_x + (wx - wy) * half_tw;
        let sy = origin_y + (wx + wy) * half_th;

        let sprite_idx = sprite as usize;
        if sprite_idx >= num_sprites {
            continue;
        }

        let (sw, sh, ref sprite_data) = sprites[sprite_idx];
        if sw == 0 || sh == 0 {
            continue;
        }

        blit_rgba(
            &mut rgba,
            img_w,
            img_h,
            sx,
            sy - (sh as i32 - tile_h),
            sprite_data,
            sw,
            sh,
        );
    }

    (rgba, img_w, img_h, origin_x, origin_y)
}

/// Render a single island; returns (rgba, width, height, origin_x, origin_y).
fn render_island(
    island: &Island,
    sprites: &[(u32, u32, Vec<u8>)],
    num_sprites: usize,
    tile_w: i32,
    tile_h: i32,
    buildings: &[BuildingInstance],
    source_map_cell_states: &[anno_sim::source_cell::SourceMapCellState],
    cod: &CodFile,
) -> (Vec<u8>, u32, u32, i32, i32) {
    let iw = island.width as i32;
    let ih = island.height as i32;

    let half_tw = tile_w / 2;
    let half_th = tile_h / 2;

    let img_w = ((iw + ih) * half_tw) as u32 + tile_w as u32;
    let img_h = ((iw + ih) * half_th) as u32 + tile_h as u32 + 500;

    let mut rgba = vec![0u8; (img_w * img_h * 4) as usize];

    let origin_x = ih * half_tw;
    let origin_y = 300;

    let mut sorted_tiles = authored_island_gfx_tiles(island, cod, source_map_cell_states)
        .into_iter()
        .map(|(x, y, sprite)| (i32::from(x), i32::from(y), sprite))
        .collect::<Vec<_>>();
    sorted_tiles.extend(
        runtime_building_gfx_tiles(buildings, cod, source_map_cell_states)
            .into_iter()
            .filter(|(island_id, _, _, _)| *island_id == island.number)
            .map(|(_, x, y, sprite)| (i32::from(x), i32::from(y), sprite)),
    );
    sorted_tiles.sort_by_key(|(x, y, _)| (x + y, *y));

    for &(tx, ty, sprite) in &sorted_tiles {
        let sx = origin_x + (tx - ty) * half_tw;
        let sy = origin_y + (tx + ty) * half_th;

        let sprite_idx = sprite as usize;
        if sprite_idx >= num_sprites {
            continue;
        }

        let (sw, sh, ref sprite_data) = sprites[sprite_idx];
        if sw == 0 || sh == 0 {
            continue;
        }

        blit_rgba(
            &mut rgba,
            img_w,
            img_h,
            sx,
            sy - (sh as i32 - tile_h),
            sprite_data,
            sw,
            sh,
        );
    }

    (rgba, img_w, img_h, origin_x, origin_y)
}

fn blit_rgba(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
) {
    for row in 0..src_h as i32 {
        let dy = y + row;
        if dy < 0 || dy >= dst_h as i32 {
            continue;
        }
        for col in 0..src_w as i32 {
            let dx = x + col;
            if dx < 0 || dx >= dst_w as i32 {
                continue;
            }
            let src_off = ((row as u32 * src_w + col as u32) * 4) as usize;
            if src_off + 3 >= src.len() {
                continue;
            }
            if src[src_off + 3] == 0 {
                continue;
            }
            let dst_off = ((dy as u32 * dst_w + dx as u32) * 4) as usize;
            if dst_off + 3 >= dst.len() {
                continue;
            }
            dst[dst_off] = src[src_off];
            dst[dst_off + 1] = src[src_off + 1];
            dst[dst_off + 2] = src[src_off + 2];
            dst[dst_off + 3] = 255;
        }
    }
}

fn blit_rgba_mask_color(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    x: i32,
    y: i32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    color: [u8; 4],
) {
    for row in 0..src_h as i32 {
        let dy = y + row;
        if dy < 0 || dy >= dst_h as i32 {
            continue;
        }
        for col in 0..src_w as i32 {
            let dx = x + col;
            if dx < 0 || dx >= dst_w as i32 {
                continue;
            }
            let src_off = ((row as u32 * src_w + col as u32) * 4) as usize;
            if src_off + 3 >= src.len() || src[src_off + 3] == 0 {
                continue;
            }
            let dst_off = ((dy as u32 * dst_w + dx as u32) * 4) as usize;
            if dst_off + 3 >= dst.len() {
                continue;
            }
            dst[dst_off..dst_off + 4].copy_from_slice(&color);
        }
    }
}

fn find_data_dir() -> std::path::PathBuf {
    for candidate in &["extracted", "../extracted", "../../extracted"] {
        let p = std::path::Path::new(candidate);
        if p.join("GFX/STADTFLD.BSH").exists() || p.join("haeuser.cod").exists() {
            return p.to_path_buf();
        }
    }
    eprintln!("Could not find game data directory.");
    std::process::exit(1);
}
