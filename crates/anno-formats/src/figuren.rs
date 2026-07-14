//! Parser for `figuren.cod` — figure (unit) definitions.
//!
//! Same byte-negation encryption + CP1252 text encoding as `haeuser.cod`,
//! but the entry shape is different:
//!
//! - Top-level entries are opened by `Nummer: <NAME>` (not `@Nummer: <num>`).
//! - Each figure has `Gfx:`, `Blocknr:`, `Rotate:`, and one or more
//!   `Objekt: ANIM` sub-blocks with their own `Nummer:` (animation index)
//!   plus `AnimOffs/AnimAdd/AnimAnz/AnimSpeed/Kind`.
//!
//! We resolve constants (e.g. `GFXTRAEGER = 0`, `GFXSHIP = 0`,
//! `GFXESEL = GFXTRAEGER+192`) so that `Gfx: GFXSHIP+32` becomes `gfx = 32`.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct FigureAnim {
    /// Animation index within the figure (Nummer: 0, 1, 2, …).
    pub nummer: i32,
    /// Loop kind (e.g. ENDLESS).
    pub kind: String,
    /// Offset (in sprites) from the figure's `gfx` base where this anim starts.
    pub anim_offs: i32,
    /// Sprite stride between consecutive frames.
    pub anim_add: i32,
    /// Number of frames in the animation cycle.
    pub anim_anz: i32,
    /// Time per frame in some figure-time unit (typically ms × scale).
    pub anim_speed: i32,
}

#[derive(Debug, Clone, Default)]
pub struct FigureDef {
    /// Symbolic name (e.g. "TRAEGER", "HANDEL1").
    pub name: String,
    /// Resolved sprite base in the figure's BSH file.
    pub gfx: i32,
    /// BSH block / category index (1=Soldat, 2=Ship, 3=Traeger, 4=Maeher, …).
    pub blocknr: i32,
    /// Number of separate rotation slices stored in BSH (1 if rotation is
    /// encoded inside the animation frames instead).
    pub rotate: i32,
    /// All raw scalar properties (Speed, Maxware, etc.) — keep around so
    /// callers can reach values we haven't promoted to typed fields.
    pub properties: HashMap<String, String>,
    /// Animation sub-objects in declaration order.
    pub anims: Vec<FigureAnim>,
}

#[derive(Debug)]
pub struct FiguresFile {
    pub constants: HashMap<String, i32>,
    pub figures: Vec<FigureDef>,
}

impl FigureDef {
    /// Walking animation: by convention animation 0 (`Nummer: 0`) inside
    /// the figure's first ANIM block. Returns it if present.
    pub fn walk_anim(&self) -> Option<&FigureAnim> {
        self.anim(0)
    }

    /// Animation by numeric `Nummer:` inside this figure.
    pub fn anim(&self, nummer: i32) -> Option<&FigureAnim> {
        self.anims.iter().find(|a| a.nummer == nummer)
    }

    /// Total sprites consumed by all rotations × walk frames, useful for
    /// laying out the BSH region: `gfx .. gfx + rotate * walk_anz`.
    pub fn walk_sprite_count(&self) -> i32 {
        let walk = self.walk_anim().map(|a| a.anim_anz).unwrap_or(0);
        self.rotate.max(1) * walk.max(1)
    }

    /// Lookup a numeric property (Speed/Hitpoint/Maxtrag/etc.) from
    /// the property bag, returning 0 when absent.
    fn prop_int(&self, key: &str) -> i32 {
        self.properties.get(key)
            .and_then(|s| s.split(',').next().and_then(|t| t.trim().parse().ok()))
            .unwrap_or(0)
    }

    /// Walking speed (`Speed:` in figuren.cod) in 1/100 tile-per-tick
    /// units. Common values: 200 (citizens), 220 (carrier), 230 (adel),
    /// 250 (child), 260 (soldier), 300 (cart), 400 (cavalry).
    pub fn speed(&self) -> i32 { self.prop_int("Speed") }

    /// Maximum cargo this figure carries in one trip (`Maxtrag:`).
    /// 4 for civilian/Träger figures, 6 for KARREN (cart).
    pub fn max_load(&self) -> i32 { self.prop_int("Maxtrag") }

    /// Ship cargo slots (`Maxware:`). Trade ship cargo capacity is
    /// `Maxware × 10` tons.
    pub fn max_ware(&self) -> i32 { self.prop_int("Maxware") }

    /// Hit-points for combat figures (`Hitpoint:`) as a float.
    /// Naval ships carry fractional values (KRIEG1 = 2.0,
    /// KRIEG2 = 4.0 etc.); land units use whole numbers.
    pub fn hit_points_f32(&self) -> f32 {
        self.properties.get("Hitpoint")
            .and_then(|s| s.split(',').next().and_then(|t| t.trim().parse().ok()))
            .unwrap_or(0.0)
    }

    /// Hit-points truncated to i32 for callers that want
    /// integer HP. Use `hit_points_f32` if you need the
    /// authentic fractional value (KRIEG1 = 2.0, etc.).
    pub fn hit_points(&self) -> i32 { self.hit_points_f32() as i32 }

    /// Maximum energy (`Maxenergy:`) — legacy "stamina" cap
    /// that doubles as the ship's HP-for-display value
    /// (KRIEG1 = 65, KRIEG2 = 120 — matches Tim Howgego's
    /// military-data appendix).
    pub fn max_energy(&self) -> i32 { self.prop_int("Maxenergy") }

    /// Build cost in gold (`Preis:`). Used for non-Soldat figures
    /// (ships, carts) that aren't gated through the SOLDAT lookup.
    pub fn price(&self) -> i32 { self.prop_int("Preis") }

    /// Workload duration / cycle time (`Worktime:`). Plantation
    /// workers (MAEHER, HOLZFAELLER, etc.) have 8; civilians 3.
    pub fn worktime(&self) -> i32 { self.prop_int("Worktime") }

    /// Maximum cannons mountable on a naval figure (`Maxkanon:`).
    /// Used by ship-arming UI in lieu of the hard-coded
    /// `combat::cannon_capacity` table when figuren.cod is
    /// available.
    pub fn max_cannons(&self) -> i32 { self.prop_int("Maxkanon") }

    /// Ship firing/approach radius (`Shotradius:`). The figure loader writes
    /// this unsigned 16-bit value at runtime offset `+0x4a`; the ship-route
    /// caller `FUN_00455a20` supplies `Shotradius >> 3` to
    /// `FUN_0046dde0` when it marks target-approach rays.
    pub fn shot_radius(&self) -> i32 { self.prop_int("Shotradius") }
}

impl FiguresFile {
    pub fn parse(data: &[u8]) -> Self {
        let text = decrypt(data);
        Self::parse_text(&text)
    }

    fn parse_text(text: &str) -> Self {
        let mut constants: HashMap<String, i32> = HashMap::new();
        let mut figures: Vec<FigureDef> = Vec::new();
        let mut current: Option<FigureDef> = None;
        let mut current_anim: Option<FigureAnim> = None;
        let mut in_anim_block = false;
        let mut obj_depth = 0i32;
        let mut base_template: Option<FigureDef> = None;
        let mut current_marks_base_template = false;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') { continue; }
            let line = line.split(';').next().unwrap_or(line).trim();
            if line.is_empty() { continue; }

            // Sub-object boundaries. ANIM is the only nested form we model;
            // FIGUR / FORMATION are top-level containers whose contents
            // behave like ordinary top-level definitions, so we skip them.
            if let Some(rest) = line.strip_prefix("Objekt:") {
                let kind = rest.trim();
                if kind.eq_ignore_ascii_case("ANIM") {
                    obj_depth += 1;
                    in_anim_block = true;
                }
                continue;
            }
            if line.starts_with("EndObj") {
                if in_anim_block {
                    if let (Some(fig), Some(anim)) = (current.as_mut(), current_anim.take()) {
                        fig.anims.push(anim);
                    }
                    in_anim_block = false;
                    obj_depth = (obj_depth - 1).max(0);
                }
                continue;
            }

            // Constants of the form `NAME = EXPR`.
            if let Some((name, expr)) = line.split_once('=') {
                let name = name.trim();
                let expr = expr.trim();
                if !in_anim_block
                    && current.is_some()
                    && name.eq_ignore_ascii_case("BASE")
                    && expr.eq_ignore_ascii_case("Nummer")
                {
                    current_marks_base_template = true;
                    continue;
                }
                if !name.is_empty()
                    && !expr.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    let val = eval(&constants, expr);
                    constants.insert(name.to_string(), val);
                    continue;
                }
            }

            // `Nummer: <token>` — new figure (or new anim sub-record, or
            // a key/value inside a non-ANIM Objekt sub-block we don't model).
            if let Some(val) = line.strip_prefix("Nummer:") {
                let val = val.trim();
                if in_anim_block {
                    // Push prior anim within this Objekt block, start a new one.
                    if let (Some(fig), Some(anim)) = (current.as_mut(), current_anim.take()) {
                        fig.anims.push(anim);
                    }
                    let nummer = eval(&constants, val);
                    current_anim = Some(FigureAnim {
                        nummer,
                        ..Default::default()
                    });
                } else if obj_depth > 0 {
                    // Inside a non-ANIM Objekt: keep as raw property; don't
                    // treat as a figure boundary.
                    if let Some(fig) = current.as_mut() {
                        fig.properties.insert("InnerNummer".to_string(), val.to_string());
                    }
                } else {
                    if let Some(fig) = current.take() {
                        if current_marks_base_template {
                            base_template = Some(fig.clone());
                            current_marks_base_template = false;
                        }
                        figures.push(fig);
                    }
                    let mut fig = FigureDef::default();
                    fig.name = val.to_string();
                    current = Some(fig);
                }
                continue;
            }

            // ObjFill: TEMPLATE — copy properties (and gfx/rotate/blocknr/anims)
            // from a previously defined figure with the given name.
            if let Some(val) = line.strip_prefix("ObjFill:") {
                let template = val.trim();
                let src = if template.eq_ignore_ascii_case("BASE") {
                    base_template.clone()
                } else {
                    figures.iter().find(|f| f.name == template).cloned()
                };
                if let Some(src) = src {
                    if let Some(fig) = current.as_mut() {
                        fig.gfx = src.gfx;
                        fig.blocknr = src.blocknr;
                        fig.rotate = src.rotate;
                        fig.anims = src.anims.clone();
                        for (k, v) in src.properties {
                            fig.properties.entry(k).or_insert(v);
                        }
                    }
                }
                continue;
            }

            // @Gfx: relative offset (+N or -N) modifies the inherited gfx.
            if let Some(val) = line.strip_prefix("@Gfx:") {
                let val = val.trim();
                let delta = if let Some(rest) = val.strip_prefix('+') {
                    eval(&constants, rest.trim())
                } else if let Some(rest) = val.strip_prefix('-') {
                    -eval(&constants, rest.trim())
                } else {
                    eval(&constants, val) - current.as_ref().map(|f| f.gfx).unwrap_or(0)
                };
                if let Some(fig) = current.as_mut() {
                    fig.gfx += delta;
                }
                continue;
            }

            // Inside ANIM sub-objects: parse anim fields.
            if in_anim_block {
                if let Some(rest) = line.strip_prefix("Kind:") {
                    if let Some(a) = current_anim.as_mut() {
                        a.kind = rest.trim().to_string();
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("AnimOffs:") {
                    if let Some(a) = current_anim.as_mut() {
                        a.anim_offs = eval(&constants, rest.trim());
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("AnimAdd:") {
                    if let Some(a) = current_anim.as_mut() {
                        a.anim_add = eval(&constants, rest.trim());
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("AnimAnz:") {
                    if let Some(a) = current_anim.as_mut() {
                        a.anim_anz = eval(&constants, rest.trim());
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("AnimSpeed:") {
                    if let Some(a) = current_anim.as_mut() {
                        a.anim_speed = eval(&constants, rest.trim());
                    }
                    continue;
                }
                // Fall-through: ignore other anim props.
                continue;
            }

            // Outside anim blocks: figure-level properties.
            if let Some(rest) = line.strip_prefix("Gfx:") {
                if let Some(fig) = current.as_mut() {
                    fig.gfx = eval(&constants, rest.trim());
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("Blocknr:") {
                if let Some(fig) = current.as_mut() {
                    fig.blocknr = eval(&constants, rest.trim());
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("Rotate:") {
                if let Some(fig) = current.as_mut() {
                    fig.rotate = eval(&constants, rest.trim());
                }
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                if let Some(fig) = current.as_mut() {
                    fig.properties.insert(
                        key.trim().to_string(),
                        value.trim().to_string(),
                    );
                }
                continue;
            }
        }

        if let (Some(fig), Some(anim)) = (current.as_mut(), current_anim.take()) {
            fig.anims.push(anim);
        }
        if let Some(fig) = current.take() {
            figures.push(fig);
        }

        FiguresFile { constants, figures }
    }

    /// Find a figure by exact symbolic name (e.g. "TRAEGER", "HANDEL1").
    pub fn find(&self, name: &str) -> Option<&FigureDef> {
        self.figures.iter().find(|f| f.name == name)
    }

}

fn decrypt(data: &[u8]) -> String {
    let raw: Vec<u8> = if data.len() > 4
        && data[..4].iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
    {
        data.to_vec()
    } else {
        data.iter().map(|&b| (-(b as i16) & 0xFF) as u8).collect()
    };
    raw.iter().map(|&b| char::from(b)).collect()
}

/// Evaluate `NUMBER`, `CONSTANT`, or `CONSTANT±NUMBER`.
fn eval(constants: &HashMap<String, i32>, expr: &str) -> i32 {
    let expr = expr.trim();
    if expr.is_empty() { return 0; }

    if let Ok(n) = expr.parse::<i32>() {
        return n;
    }
    for op in ['+', '-'] {
        if let Some(idx) = expr.rfind(op) {
            if idx == 0 { continue; }
            let (left, right) = expr.split_at(idx);
            let left = left.trim();
            let right = right[1..].trim();
            let lv = eval(constants, left);
            let rv = eval(constants, right);
            return if op == '+' { lv + rv } else { lv - rv };
        }
    }
    *constants.get(expr).unwrap_or(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_figure() {
        let text = b"\
GFXSHIP = 0
Nummer: HANDEL1
  Gfx: GFXSHIP
  Blocknr: 2
  Rotate: 1
  Objekt: ANIM
    Nummer: 0
    Kind: ENDLESS
    AnimOffs: 0
    AnimAdd: 1
    AnimAnz: 40
    AnimSpeed: 90
  EndObj;
";
        let f = FiguresFile::parse_text(std::str::from_utf8(text).unwrap());
        assert_eq!(f.figures.len(), 1);
        let fig = &f.figures[0];
        assert_eq!(fig.name, "HANDEL1");
        assert_eq!(fig.gfx, 0);
        assert_eq!(fig.blocknr, 2);
        assert_eq!(fig.rotate, 1);
        assert_eq!(fig.anims.len(), 1);
        assert_eq!(fig.anims[0].anim_anz, 40);
    }

    #[test]
    fn typed_accessors_pull_numeric_props() {
        let text = b"\
GFXSHIP = 0
Nummer: HANDEL1
  Gfx: GFXSHIP
  Blocknr: 2
  Rotate: 1
  Speed: 220
  Maxtrag: 6
  Maxware: 4
  Hitpoint: 65
  Preis: 600
  Worktime: 8
  Maxkanon: 8
  Shotradius: 24
  Objekt: ANIM
    Nummer: 0
    Kind: ENDLESS
    AnimOffs: 0
    AnimAdd: 1
    AnimAnz: 40
    AnimSpeed: 90
  EndObj;
";
        let f = FiguresFile::parse_text(std::str::from_utf8(text).unwrap());
        let h = f.find("HANDEL1").expect("HANDEL1 parsed");
        assert_eq!(h.speed(), 220);
        assert_eq!(h.max_load(), 6);
        assert_eq!(h.max_ware(), 4);
        assert_eq!(h.hit_points(), 65);
        assert_eq!(h.price(), 600);
        assert_eq!(h.worktime(), 8);
        assert_eq!(h.max_cannons(), 8);
        assert_eq!(h.shot_radius(), 24);
    }

    #[test]
    fn resolves_constants_in_gfx() {
        let text = b"\
GFXTRAEGER = 0
GFXESEL = GFXTRAEGER+192
Nummer: ESEL
  Gfx: GFXESEL
  Rotate: 8
  Blocknr: 3
  Objekt: ANIM
    Nummer: 0
    Kind: ENDLESS
    AnimAdd: 1
    AnimAnz: 8
  EndObj;
";
        let f = FiguresFile::parse_text(std::str::from_utf8(text).unwrap());
        let esel = f.find("ESEL").expect("ESEL parsed");
        assert_eq!(esel.gfx, 192);
        assert_eq!(esel.walk_sprite_count(), 64); // 8 rotations × 8 frames
    }

    #[test]
    fn objfill_base_uses_last_marked_base_figure() {
        let text = b"\
GFXSOLDAT = 0
Nummer: SOLDAT1
  BASE = Nummer
  Gfx: GFXSOLDAT
  Blocknr: 1
  Rotate: 8
  Speed: 260
  Objekt: ANIM
    Nummer: 0
    Kind: ENDLESS
    AnimOffs: 0
    AnimAdd: 1
    AnimAnz: 8
    AnimSpeed: 80
  EndObj;
Nummer: SOLDAT2
  ObjFill: BASE
  Gfx: GFXSOLDAT+280
";
        let f = FiguresFile::parse_text(std::str::from_utf8(text).unwrap());
        let soldat2 = f.find("SOLDAT2").expect("SOLDAT2 parsed");
        assert_eq!(soldat2.gfx, 280);
        assert_eq!(soldat2.blocknr, 1);
        assert_eq!(soldat2.rotate, 8);
        assert_eq!(soldat2.speed(), 260);
        assert_eq!(soldat2.walk_anim().unwrap().anim_anz, 8);
    }

    #[test]
    fn parses_real_figuren_cod() {
        let path = "../../extracted/figuren.cod";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("skipping: {} not found", path);
            return;
        };
        let f = FiguresFile::parse(&bytes);
        // Sanity: enough figures and at least the carriers/ships we know about.
        assert!(f.figures.len() >= 50, "got {} figures", f.figures.len());
        let traeger = f.find("TRAEGER").expect("TRAEGER must exist");
        assert_eq!(traeger.rotate, 8);
        assert_eq!(traeger.speed(), 220);
        assert_eq!(traeger.max_load(), 4);
        let walk = traeger.walk_anim().expect("walk anim");
        assert_eq!(walk.anim_anz, 8);
        assert_eq!(walk.anim_offs, 0);
        let loaded = traeger.anim(1).expect("loaded carrier anim");
        assert_eq!(loaded.anim_anz, 8);
        assert_eq!(loaded.anim_offs, 64);
        let handel1 = f.find("HANDEL1").expect("HANDEL1 must exist");
        assert_eq!(handel1.gfx, 0);
        assert_eq!(handel1.rotate, 1);
        assert_eq!(handel1.max_ware(), 4);
        let handel1_walk = handel1.walk_anim().unwrap();
        assert_eq!(handel1_walk.anim_offs, 0);
        assert_eq!(handel1_walk.anim_anz, 40);
    }

    #[test]
    fn ship_stats_from_figuren_cod() {
        // Pin the canonical figuren.cod ship stats so combat.rs's
        // hard-coded UnitStats can be cross-referenced. Sourced
        // directly from the encrypted figuren.cod entries.
        let path = "../../extracted/figuren.cod";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("skipping: {} not found", path);
            return;
        };
        let f = FiguresFile::parse(&bytes);
        // KRIEG1 (small warship): Hitpoint 2.0, Maxenergy 65,
        // Maxkanon 8, Maxware 3, Speed 550.
        let krieg1 = f.find("KRIEG1").expect("KRIEG1 in figuren.cod");
        assert_eq!(krieg1.gfx, 64);
        assert_eq!(krieg1.hit_points_f32(), 2.0);
        assert_eq!(krieg1.max_energy(), 65);
        assert_eq!(krieg1.max_cannons(), 8);
        assert_eq!(krieg1.max_ware(), 3);
        assert_eq!(krieg1.speed(), 550);
        assert_eq!(krieg1.walk_anim().unwrap().anim_offs, 0);
        assert_eq!(krieg1.walk_anim().unwrap().anim_anz, 40);
        // HANDEL2 / HANDLER are the 60-ton trader hulls.
        let handel2 = f.find("HANDEL2").expect("HANDEL2 in figuren.cod");
        assert_eq!(handel2.gfx, 32);
        assert_eq!(handel2.max_ware(), 6);
        assert_eq!(handel2.walk_anim().unwrap().anim_offs, 0);
        assert_eq!(handel2.walk_anim().unwrap().anim_anz, 40);
        let handler = f.find("HANDLER").expect("HANDLER in figuren.cod");
        assert_eq!(handler.gfx, 16);
        assert_eq!(handler.max_ware(), 6);
        assert_eq!(handler.max_cannons(), 12);
        assert_eq!(handler.walk_anim().unwrap().anim_offs, 0);
        assert_eq!(handler.walk_anim().unwrap().anim_anz, 40);
        // KRIEG2 (large warship): Maxkanon 14, Maxware 8, Speed 600 — hp via
        // Maxenergy is 120, matching combat.rs's /40 scale to 3.0.
        let krieg2 = f.find("KRIEG2").expect("KRIEG2 in figuren.cod");
        assert_eq!(krieg2.gfx, 48);
        assert_eq!(krieg2.max_energy(), 120);
        assert_eq!(krieg2.max_cannons(), 14);
        assert_eq!(krieg2.max_ware(), 8);
        assert_eq!(krieg2.walk_anim().unwrap().anim_offs, 0);
        assert_eq!(krieg2.walk_anim().unwrap().anim_anz, 40);
        // PIRAT: 10 cannon and 5 cargo slots.
        let pirat = f.find("PIRAT").expect("PIRAT in figuren.cod");
        assert_eq!(pirat.gfx, 80);
        assert_eq!(pirat.max_cannons(), 10);
        assert_eq!(pirat.max_ware(), 5);
        assert_eq!(pirat.walk_anim().unwrap().anim_offs, 0);
        assert_eq!(pirat.walk_anim().unwrap().anim_anz, 32);
    }

    #[test]
    fn land_soldier_variants_from_figuren_cod() {
        let path = "../../extracted/figuren.cod";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("skipping: {} not found", path);
            return;
        };
        let f = FiguresFile::parse(&bytes);

        let families = [
            ("SOLDAT", [0, 280, 560, 840], 80),
            ("KAVALERIE", [1120, 1424, 1728, 2032], 75),
            ("KANONIER", [2336, 2552, 2768, 2984], 95),
            ("MUSKETIER", [3200, 3336, 3472, 3608], 100),
        ];
        for (family, bases, anim_speed) in families {
            for (idx, base) in bases.into_iter().enumerate() {
                let name = format!("{}{}", family, idx + 1);
                let def = f.find(&name).unwrap_or_else(|| panic!("{name} in figuren.cod"));
                assert_eq!(def.gfx, base, "{name} gfx");
                assert_eq!(def.rotate, 8, "{name} rotate");
                assert_eq!(def.walk_anim().unwrap().anim_offs, 0, "{name} walk offset");
                assert_eq!(def.walk_anim().unwrap().anim_anz, 8, "{name} walk frames");
                assert_eq!(
                    def.walk_anim().unwrap().anim_speed,
                    anim_speed,
                    "{name} walk speed",
                );
            }
        }
    }
}
