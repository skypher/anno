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
        self.anims.iter().find(|a| a.nummer == 0)
    }

    /// Total sprites consumed by all rotations × walk frames, useful for
    /// laying out the BSH region: `gfx .. gfx + rotate * walk_anz`.
    pub fn walk_sprite_count(&self) -> i32 {
        let walk = self.walk_anim().map(|a| a.anim_anz).unwrap_or(0);
        self.rotate.max(1) * walk.max(1)
    }
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
                if let Some(src) = figures.iter().find(|f| f.name == template).cloned() {
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
        let walk = traeger.walk_anim().expect("walk anim");
        assert_eq!(walk.anim_anz, 8);
        let handel1 = f.find("HANDEL1").expect("HANDEL1 must exist");
        assert_eq!(handel1.gfx, 0);
        assert_eq!(handel1.rotate, 1);
        assert_eq!(handel1.walk_anim().unwrap().anim_anz, 40);
    }
}
