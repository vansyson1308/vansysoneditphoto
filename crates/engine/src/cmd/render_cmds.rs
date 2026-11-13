//! Filter › Render › Flame…, Picture Frame… and Tree…: procedural renderers
//! (`magicpic_algo::render2`). They draw onto the active pixel layer (or a
//! new layer with `newLayer: true`, or when the active layer has no pixels),
//! inside the selection, in any colour mode and depth, as one undo step.
//! Results report what was drawn (`layer`, `bounds`, `primitives`) so agents
//! can verify them without screenshots.
//!
//! Starfield, Starburst and Bokeh are MagicPic's own (no Photoshop counterpart, so no menu item):
//! the dreamy-photo elements agents otherwise build from noise, blurs, levels and gradients. They
//! draw on a new Screen layer by default, seeded, so the same call gives the same picture.

use magicpic_algo::render2::{self, Blend, FlameSpec, FrameSpec, Prim, TreeSpec};
use magicpic_doc::{Layer, LayerContent};
use magicpic_geom::Rect;
use magicpic_raster::Surface;
use serde_json::{Map, Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// `[r, g, b(, a)]` (0–1) or `#rrggbb` → `[r, g, b, a]`.
fn parse_colour(v: &Value) -> Option<[f32; 4]> {
    match v {
        Value::Array(a) if a.len() >= 3 => {
            let c = |j: usize, d: f64| a.get(j).and_then(Value::as_f64).unwrap_or(d) as f32;
            Some([c(0, 0.0), c(1, 0.0), c(2, 0.0), c(3, 1.0)])
        }
        Value::String(h) if h.len() == 7 && h.starts_with('#') => {
            let ch = |j: usize| h.get(j..j + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map_or(0.0, |v| v as f32 / 255.0);
            Some([ch(1), ch(3), ch(5), 1.0])
        }
        _ => None,
    }
}

/// Params as an object with the named colour keys normalized to arrays (invalid ones dropped).
fn normalized(p: &Value, colours: &[&str]) -> Map<String, Value> {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.remove("layer");
    m.remove("newLayer");
    m.remove("path");
    for k in colours {
        if let Some(v) = m.remove(*k)
            && let Some(c) = parse_colour(&v)
        {
            m.insert((*k).to_string(), json!(c));
        }
    }
    m
}

/// Integer fields of a spec (`u32` in [`FlameSpec`], [`TreeSpec`], [`FrameSpec`]) given as JSON
/// floats: dialogs send every number as a float (`1.0`) and agents may too, which serde refuses
/// for a `u32` ("invalid type: floating point `1.0`, expected u32", #2501). Finite floats round to
/// the nearest integer, like [`crate::commands::int`]; a negative or too-large result is then
/// still refused by `spec_from`. Strings (names) and other types are left alone.
fn whole_numbers(m: &mut Map<String, Value>, keys: &[&str]) {
    for k in keys {
        if let Some(Value::Number(n)) = m.get(*k)
            && !(n.is_u64() || n.is_i64())
            && let Some(f) = n.as_f64().filter(|f| f.is_finite())
        {
            m.insert((*k).to_string(), json!(f.round() as i64));
        }
    }
}

fn spec_from<T: serde::de::DeserializeOwned>(m: Map<String, Value>) -> Result<T> {
    serde_json::from_value(Value::Object(m)).map_err(|e| EngineError::Other(format!("invalid parameters: {e}")))
}

/// Accepts a 1-based number or a name (camelCase) for an enumerated option.
fn index_param(m: &mut Map<String, Value>, k: &str, names: &[&str]) {
    if let Some(Value::String(n)) = m.get(k) {
        let lower = n.to_ascii_lowercase().replace([' ', '-', '_'], "");
        let i = names.iter().position(|s| s.to_ascii_lowercase().replace([' ', '-', '_'], "") == lower).map_or(1, |i| i + 1);
        m.insert(k.to_string(), json!(i));
    }
}

/// Flattened polylines of the path Flame follows: `path` (a saved path name, `"work"`), else the
/// work path, else the first saved path. `None` when the document has no path.
fn flame_paths(s: &Session, p: &Value) -> Option<Vec<Vec<(f32, f32)>>> {
    let d = s.active()?;
    let doc = &d.doc;
    let path = match p.get("path").and_then(Value::as_str) {
        Some(n) if !n.is_empty() && !n.eq_ignore_ascii_case("work") => doc.paths.iter().find(|sp| sp.name == n).map(|sp| &sp.path),
        _ => doc.work_path.as_ref().or_else(|| doc.paths.first().map(|sp| &sp.path)),
    }?;
    let polys: Vec<Vec<(f32, f32)>> = magicpic_vector::flatten_path(path, 0.5)
        .into_iter()
        .map(|pl| {
            let mut v: Vec<(f32, f32)> = pl.pts.iter().map(|&(x, y)| (x as f32, y as f32)).collect();
            if pl.closed
                && let Some(&f) = v.first()
            {
                v.push(f);
            }
            v
        })
        .filter(|v| !v.is_empty())
        .collect();
    (!polys.is_empty()).then_some(polys)
}

/// Draws `prims` onto the target layer as one undo step. A layer created for them gets `blend`.
fn draw(s: &mut Session, label: &str, p: &Value, prims: Vec<Prim>, extra: Value) -> Result<Value> {
    draw_with(s, label, p, prims, extra, None)
}

fn draw_with(s: &mut Session, label: &str, p: &Value, prims: Vec<Prim>, extra: Value, blend: Option<magicpic_color::BlendMode>) -> Result<Value> {
    let want_new = p.get("newLayer").and_then(Value::as_bool).unwrap_or(false);
    let target = p.get("layer").map(|_| crate::commands::layer_param(s, p)).transpose()?;
    let count = prims.len();
    let (layer, created, used) = s.edit(label, |doc, active| {
        let id = target.or(*active);
        let raster = id.and_then(|i| doc.layer(i)).is_some_and(|l| matches!(l.content, LayerContent::Raster(_)));
        let (id, created) = if want_new || !raster {
            let mut l = Layer::new(label.trim_end_matches('…'), LayerContent::Raster(Surface::new(doc.pixel_format())));
            if let Some(mode) = blend {
                l.blend = mode;
            }
            let nid = crate::layer_place::insert_new(doc, *active, l, crate::layer_place::Place::AutoEditable);
            *active = Some(nid);
            (nid, true)
        } else {
            (id.ok_or(EngineError::Other("no active layer".into()))?, false)
        };
        let selection = doc.selection.clone();
        let mut clip = doc.bounds();
        if let Some(sel) = &selection {
            clip = clip.intersect(&sel.content_bounds());
        }
        let surf = crate::commands::paint_surface(doc, id, &Value::Null)?;
        let used = render2::composite(surf, &prims, clip, selection.as_ref());
        surf.prune();
        Ok((id, created, used))
    })?;
    let mut r = json!({
        "layer": layer.0,
        "newLayer": created,
        "bounds": if used.is_empty() { Value::Null } else { json!([used.x0, used.y0, used.x1, used.y1]) },
        "primitives": count,
    });
    if let (Value::Object(m), Value::Object(e)) = (&mut r, extra) {
        m.extend(e);
    }
    Ok(r)
}

fn canvas(s: &Session) -> Result<Rect> {
    Ok(s.active().ok_or(EngineError::NoDocument)?.doc.bounds())
}

fn run_flame(s: &mut Session, p: &Value) -> Result<Value> {
    let mut m = normalized(p, &["color"]);
    if !m.get("useCustomColor").and_then(Value::as_bool).unwrap_or(m.contains_key("color")) {
        m.remove("color");
    }
    m.remove("useCustomColor");
    if let Some(Value::Number(q)) = m.get("quality").cloned() {
        m.insert("quality".into(), json!(q.as_f64().unwrap_or(2.0).clamp(0.0, 4.0) as u32));
    } else if let Some(Value::String(q)) = m.get("quality").cloned() {
        let i = ["draft", "low", "medium", "high", "fine"].iter().position(|n| *n == q).unwrap_or(2);
        m.insert("quality".into(), json!(i));
    }
    whole_numbers(&mut m, &["seed"]);
    let spec: FlameSpec = spec_from(m)?;
    let found = flame_paths(s, p);
    let used_path = found.is_some();
    let paths = found.unwrap_or_else(|| {
        let c = canvas(s).unwrap_or(Rect::new(0, 0, 1, 1));
        if spec.flame_type == render2::FlameType::CandleLight {
            vec![vec![(c.x0 as f32 + c.width() as f32 * 0.5, c.y0 as f32 + c.height() as f32 * 0.75)]]
        } else {
            vec![render2::default_flame_path(c)]
        }
    });
    let prims = render2::flame(&spec, &paths);
    draw(s, "Flame…", p, prims, json!({ "usedPath": used_path }))
}

fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let mut m = normalized(p, &["leavesColor", "branchesColor"]);
    index_param(&mut m, "baseTreeType", &render2::tree_type_names());
    if m.contains_key("leavesColor") && !m.contains_key("defaultLeaves") {
        m.insert("defaultLeaves".into(), json!(false));
    }
    if m.contains_key("branchesColor") && !m.contains_key("customBranchColor") {
        m.insert("customBranchColor".into(), json!(true));
    }
    whole_numbers(&mut m, &["baseTreeType", "lightDirection", "seed"]);
    let spec: TreeSpec = spec_from(m)?;
    let prims = render2::tree(&spec, canvas(s)?);
    let name = render2::tree_type_names()[(spec.base_tree_type.clamp(1, 34) - 1) as usize];
    draw(s, "Tree…", p, prims, json!({ "treeType": name }))
}

fn run_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let mut m = normalized(p, &["vineColor", "flowerColor", "leafColor"]);
    index_param(&mut m, "frame", &render2::FRAME_STYLES);
    whole_numbers(&mut m, &["frame", "lines", "seed"]);
    let spec: FrameSpec = spec_from(m)?;
    let prims = render2::picture_frame(&spec, canvas(s)?);
    let name = render2::FRAME_STYLES[(spec.frame.clamp(1, render2::FRAME_STYLES.len() as u32) - 1) as usize];
    draw(s, "Picture Frame…", p, prims, json!({ "frame": name }))
}

// ---------------------------------------------------------------------------
// Starfield, Starburst, Bokeh
// ---------------------------------------------------------------------------

/// Most primitives one call draws: each star or disc is one to three.
const MAX_ELEMENTS: usize = 50_000;
/// Most `points` a Starburst takes.
const MAX_POINTS: usize = 1_000;
const MAX_COLOURS: usize = 64;

/// A small seeded generator (SplitMix64): the same seed gives the same picture everywhere.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// Only these keys; anything else is a typo the caller should hear about.
fn only(cmd: &str, p: &Value, keys: &[&str]) -> Result<()> {
    if let Some(k) = p.as_object().and_then(|o| o.keys().find(|k| !keys.contains(&k.as_str()))) {
        return Err(EngineError::Other(format!("{cmd}: unknown parameter `{k}` (takes {})", keys.join(", "))));
    }
    if !p.is_object() && !p.is_null() {
        return Err(EngineError::Other(format!("{cmd}: params must be an object")));
    }
    Ok(())
}

/// A number in `lo..=hi`, or `default` when absent.
fn number(cmd: &str, p: &Value, key: &str, default: f32, lo: f32, hi: f32) -> Result<f32> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .map(|n| n as f32)
            .filter(|n| n.is_finite() && (lo..=hi).contains(n))
            .ok_or_else(|| EngineError::Other(format!("{cmd}: `{key}` must be a number from {lo} to {hi}, got {v}"))),
    }
}

/// `[min, max]` within `lo..=hi` (either order), or `default`.
fn span(cmd: &str, p: &Value, key: &str, default: (f32, f32), lo: f32, hi: f32) -> Result<(f32, f32)> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Array(a)) if a.len() == 2 => {
            let get = |i: usize| a.get(i).and_then(Value::as_f64).map(|n| n as f32).filter(|n| n.is_finite() && (lo..=hi).contains(n));
            match (get(0), get(1)) {
                (Some(x), Some(y)) => Ok((x.min(y), x.max(y))),
                _ => Err(EngineError::Other(format!("{cmd}: `{key}` must be [min, max] with numbers from {lo} to {hi}"))),
            }
        }
        Some(v) => Err(EngineError::Other(format!("{cmd}: `{key}` must be [min, max], got {v}"))),
    }
}

fn seed(cmd: &str, p: &Value) -> Result<u64> {
    match p.get("seed") {
        None | Some(Value::Null) => Ok(0),
        Some(v) => v
            .as_u64()
            .filter(|s| *s <= u64::from(u32::MAX))
            .ok_or_else(|| EngineError::Other(format!("{cmd}: `seed` must be a whole number from 0 to 4294967295"))),
    }
}

fn colour_param(cmd: &str, p: &Value, key: &str) -> Result<Option<[f32; 4]>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => parse_colour(v)
            .filter(|c| c.iter().all(|x| x.is_finite()))
            .map(|c| Some(c.map(|x| x.clamp(0.0, 1.0))))
            .ok_or_else(|| EngineError::Other(format!("{cmd}: `{key}` must be \"#rrggbb\" or [r, g, b] from 0 to 1, got {v}"))),
    }
}

/// `region` `{x, y, width, height}` within the canvas, or the canvas.
fn region(cmd: &str, s: &Session, p: &Value) -> Result<Rect> {
    let c = canvas(s)?;
    let Some(v) = p.get("region").filter(|v| !v.is_null()) else { return Ok(c) };
    let field = |k: &str| v.get(k).and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok());
    let (Some(x), Some(y), Some(w), Some(h)) = (field("x"), field("y"), field("width"), field("height")) else {
        return Err(EngineError::Other(format!("{cmd}: `region` needs whole `x`, `y`, `width` and `height` in pixels")));
    };
    let r = Rect::new(x, y, x.saturating_add(w.max(0)), y.saturating_add(h.max(0))).intersect(&c);
    if r.is_empty() { Err(EngineError::Other(format!("{cmd}: `region` lies outside the canvas"))) } else { Ok(r) }
}

fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], (c[3] * a).clamp(0.0, 1.0)]
}

/// The params `draw` reads, with `newLayer` on unless the caller turned it off.
fn on_new_layer(p: &Value) -> Value {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.entry("newLayer").or_insert(json!(true));
    Value::Object(m)
}

/// `filter.render.starfield`: many small stars of varied size and brightness.
fn run_starfield(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "filter.render.starfield";
    only(CMD, p, &["density", "sizeRange", "seed", "color", "brightness", "region", "newLayer", "layer"])?;
    let density = number(CMD, p, "density", 600.0, 0.0, 20_000.0)?;
    let (r0, r1) = span(CMD, p, "sizeRange", (0.4, 2.2), 0.1, 50.0)?;
    let brightness = number(CMD, p, "brightness", 1.0, 0.0, 2.0)?;
    let colour = colour_param(CMD, p, "color")?;
    let seed = seed(CMD, p)?;
    let area = region(CMD, s, p)?;
    let pixels = f64::from(area.width()) * f64::from(area.height());
    let count = ((f64::from(density) * pixels / 1e6).round() as usize).min(MAX_ELEMENTS / 2);
    let mut rng = Rng::new(seed);
    let mut prims = Vec::with_capacity(count * 2);
    for _ in 0..count {
        let x = rng.between(area.x0 as f32, area.x1 as f32);
        let y = rng.between(area.y0 as f32, area.y1 as f32);
        // Many faint small stars, few big bright ones, as in a real sky.
        let t = rng.unit().powi(3);
        let r = r0 + (r1 - r0) * t;
        let light = ((0.25 + 0.75 * rng.unit()) * (0.6 + 0.4 * t) * brightness).min(1.0);
        let tint = colour.unwrap_or_else(|| {
            let k = rng.unit();
            if k < 0.15 {
                [0.75, 0.85, 1.0, 1.0]
            } else if k < 0.25 {
                [1.0, 0.9, 0.75, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            }
        });
        // A star smaller than a pixel lights its pixel by its area instead.
        let (radius, alpha) = if r < 1.0 { (1.0, light * r) } else { (r, light) };
        let mut core = Prim::ellipse((x, y), radius, radius, 0.0, with_alpha(tint, alpha), with_alpha(tint, alpha * 0.15));
        core.soft = (radius * 0.6).max(1.0);
        core.blend = Blend::Add;
        prims.push(core);
        if r > 1.4 {
            let mut glow = Prim::ellipse((x, y), r * 3.5, r * 3.5, 0.0, with_alpha(tint, alpha * 0.18), with_alpha(tint, 0.0));
            glow.soft = r * 2.0;
            glow.blend = Blend::Add;
            prims.push(glow);
        }
    }
    let p = on_new_layer(p);
    draw_with(s, "Starfield", &p, prims, json!({"stars": count, "seed": seed}), Some(magicpic_color::BlendMode::Screen))
}

/// One Starburst point: `{x, y, size?, rayLength?, angle?, color?}`.
struct Burst {
    x: f32,
    y: f32,
    size: f32,
    ray: Option<f32>,
    angle: Option<f32>,
    colour: Option<[f32; 4]>,
}

fn bursts(cmd: &str, p: &Value) -> Result<Vec<Burst>> {
    let Some(points) = p.get("points").and_then(Value::as_array).filter(|a| !a.is_empty()) else {
        return Err(EngineError::Other(format!("{cmd}: `points` must be a non-empty list of {{\"x\", \"y\", \"size\"}} in pixels")));
    };
    if points.len() > MAX_POINTS {
        return Err(EngineError::Other(format!("{cmd}: at most {MAX_POINTS} points, got {}", points.len())));
    }
    points
        .iter()
        .enumerate()
        .map(|(i, pt)| {
            let at = |k: &str| pt.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite() && v.abs() <= 1e6);
            let (Some(x), Some(y)) = (at("x"), at("y")) else {
                return Err(EngineError::Other(format!("{cmd}: point {i} needs numeric `x` and `y` in pixels")));
            };
            Ok(Burst {
                x,
                y,
                size: number(cmd, pt, "size", 4.0, 0.5, 500.0)?,
                ray: pt.get("rayLength").map(|_| number(cmd, pt, "rayLength", 30.0, 1.0, 5_000.0)).transpose()?,
                angle: pt.get("angle").map(|_| number(cmd, pt, "angle", 0.0, -360.0, 360.0)).transpose()?,
                colour: colour_param(cmd, pt, "color")?,
            })
        })
        .collect()
}

/// `filter.render.starburst`: twinkling stars with soft, tapering rays.
fn run_starburst(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "filter.render.starburst";
    only(CMD, p, &["points", "rays", "rayLength", "taper", "color", "glow", "angle", "newLayer", "layer"])?;
    let points = bursts(CMD, p)?;
    let rays = match p.get("rays") {
        None | Some(Value::Null) => 4,
        Some(v) => match v.as_u64() {
            Some(n @ (4 | 6 | 8)) => n as usize,
            _ => return Err(EngineError::Other(format!("{CMD}: `rays` must be 4, 6 or 8, got {v}"))),
        },
    };
    let ray_length = p.get("rayLength").map(|_| number(CMD, p, "rayLength", 30.0, 1.0, 5_000.0)).transpose()?;
    let taper = number(CMD, p, "taper", 0.85, 0.0, 1.0)?;
    let glow = number(CMD, p, "glow", 0.5, 0.0, 1.0)?;
    let angle = number(CMD, p, "angle", 0.0, -360.0, 360.0)?;
    let colour = colour_param(CMD, p, "color")?.unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let mut prims = Vec::new();
    for b in &points {
        let col = b.colour.unwrap_or(colour);
        let length = b.ray.or(ray_length).unwrap_or(b.size * 8.0);
        let rotation = b.angle.unwrap_or(angle).to_radians();
        if glow > 0.0 {
            let mut halo = Prim::ellipse((b.x, b.y), b.size * 4.0, b.size * 4.0, 0.0, with_alpha(col, 0.55 * glow), with_alpha(col, 0.0));
            halo.soft = b.size * 2.0;
            halo.blend = Blend::Add;
            prims.push(halo);
        }
        for k in 0..rays {
            // With 8 rays every other one is a shorter, fainter diagonal.
            let minor = rays == 8 && k % 2 == 1;
            let a = rotation + std::f32::consts::TAU * k as f32 / rays as f32;
            let len = if minor { length * 0.55 } else { length };
            let tip = (b.x + a.cos() * len, b.y + a.sin() * len);
            let base = (b.size * 0.45).max(0.6);
            // Narrowing and fading to nothing: no hard "plus sign" ends.
            let end = (base * (1.0 - taper)).max(0.3);
            let mut ray = Prim::capsule((b.x, b.y), tip, base, end, with_alpha(col, if minor { 0.6 } else { 0.9 }), with_alpha(col, 0.0));
            ray.soft = (base * 0.8).max(1.0);
            ray.blend = Blend::Add;
            prims.push(ray);
        }
        let white = [1.0, 1.0, 1.0, 1.0];
        let mut core = Prim::ellipse((b.x, b.y), b.size, b.size, 0.0, white, with_alpha(col, 0.6));
        core.soft = (b.size * 0.5).max(1.0);
        core.blend = Blend::Add;
        prims.push(core);
    }
    let p = on_new_layer(p);
    draw_with(s, "Starburst", &p, prims, json!({"points": points.len(), "rays": rays}), Some(magicpic_color::BlendMode::Screen))
}

/// `filter.render.bokeh`: soft out-of-focus discs of light.
fn run_bokeh(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "filter.render.bokeh";
    only(CMD, p, &["count", "sizeRange", "softness", "colors", "opacity", "seed", "region", "newLayer", "layer"])?;
    let area = region(CMD, s, p)?;
    let short = area.width().min(area.height()) as f32;
    let count = number(CMD, p, "count", 40.0, 0.0, 5_000.0)?.round() as usize;
    // Proportional to the image, with no pixel floor: the Filter dialog previews on a reduced
    // proxy, and a floor there would draw relatively bigger discs than OK applies.
    let (r0, r1) = span(CMD, p, "sizeRange", ((short * 0.015).max(0.25), (short * 0.06).max(0.5)), 1.0, 2_000.0)?;
    let softness = number(CMD, p, "softness", 0.3, 0.0, 1.0)?;
    let opacity = number(CMD, p, "opacity", 0.35, 0.0, 1.0)?;
    let seed = seed(CMD, p)?;
    let colours: Vec<[f32; 4]> = match p.get("colors") {
        None | Some(Value::Null) => vec![[1.0, 0.84, 0.63, 1.0], [0.63, 0.78, 1.0, 1.0], [1.0, 0.69, 0.88, 1.0]],
        Some(Value::Array(a)) if !a.is_empty() && a.len() <= MAX_COLOURS => a
            .iter()
            .map(|v| parse_colour(v).filter(|c| c.iter().all(|x| x.is_finite())).map(|c| c.map(|x| x.clamp(0.0, 1.0))))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| EngineError::Other(format!("{CMD}: every `colors` entry must be \"#rrggbb\" or [r, g, b]")))?,
        Some(_) => return Err(EngineError::Other(format!("{CMD}: `colors` must be a list of 1 to {MAX_COLOURS} colours"))),
    };
    let mut rng = Rng::new(seed);
    let mut prims = Vec::with_capacity(count);
    for _ in 0..count {
        let x = rng.between(area.x0 as f32, area.x1 as f32);
        let y = rng.between(area.y0 as f32, area.y1 as f32);
        let r = rng.between(r0, r1);
        let pick = (rng.unit() * colours.len() as f32) as usize;
        let col = colours.get(pick.min(colours.len().saturating_sub(1))).copied().unwrap_or([1.0; 4]);
        let a = opacity * rng.between(0.6, 1.0);
        // Out-of-focus highlights are flat with a slightly brighter rim.
        let mut disc = Prim::ellipse((x, y), r, r, 0.0, with_alpha(col, a * 0.75), with_alpha(col, a));
        disc.soft = (r * softness).max(1.0);
        disc.blend = Blend::Add;
        prims.push(disc);
    }
    let p = on_new_layer(p);
    draw_with(s, "Bokeh", &p, prims, json!({"discs": count, "seed": seed}), Some(magicpic_color::BlendMode::Screen))
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// The Render commands of this module.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "filter.render.flame",
            label: "Flame…",
            menu: &["Filter", "Render"],
            shortcut: None,
            params: r##"{"flameType":"oneFlameAlongPath|multipleFlamesAlongPath|multipleFlamesPathDirections|multipleFlamesVariousLength|candleLight|multipleFlamesOneDirection","length":1..1000=150,"randomizeLength":bool,"width":1..1000=40,"angle":-180..180=0,"interval":1..1000=60,"adjustIntervalForLoops":bool=true,"useCustomColor":bool,"color":color,"turbulent":0..100=25,"jag":0..100=25,"opacity":0..100=75,"flameLines":1..100=20,"flameBottomAlignment":0..100=20,"flameStyle":"normal|violent|flat","flameShape":"parallel|toCenter|spread|oval|pointed","randomizeShapes":bool,"seed":u32=0,"quality":"draft|low|medium|high|fine","path":text,"newLayer":bool} → {layer,newLayer,bounds,primitives,usedPath}"##,
            enabled: has_doc,
            run: run_flame,
            journal: true,
        },
        CommandSpec {
            id: "filter.render.pictureFrame",
            label: "Picture Frame…",
            menu: &["Filter", "Render"],
            shortcut: None,
            params: r##"{"frame":"vineWithFlowers|vineWithLeaves|ivy|roses|daisies|berries|bamboo|waves|zigzag|dots|rope|scallops|doubleLine|snowflakes|stars|hearts","margin":0..30=4,"size":1..100=50,"arrangement":1..100=50,"lines":1..5=1,"thickness":1..100=30,"fade":0..100=0,"vineColor":color,"flowerColor":color,"leafColor":color,"seed":u32=0,"newLayer":bool} → {layer,newLayer,bounds,primitives,frame}"##,
            enabled: has_doc,
            run: run_frame,
            journal: true,
        },
        CommandSpec {
            id: "filter.render.tree",
            label: "Tree…",
            menu: &["Filter", "Render"],
            shortcut: None,
            params: r##"{"baseTreeType":1..34=1,"lightDirection":1..5=3,"leavesAmount":0..100=50,"leavesSize":0..200=100,"branchesHeight":50..300=100,"branchesThickness":50..200=100,"defaultLeaves":bool=true,"leavesColor":color,"customBranchColor":bool,"branchesColor":color,"flatShading":bool,"seed":u32=0,"x":0..1=0.5,"y":0..1=0.95,"size":0.05..2=0.8,"newLayer":bool} → {layer,newLayer,bounds,primitives,treeType}"##,
            enabled: has_doc,
            run: run_tree,
            journal: true,
        },
        CommandSpec {
            id: "filter.render.starfield",
            label: "Starfield",
            menu: &[],
            shortcut: None,
            params: r##"{"density":0..20000=600 (stars per megapixel),"sizeRange":[min,max]=[0.4,2.2] (radius px, 0.1..50),"brightness":0..2=1,"color":color? (default: white with a few blue and warm stars),"seed":u32=0,"region":{"x","y","width","height"}?,"newLayer":bool=true} → {layer,newLayer,bounds,primitives,stars,seed} (draws on a new Screen layer unless newLayer is false; many faint small stars, few bright ones, the bigger ones with a glow)"##,
            enabled: has_doc,
            run: run_starfield,
            journal: true,
        },
        CommandSpec {
            id: "filter.render.starburst",
            label: "Starburst",
            menu: &[],
            shortcut: None,
            params: r##"{"points":[{"x":px,"y":px,"size":0.5..500=4,"rayLength":px?,"angle":deg?,"color":color?}] (1..1000),"rays":4|6|8=4,"rayLength":1..5000=8×size,"taper":0..1=0.85 (how far the rays narrow),"glow":0..1=0.5,"angle":-360..360=0,"color":color="#ffffff","newLayer":bool=true} → {layer,newLayer,bounds,primitives,points,rays} (twinkle stars whose rays narrow and fade to nothing instead of ending in a hard plus sign; with 8 rays the diagonals are shorter; a new Screen layer unless newLayer is false)"##,
            enabled: has_doc,
            run: run_starburst,
            journal: true,
        },
        CommandSpec {
            id: "filter.render.bokeh",
            label: "Bokeh",
            menu: &[],
            shortcut: None,
            params: r##"{"count":0..5000=40,"sizeRange":[min,max] (radius px, 1..2000; default 1.5% to 6% of the short side),"softness":0..1=0.3,"colors":[color,…]=warm, blue and pink (1..64),"opacity":0..1=0.35,"seed":u32=0,"region":{"x","y","width","height"}?,"newLayer":bool=true} → {layer,newLayer,bounds,primitives,discs,seed} (soft out-of-focus discs with a slightly brighter rim, on a new Screen layer unless newLayer is false)"##,
            enabled: has_doc,
            run: run_bokeh,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(depth: u64) -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 160, "height": 120, "depth": depth})).unwrap();
        assert_eq!(Some(s.active().unwrap().doc.depth), crate::document_preset_cmds::sample_type(depth));
        s.execute("layer.new.layer", json!({})).unwrap();
        s
    }

    fn pixels(s: &Session) -> Vec<f32> {
        let d = s.active().unwrap();
        d.doc.layer(d.active_layer.unwrap()).unwrap().surface().unwrap().read_region(Rect::new(0, 0, 160, 120))
    }

    fn opaque(s: &Session) -> usize {
        pixels(s).as_chunks::<4>().0.iter().filter(|p| p[3] > 0.05).count()
    }

    #[test]
    fn each_renderer_draws_undoes_and_is_seeded_at_every_depth() {
        for depth in [8, 16, 32] {
            for (id, p) in [
                ("filter.render.flame", json!({})),
                ("filter.render.tree", json!({"baseTreeType": 7})),
                ("filter.render.pictureFrame", json!({"frame": "roses"})),
            ] {
                let mut s = session(depth);
                let r = s.execute(id, p.clone()).unwrap_or_else(|e| panic!("{id}@{depth}: {e}"));
                assert!(r["primitives"].as_u64().unwrap() > 0, "{id}");
                assert!(r["bounds"].is_array(), "{id}@{depth}: {r}");
                let n = opaque(&s);
                assert!(n > 50, "{id}@{depth} drew {n} px");
                let first = pixels(&s);
                s.execute("edit.undo", json!({})).unwrap();
                assert_eq!(opaque(&s), 0, "{id} undo");
                s.execute(id, p.clone()).unwrap();
                assert_eq!(pixels(&s), first, "{id} deterministic");
                s.execute("edit.undo", json!({})).unwrap();
                let mut q = p.clone();
                q["seed"] = json!(5);
                s.execute(id, q).unwrap();
                assert_ne!(pixels(&s), first, "{id} seed changes the result");
            }
        }
    }

    #[test]
    fn flame_follows_the_work_path() {
        let mut s = session(8);
        let r = s.execute("filter.render.flame", json!({})).unwrap();
        assert_eq!(r["usedPath"], json!(false));
        s.execute("edit.undo", json!({})).unwrap();
        // A vertical line path on the left edge: the flame stays near it.
        s.edit("path", |doc, _| {
            let mut sub = magicpic_doc::Subpath::default();
            for (x, y) in [(20.0, 110.0), (20.0, 20.0)] {
                sub.knots.push(magicpic_doc::Knot::corner(x, y));
            }
            doc.work_path = Some(magicpic_doc::Path { subpaths: vec![sub], ..Default::default() });
            Ok(())
        })
        .unwrap();
        let r = s.execute("filter.render.flame", json!({"width": 16, "length": 60})).unwrap();
        assert_eq!(r["usedPath"], json!(true));
        let b: Vec<i64> = r["bounds"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        assert!(b[0] < 20 && b[2] < 60, "flame bounds {b:?} hug the path at x=20");
    }

    #[test]
    fn selection_limits_and_new_layer() {
        let mut s = session(8);
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 80, "height": 120})).unwrap();
        let r = s.execute("filter.render.tree", json!({"newLayer": true, "baseTreeType": 1, "leavesAmount": 100})).unwrap();
        assert_eq!(r["newLayer"], json!(true));
        let px = pixels(&s);
        let right = px.as_chunks::<4>().0.iter().enumerate().filter(|(i, p)| i % 160 >= 81 && p[3] > 0.0).count();
        assert_eq!(right, 0, "nothing drawn outside the selection");
        assert!(opaque(&s) > 50);
    }

    #[test]
    fn works_in_cmyk_and_gray() {
        for mode in ["cmyk", "grayscale", "lab"] {
            let mut s = Session::new();
            s.execute("file.new", json!({"width": 96, "height": 96, "mode": mode})).unwrap();
            let r = s.execute("filter.render.pictureFrame", json!({"frame": 13, "newLayer": true})).unwrap_or_else(|e| panic!("{mode}: {e}"));
            assert!(r["bounds"].is_array(), "{mode}");
        }
    }

    /// #2501: the Tree and Picture Frame dialogs send every number as a JSON float, and their
    /// `u32` fields (`baseTreeType`, `lightDirection`, `frame`, `lines`, `seed`) refused `1.0`
    /// ("invalid type: floating point `1.0`, expected u32"). Whole floats now draw exactly what
    /// the integers draw, fractional ones (a dragged 1–5 slider) round, and hostile values fail.
    #[test]
    fn integer_params_accept_whole_floats_and_round_fractions() {
        let tree = json!({"baseTreeType": 1.0, "lightDirection": 3.0, "leavesAmount": 50.0, "leavesSize": 100.0, "branchesHeight": 100.0,
            "branchesThickness": 100.0, "defaultLeaves": true, "customBranchColor": false, "flatShading": false, "seed": 0, "x": 0.5, "y": 0.95, "size": 0.8});
        let frame =
            json!({"frame": "vineWithFlowers", "margin": 4.0, "size": 50.0, "arrangement": 50.0, "lines": 1.0, "thickness": 30.0, "fade": 0.0, "seed": 0});
        for (id, floats, ints) in [
            ("filter.render.tree", tree.clone(), json!({"baseTreeType": 1, "lightDirection": 3})),
            (
                "filter.render.tree",
                json!({"baseTreeType": 7.4, "lightDirection": 2.6, "seed": 3.0}),
                json!({"baseTreeType": 7, "lightDirection": 3, "seed": 3}),
            ),
            ("filter.render.pictureFrame", frame.clone(), json!({"frame": 1, "lines": 1})),
            ("filter.render.pictureFrame", json!({"frame": 13.0, "lines": 3.0, "seed": 2.0}), json!({"frame": 13, "lines": 3, "seed": 2})),
            ("filter.render.flame", json!({"seed": 4.0}), json!({"seed": 4})),
        ] {
            let mut s = session(8);
            let a = s.execute(id, floats.clone()).unwrap_or_else(|e| panic!("{id} {floats}: {e}"));
            let drawn = pixels(&s);
            s.execute("edit.undo", json!({})).unwrap();
            let b = s.execute(id, ints.clone()).unwrap();
            assert_eq!(a, b, "{id}: {floats} reports what {ints} does");
            assert_eq!(pixels(&s), drawn, "{id}: {floats} draws what {ints} draws");
        }
        // Negative, too large or non-numeric integer fields are refused, never a panic.
        let mut s = session(8);
        for (id, p) in [
            ("filter.render.tree", json!({"baseTreeType": -1.0})),
            ("filter.render.tree", json!({"lightDirection": 1e300})),
            ("filter.render.tree", json!({"seed": -0.6})),
            ("filter.render.pictureFrame", json!({"lines": -3.5})),
            ("filter.render.pictureFrame", json!({"seed": 5e9})),
            ("filter.render.pictureFrame", json!({"lines": "two"})),
            ("filter.render.flame", json!({"seed": f64::MAX})),
        ] {
            let e = s.execute(id, p.clone()).expect_err(&format!("{id} {p}"));
            assert!(e.to_string().contains("invalid parameters"), "{id} {p}: {e}");
        }
    }

    /// `cargo test --release -p magicpic-engine render_cmds::tests::bench_24mp -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_24mp() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 6000, "height": 4000})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        for (id, p) in [
            ("filter.render.flame", json!({"flameType": "multipleFlamesAlongPath", "length": 600, "width": 160, "interval": 120})),
            ("filter.render.flame", json!({"length": 1500, "width": 400, "flameLines": 100, "quality": "fine"})),
            ("filter.render.tree", json!({"baseTreeType": 1, "leavesAmount": 100})),
            ("filter.render.tree", json!({"baseTreeType": 6, "leavesAmount": 100})),
            ("filter.render.pictureFrame", json!({"frame": 1})),
        ] {
            let t = std::time::Instant::now();
            let r = s.execute(id, p).unwrap();
            println!("{id}: {:?} ({} prims)", t.elapsed(), r["primitives"]);
        }
    }

    /// Starfield, Starburst and Bokeh (agent setup report, P2): a new Screen layer by default, one
    /// undo step, the same picture for the same seed, and errors for bad params.
    #[test]
    fn fantasy_renderers_draw_on_a_new_screen_layer_seeded_and_undoable() {
        let lit = |s: &Session| {
            let d = s.active().unwrap();
            let l = d.doc.layer(d.active_layer.unwrap()).unwrap();
            let px = l.surface().unwrap().read_region(Rect::new(0, 0, 160, 120));
            (l.blend, l.name.clone(), px.as_chunks::<4>().0.iter().filter(|p| p[3] > 0.02).count(), px)
        };
        let calls = [
            ("filter.render.starfield", json!({"density": 3000, "seed": 7}), "Starfield"),
            ("filter.render.starburst", json!({"points": [{"x": 40, "y": 30, "size": 3}, {"x": 120, "y": 80, "size": 5}], "rays": 8, "seed": 1}), "Starburst"),
            ("filter.render.bokeh", json!({"count": 25, "seed": 3, "colors": ["#ffd0a0", [0.6, 0.7, 1.0]]}), "Bokeh"),
        ];
        for depth in [8, 16, 32] {
            for (id, params, name) in &calls {
                let params = if *id == "filter.render.starburst" { json!({"points": params["points"], "rays": 8}) } else { params.clone() };
                let mut s = session(depth);
                let layers = s.active().unwrap().doc.layers.len();
                let r = s.execute(id, params.clone()).unwrap();
                assert_eq!(r["newLayer"], true, "{id}");
                assert_eq!(s.active().unwrap().doc.layers.len(), layers + 1, "{id}: on a new layer");
                let (blend, layer_name, count, first) = lit(&s);
                assert_eq!((blend, layer_name.as_str()), (magicpic_color::BlendMode::Screen, *name), "{id}");
                assert!(count > 20, "{id} at {depth} bits drew only {count} pixels");
                // One undo step takes the layer away again.
                s.execute("edit.undo", json!({})).unwrap();
                assert_eq!(s.active().unwrap().doc.layers.len(), layers, "{id}: one undo step");
                // Same seed, same picture.
                s.execute(id, params).unwrap();
                assert_eq!(lit(&s).3, first, "{id} is reproducible");
            }
        }
        // Starfield density scales the count; a region keeps stars inside it.
        let mut s = session(8);
        let few = s.execute("filter.render.starfield", json!({"density": 500})).unwrap()["stars"].as_u64().unwrap();
        let many = s.execute("filter.render.starfield", json!({"density": 5000})).unwrap()["stars"].as_u64().unwrap();
        assert!(many > few * 5, "{few} vs {many}");
        let r = s.execute("filter.render.bokeh", json!({"region": {"x": 0, "y": 0, "width": 40, "height": 40}, "sizeRange": [2, 4]})).unwrap();
        let b = r["bounds"].as_array().unwrap();
        assert!(b[2].as_i64().unwrap() <= 40 + 4 + 3, "{r}");
        // On the active layer when asked.
        let before = s.active().unwrap().doc.layers.len();
        assert_eq!(s.execute("filter.render.starfield", json!({"newLayer": false})).unwrap()["newLayer"], false);
        assert_eq!(s.active().unwrap().doc.layers.len(), before);

        for (id, bad) in [
            ("filter.render.starfield", json!({"density": -1})),
            ("filter.render.starfield", json!({"density": "lots"})),
            ("filter.render.starfield", json!({"sizeRange": [1]})),
            ("filter.render.starfield", json!({"sizeRange": [1, 999]})),
            ("filter.render.starfield", json!({"color": "red"})),
            ("filter.render.starfield", json!({"seed": -3})),
            ("filter.render.starfield", json!({"densty": 5})),
            ("filter.render.starburst", json!({})),
            ("filter.render.starburst", json!({"points": []})),
            ("filter.render.starburst", json!({"points": [{"x": 1}]})),
            ("filter.render.starburst", json!({"points": [{"x": 1, "y": 1}], "rays": 5})),
            ("filter.render.starburst", json!({"points": vec![json!({"x": 1, "y": 1}); 1001]})),
            ("filter.render.starburst", json!({"points": [{"x": 1e300, "y": 1}]})),
            ("filter.render.bokeh", json!({"colors": []})),
            ("filter.render.bokeh", json!({"colors": ["nope"]})),
            ("filter.render.bokeh", json!({"count": 1e9})),
            ("filter.render.bokeh", json!({"region": {"x": 500, "y": 500, "width": 10, "height": 10}})),
        ] {
            let layers = s.active().unwrap().doc.layers.len();
            assert!(s.execute(id, bad.clone()).is_err(), "{id} {bad}");
            assert_eq!(s.active().unwrap().doc.layers.len(), layers, "{id} {bad}: a refused call adds nothing");
        }
    }

    /// Lens Flare `ghosts: false` (agent report P2.16): the core only, no reflection rings across
    /// the frame. With ghosts the far side of the axis lights up; without, it stays dark.
    #[test]
    fn lens_flare_without_ghosts_lights_only_around_its_core() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 400, "height": 300, "background": "black"})).unwrap();
        // The quarter of the canvas across the centre from the flare, where ghosts land:
        // (total light, brightest pixel).
        let far = |s: &Session| -> (f32, f32) {
            let px = magicpic_compose::flatten(&s.active().unwrap().doc).px.clone();
            let lit: Vec<f32> = px.iter().enumerate().filter(|(i, _)| (i % 400) > 240 && (i / 400) > 180).map(|(_, p)| p[0] + p[1] + p[2]).collect();
            (lit.iter().sum(), lit.iter().copied().fold(0.0, f32::max))
        };
        s.execute("filter.render.lensFlare", json!({"centerX": 0.2, "centerY": 0.2, "brightness": 150})).unwrap();
        let with = far(&s);
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("filter.render.lensFlare", json!({"centerX": 0.2, "centerY": 0.2, "brightness": 150, "ghosts": false})).unwrap();
        let without = far(&s);
        // What remains there is the core's wide, faint glow: no rings or discs (bright peaks).
        assert!(with.0 > 1.0 && without.0 < with.0 * 0.4, "ghost light across the frame: with {with:?}, without {without:?}");
        // The core's one-pixel rays still reach this far; the bright ring and disc peaks do not.
        assert!(without.1 < with.1 * 0.5, "no ghost peaks: with {with:?}, without {without:?}");
        // The core is still there.
        let px = magicpic_compose::flatten(&s.active().unwrap().doc).px.clone();
        assert!(px[60 * 400 + 80][0] > 0.5, "the flare's core");
    }
}
