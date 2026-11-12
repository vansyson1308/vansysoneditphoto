//! Seeing: observation aids and exploratory variants (View › Seeing, `seeing.*`).
//!
//! Three things, all of which leave the document they look at unchanged:
//!
//! * **Views** (`seeing.view`): a reduced render of the document, or of a region of it, shown
//!   plainly or through one of the [`magicpic_algo::seeing::View`]s (tones, colour masses,
//!   shapes, lines, colour without tone). A view is returned as a PNG; it is never written into
//!   the document, so it cannot end up in an export by accident. `seeing.study.*` opens a view
//!   as a separate document for looking at in the app.
//! * **Frames** (`seeing.frames`): seeded random rectangles of the picture to study or crop to.
//! * **Variants** (`seeing.variant`): a copy of the document with a *recipe* applied: an
//!   optional frame and a list of ordinary engine commands. The copy is all-or-nothing: if a
//!   step fails it is discarded. The recipe is stored in the copy as a note (so it is saved with
//!   `.pcraft` and PSD) and replaying it on the same source gives the same pixels.
//!   `seeing.experiments` proposes seeded recipes that deliberately break one photographic rule
//!   (steady camera, sharp subject, correct exposure), using adjustment layers and smart filters
//!   so each stays editable.
//!
//! Nothing here scores a picture or decides which variant is better: the viewer's own reaction
//! is recorded, as they give it, with `seeing.note`.

use base64::Engine as _;
use magicpic_algo::seeing::{self, Chance, Frame, FrameParams, View, ViewParams};
use magicpic_color::{ColorMode, SampleType};
use magicpic_doc::{Document, Layer, LayerContent, Note};
use magicpic_geom::{Rect, Size};
use magicpic_raster::Surface;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::transaction_cmds::Checkpoint;
use crate::{EngineError, Result, Session};

/// Recipe format version written by this build.
pub const RECIPE_VERSION: u32 = 1;
/// Author of the notes that hold a variant's recipe and the viewer's reactions.
pub const NOTE_AUTHOR: &str = "MagicPic Seeing";
const RECIPE_PREFIX: &str = "recipe: ";
const REACTION_PREFIX: &str = "reaction: ";
/// Most steps in one recipe.
pub const MAX_STEPS: usize = 32;
/// Largest region `seeing.view` composites (document pixels), as automation previews.
const MAX_SOURCE_PIXELS: u64 = 64 << 20;
const MIN_SIDE: u32 = 16;
const MAX_SIDE: u32 = 2048;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

/// One engine command of a recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    #[serde(default)]
    pub params: Value,
}

/// What a variant was made from, for the reader of a stored recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// How to make a variant from a source document: crop to `frame` (if any), then run `steps`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Recipe {
    /// Format version ([`RECIPE_VERSION`]).
    #[serde(default = "version")]
    pub magicpic_seeing: u32,
    /// Short name of the variant ("exposure +2 EV").
    #[serde(default)]
    pub label: String,
    /// The experiment kind that generated it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The rule it deliberately breaks, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// The seed its chance came from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

fn version() -> u32 {
    RECIPE_VERSION
}

/// The experiment kinds `seeing.experiments` draws from, with the rule each one breaks.
pub const KINDS: &[(&str, &str)] = &[
    ("overexpose", "expose correctly"),
    ("underexpose", "expose correctly"),
    ("shake", "hold the camera steady"),
    ("pan", "hold the camera steady"),
    ("spin", "hold the camera steady"),
    ("zoom", "hold the camera steady"),
    ("defocus", "keep the subject sharp"),
    ("chance", "choose by habit"),
];

fn kinds_text() -> String {
    KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join("|")
}

/// The steps that stamp the visible picture into a smart object, so a blur on top is a smart
/// filter: the layers below stay as they were and the blur stays editable.
fn stamped(filter: Step) -> Vec<Step> {
    vec![Step { id: "layer.stampVisible".into(), params: json!({}) }, Step { id: "filter.convertForSmartFilters".into(), params: json!({}) }, filter]
}

/// The recipe of experiment `kind` for a `width` × `height` picture, its chance drawn from `rng`.
fn experiment(kind: &str, seed: u64, rng: &mut Chance, width: u32, height: u32) -> Option<Recipe> {
    let rule = KINDS.iter().find(|k| k.0 == kind)?.1;
    let long = width.max(height).max(1) as f32;
    let (label, frame, steps) = match kind {
        "overexpose" | "underexpose" => {
            let stops = 1 + rng.below(3) as i32;
            let ev = if kind == "overexpose" { stops } else { -stops };
            let name = format!("Seeing: {ev:+} EV");
            (format!("exposure {ev:+} EV"), None, vec![Step { id: "layer.newAdjustmentLayer.exposure".into(), params: json!({"exposure": ev, "name": name}) }])
        }
        "shake" | "pan" => {
            // 2–6 % of the long side: enough to turn trunks into streaks, not to erase the picture.
            let distance = (long * rng.between(0.02, 0.06)).round().clamp(1.0, 2000.0);
            let angle = if kind == "shake" { 90 } else { 0 };
            let label = if kind == "shake" { "vertical camera movement" } else { "horizontal camera movement" };
            (label.to_string(), None, stamped(Step { id: "filter.blur.motionBlur".into(), params: json!({"angle": angle, "distance": distance}) }))
        }
        "spin" => {
            let amount = rng.between(8.0, 25.0).round();
            (
                "camera turned during the exposure".to_string(),
                None,
                stamped(Step { id: "filter.blur.radialBlur".into(), params: json!({"amount": amount, "method": "spin", "quality": "good"}) }),
            )
        }
        "zoom" => {
            let amount = rng.between(15.0, 50.0).round();
            (
                "moving towards the subject".to_string(),
                None,
                stamped(Step { id: "filter.blur.radialBlur".into(), params: json!({"amount": amount, "method": "zoom", "quality": "good"}) }),
            )
        }
        "defocus" => {
            let radius = ((long * rng.between(0.004, 0.02) * 10.0).round() / 10.0).clamp(0.1, 1000.0);
            ("out of focus".to_string(), None, stamped(Step { id: "filter.blur.gaussianBlur".into(), params: json!({"radius": radius}) }))
        }
        "chance" => {
            // Where the hoop lands, then one experiment on what is inside it.
            let frame = seeing::frames(rng.next_u64(), width, height, FrameParams { count: 1, ..Default::default() }).ok()?.into_iter().next()?;
            let pool: Vec<&str> = KINDS.iter().map(|k| k.0).filter(|k| *k != "chance").collect();
            let pick = pool.get(rng.below(pool.len())).copied()?;
            let inner = experiment(pick, seed, rng, frame.width, frame.height)?;
            (format!("chance frame, {}", inner.label), Some(frame), inner.steps)
        }
        _ => return None,
    };
    Some(Recipe { magicpic_seeing: RECIPE_VERSION, label, kind: Some(kind.into()), rule: Some(rule.into()), seed: Some(seed), frame, steps, source: None })
}

/// The recipe for `kind` and `seed`: a pure function of both and the picture's size.
pub fn experiment_recipe(kind: &str, seed: u64, width: u32, height: u32) -> Option<Recipe> {
    // The kind is mixed into the stream so "overexpose" and "defocus" of one seed are unrelated.
    let salt = kind.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    experiment(kind, seed, &mut Chance::new(seed ^ salt), width, height)
}

/// Refuse parameters the command does not have, so a misspelt key is an error and not a
/// silent default (`veiw` would otherwise show the plain picture).
fn known(cmd: &str, p: &Value, keys: &[&str]) -> Result<()> {
    match p.as_object().and_then(|o| o.keys().find(|k| !keys.contains(&k.as_str()))) {
        Some(key) => {
            Err(bad(cmd, format!("unknown parameter `{key}`; this command takes {}", if keys.is_empty() { "none".to_string() } else { keys.join(", ") })))
        }
        None => Ok(()),
    }
}

fn seed_param(cmd: &str, p: &Value) -> Result<Option<u64>> {
    match p.get("seed") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| bad(cmd, "`seed` must be a whole number from 0 up")),
    }
}

fn unit_param(cmd: &str, p: &Value, key: &str, default: f32, lo: f32, hi: f32) -> Result<f32> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let f = v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` must be a number")))? as f32;
            if (lo..=hi).contains(&f) { Ok(f) } else { Err(bad(cmd, format!("`{key}` must be between {lo} and {hi}"))) }
        }
    }
}

fn count_param(cmd: &str, p: &Value, key: &str, default: u32, lo: u32, hi: u32) -> Result<u32> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number")))?;
            u32::try_from(n).ok().filter(|n| (lo..=hi).contains(n)).ok_or_else(|| bad(cmd, format!("`{key}` must be between {lo} and {hi}")))
        }
    }
}

fn view_params(cmd: &str, p: &Value) -> Result<ViewParams> {
    let d = ViewParams::default();
    Ok(ViewParams { strength: unit_param(cmd, p, "strength", d.strength, 0.0, 1.0)?, levels: count_param(cmd, p, "levels", d.levels, 2, 8)? })
}

/// `"view"`: `None` for the plain picture ("normal", the default).
fn view_param(cmd: &str, p: &Value) -> Result<Option<View>> {
    match p.get("view") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s == "normal" => Ok(None),
        Some(Value::String(s)) => {
            View::from_id(s).map(Some).ok_or_else(|| bad(cmd, format!("unknown view `{s}`: use normal|{}", View::ALL.map(View::id).join("|"))))
        }
        Some(_) => Err(bad(cmd, "`view` must be a text")),
    }
}

/// A frame given as `{x,y,width,height}` in document pixels, checked to lie inside the canvas.
fn frame_of(cmd: &str, key: &str, v: &Value, doc: &Document) -> Result<Frame> {
    let field = |k: &str| -> Result<u32> {
        let n = v
            .get(k)
            .and_then(Value::as_u64)
            .ok_or_else(|| bad(cmd, format!("`{key}` needs whole, non-negative `x`, `y`, `width` and `height` in document pixels")))?;
        u32::try_from(n).map_err(|_| bad(cmd, format!("`{key}.{k}` is too large")))
    };
    let f = Frame { x: field("x")?, y: field("y")?, width: field("width")?, height: field("height")? };
    let inside = f.width >= 1
        && f.height >= 1
        && u64::from(f.x) + u64::from(f.width) <= u64::from(doc.size.width)
        && u64::from(f.y) + u64::from(f.height) <= u64::from(doc.size.height);
    if inside { Ok(f) } else { Err(bad(cmd, format!("`{key}` must lie inside the {} x {} canvas and be at least 1 x 1", doc.size.width, doc.size.height))) }
}

fn frame_rect(f: Frame) -> Option<Rect> {
    let x0 = i32::try_from(f.x).ok()?;
    let y0 = i32::try_from(f.y).ok()?;
    let x1 = x0.checked_add(i32::try_from(f.width).ok()?)?;
    let y1 = y0.checked_add(i32::try_from(f.height).ok()?)?;
    Some(Rect::new(x0, y0, x1, y1))
}

/// The document (or `region` of it) reduced to at most `max_side` and shown through `view`:
/// width, height and straight-alpha pixels.
///
/// `linear` says the composite holds linear light (a linear profile, as an opened EXR has): its
/// numbers are then given the sRGB curve first, as the canvas does, so the picture and its
/// tones look as they do on screen instead of far too dark.
fn render_view(
    cmd: &str,
    doc: &Document,
    linear: bool,
    region: Option<Frame>,
    max_side: u32,
    view: Option<View>,
    params: ViewParams,
) -> Result<(u32, u32, Vec<[f32; 4]>)> {
    if doc.size.width == 0 || doc.size.height == 0 {
        return Err(bad(cmd, "the document has no pixels"));
    }
    let buf = match region {
        None => magicpic_compose::thumbnail_buffer(doc, max_side),
        Some(f) => {
            if u64::from(f.width) * u64::from(f.height) > MAX_SOURCE_PIXELS {
                return Err(bad(cmd, format!("`region` covers more than {MAX_SOURCE_PIXELS} pixels; pass a smaller region")));
            }
            let rect = frame_rect(f).ok_or_else(|| bad(cmd, "`region` is outside the coordinate range"))?;
            let scale = (max_side as f32 / f.width.max(f.height) as f32).min(1.0);
            let w = ((f.width as f32 * scale).round() as u32).max(1);
            let h = ((f.height as f32 * scale).round() as u32).max(1);
            magicpic_compose::render_reduced_rect(doc, rect, w, h)
        }
    };
    let (w, h) = (buf.rect.width(), buf.rect.height());
    let mut px = buf.into_px();
    if linear {
        let encode = |v: f32| if v.is_finite() { magicpic_color::convert::linear_to_srgb(v.clamp(0.0, 65504.0)) } else { 0.0 };
        for q in px.iter_mut() {
            *q = [encode(q[0]), encode(q[1]), encode(q[2]), q[3]];
        }
    }
    if let Some(view) = view {
        seeing::apply_view(view, params, w as usize, h as usize, &mut px).map_err(|e| bad(cmd, e.to_string()))?;
    }
    Ok((w, h, px))
}

/// Whether the canvas shows `doc`'s composite through the sRGB curve because it is linear light
/// (see [`render_view`]): a view shows what the canvas shows. A document whose display transform
/// cannot be built is shown as its numbers are.
fn is_linear(s: &Session, doc: &Document) -> bool {
    s.color.canvas_display(doc).is_ok_and(|d| d.encode_srgb)
}

fn max_side_param(cmd: &str, p: &Value, default: u32) -> Result<u32> {
    count_param(cmd, p, "maxSide", default, MIN_SIDE, MAX_SIDE)
}

/// `seeing.view`: an image of a document through a view. A query: the document is not touched.
fn view(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "seeing.view";
    known(CMD, p, &["view", "maxSide", "region", "strength", "levels", "format", "quality", "document"])?;
    let st = match p.get("document") {
        None | Some(Value::Null) => s.active().ok_or(EngineError::NoDocument)?,
        Some(v) => {
            let i = v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(CMD, "`document` must be a document index"))?;
            s.documents().get(i).ok_or_else(|| bad(CMD, format!("no document at index {i} ({} open)", s.documents().len())))?
        }
    };
    let doc = &st.doc;
    let view = view_param(CMD, p)?;
    let params = view_params(CMD, p)?;
    let max_side = max_side_param(CMD, p, 1024)?;
    let jpeg = match p.get("format") {
        None | Some(Value::Null) => false,
        Some(Value::String(f)) if f == "png" => false,
        Some(Value::String(f)) if f == "jpeg" => true,
        Some(_) => return Err(bad(CMD, "`format` must be \"png\" or \"jpeg\"")),
    };
    let quality = count_param(CMD, p, "quality", 85, 1, 100)? as u8;
    let region = match p.get("region") {
        None | Some(Value::Null) => None,
        Some(v) => Some(frame_of(CMD, "region", v, doc)?),
    };
    let (w, h, px) = render_view(CMD, doc, is_linear(s, doc), region, max_side, view, params)?;
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let (layout, mime, format, options) = if jpeg {
        let options = magicpic_codecs::EncodeOptions { jpeg_quality: quality, ..Default::default() };
        (magicpic_codecs::ChannelLayout::Rgb, "image/jpeg", magicpic_codecs::Format::Jpeg, options)
    } else {
        (magicpic_codecs::ChannelLayout::Rgba, "image/png", magicpic_codecs::Format::Png, Default::default())
    };
    let mut bytes = Vec::with_capacity(px.len().saturating_mul(4));
    for q in &px {
        if jpeg {
            // JPEG has no transparency: the picture is shown on white paper.
            let a = q[3].clamp(0.0, 1.0);
            bytes.extend([0, 1, 2].map(|c| to_u8(q[c] * a + (1.0 - a))));
        } else {
            bytes.extend(q.map(to_u8));
        }
    }
    let image = magicpic_codecs::Image::from_u8(w, h, layout, bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    let encoded = magicpic_codecs::encode(&image, format, &options).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({
        "mime": mime,
        "base64": base64::engine::general_purpose::STANDARD.encode(&encoded),
        "bytes": encoded.len(),
        "width": w,
        "height": h,
        "view": view.map_or("normal", View::id),
        "description": view.map_or("the picture as it is", View::describe),
        // A view is something to look through. It is not an edit and is not in the document.
        "analysisOnly": view.is_some(),
        "region": region,
        "document": {"id": doc.id.0, "name": doc.name, "width": doc.size.width, "height": doc.size.height, "revision": st.revision},
    }))
}

/// `seeing.study`: open a view of the active document as a new, separate document.
fn study(s: &mut Session, p: &Value, fixed: Option<View>) -> Result<Value> {
    const CMD: &str = "seeing.study";
    known(CMD, p, if fixed.is_some() { &["maxSide", "strength", "levels"] } else { &["view", "maxSide", "strength", "levels"] })?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let src = &st.doc;
    let linear = is_linear(s, src);
    let view = match fixed {
        Some(v) => v,
        None => view_param(CMD, p)?.ok_or_else(|| bad(CMD, format!("pass `view`: {}", View::ALL.map(View::id).join("|"))))?,
    };
    let params = view_params(CMD, p)?;
    let max_side = max_side_param(CMD, p, MAX_SIDE)?;
    let (w, h, px) = render_view(CMD, src, linear, None, max_side, Some(view), params)?;
    let mut doc = Document::new(format!("{} ({} study)", src.name, view.id()), Size::new(w, h), ColorMode::Rgb, SampleType::U8);
    doc.resolution_dpi = src.resolution_dpi;
    // The view's numbers are the source's own RGB numbers, so its profile still describes them
    // (not for a linear source, whose numbers were given the sRGB curve to be looked at).
    if src.mode == ColorMode::Rgb && !linear {
        doc.icc_profile = src.icc_profile.clone();
    }
    let fmt = doc.pixel_format();
    let mut surf = Surface::new(fmt);
    let vals: Vec<f32> = px.iter().flat_map(|q| magicpic_raster::from_rgba(&fmt, *q)).collect();
    surf.write_region(doc.bounds(), &vals);
    surf.prune();
    doc.layers = vec![Layer::new(format!("Study: {}", view.id()), LayerContent::Raster(surf))];
    let name = doc.name.clone();
    let index = s.add_document(doc, None);
    Ok(json!({"document": index, "name": name, "view": view.id(), "width": w, "height": h, "description": view.describe()}))
}

/// `seeing.frames`: seeded random frames of the active document. A query.
fn frames(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "seeing.frames";
    known(CMD, p, &["seed", "count", "minFraction", "maxFraction", "aspect"])?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let d = FrameParams::default();
    let aspect = match p.get("aspect") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| bad(CMD, "`aspect` must be a number (width / height)"))? as f32),
    };
    let params = FrameParams {
        count: count_param(CMD, p, "count", d.count, 1, seeing::MAX_FRAMES as u32)?,
        min_fraction: unit_param(CMD, p, "minFraction", d.min_fraction, 0.05, 1.0)?,
        max_fraction: unit_param(CMD, p, "maxFraction", d.max_fraction, 0.05, 1.0)?,
        aspect,
    };
    let seed = seed_param(CMD, p)?.unwrap_or(0);
    let frames = seeing::frames(seed, doc.size.width, doc.size.height, params).map_err(|e| bad(CMD, e.to_string()))?;
    Ok(json!({"seed": seed, "frames": frames, "documentWidth": doc.size.width, "documentHeight": doc.size.height}))
}

/// `seeing.experiments`: seeded rule-breaking recipes for the active document. A query.
fn experiments(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "seeing.experiments";
    known(CMD, p, &["seed", "count", "kinds"])?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let seed = seed_param(CMD, p)?.unwrap_or(0);
    let kinds: Vec<String> = match p.get("kinds") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            a.iter().map(|k| k.as_str().map(str::to_string).ok_or_else(|| bad(CMD, "`kinds` must be a list of texts"))).collect::<Result<_>>()?
        }
        Some(_) => return Err(bad(CMD, "`kinds` must be a list of texts")),
    };
    if kinds.len() > KINDS.len() {
        return Err(bad(CMD, format!("`kinds` lists at most {} kinds", KINDS.len())));
    }
    if let Some(k) = kinds.iter().find(|k| !KINDS.iter().any(|known| known.0 == k.as_str())) {
        return Err(bad(CMD, format!("unknown kind `{k}`: use {}", kinds_text())));
    }
    let count = count_param(CMD, p, "count", if kinds.is_empty() { 3 } else { kinds.len() as u32 }, 1, 12)? as usize;
    // Without `kinds`, the seed picks them, never the same kind twice in a row.
    let mut rng = Chance::new(seed);
    let mut out = Vec::with_capacity(count);
    let mut last: Option<&str> = None;
    for i in 0..count {
        let kind: &str = match kinds.get(i % kinds.len().max(1)) {
            Some(k) => k.as_str(),
            None => {
                let pool: Vec<&str> = KINDS.iter().map(|k| k.0).filter(|k| Some(*k) != last).collect();
                pool.get(rng.below(pool.len())).copied().unwrap_or("overexpose")
            }
        };
        last = KINDS.iter().map(|k| k.0).find(|k| *k == kind);
        // Each recipe gets its own seed, so any one of them can be made again alone.
        let each = seed.wrapping_add(i as u64);
        let recipe = experiment_recipe(kind, each, doc.size.width, doc.size.height).ok_or_else(|| bad(CMD, format!("unknown kind `{kind}`")))?;
        out.push(recipe);
    }
    Ok(
        json!({"seed": seed, "version": RECIPE_VERSION, "experiments": out, "kinds": KINDS.iter().map(|k| json!({"kind": k.0, "breaks": k.1})).collect::<Vec<_>>()}),
    )
}

fn check_recipe(cmd: &str, r: &Recipe) -> Result<()> {
    if r.magicpic_seeing != RECIPE_VERSION {
        return Err(bad(cmd, format!("recipe version {} is not supported (this build reads version {RECIPE_VERSION})", r.magicpic_seeing)));
    }
    if r.steps.len() > MAX_STEPS {
        return Err(bad(cmd, format!("a recipe has at most {MAX_STEPS} steps")));
    }
    if r.label.chars().count() > 200 {
        return Err(bad(cmd, "`label` is at most 200 characters"));
    }
    for (i, step) in r.steps.iter().enumerate() {
        // A recipe edits one copy. It does not make further variants (unbounded recursion), nor
        // open, save, close or switch documents.
        if !crate::transaction_cmds::stays_on_document(&step.id) {
            return Err(bad(cmd, format!("step {} (`{}`) is not allowed in a recipe: steps edit the variant only", i + 1, step.id)));
        }
        if !(step.params.is_object() || step.params.is_null()) {
            return Err(bad(cmd, format!("step {} (`{}`): `params` must be an object", i + 1, step.id)));
        }
    }
    Ok(())
}

fn recipe_note(recipe: &Recipe) -> Result<Note> {
    let text = serde_json::to_string(recipe).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(Note { author: NOTE_AUTHOR.into(), text: format!("{RECIPE_PREFIX}{text}"), open: false, ..Default::default() })
}

/// Make the variant `recipe` describes from the active document. All or nothing: a failure
/// puts the whole session back as it was ([`Checkpoint`]), so no half-made copy stays open and
/// no other document keeps a change a step made to it.
fn make_variant(s: &mut Session, cmd: &str, mut recipe: Recipe, name: Option<&str>) -> Result<Value> {
    check_recipe(cmd, &recipe)?;
    if s.active_job().is_some() {
        return Err(bad(cmd, "a background job is running on this document; wait for it or cancel it first"));
    }
    let (source_index, src_name, size) = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        (s.active_index().unwrap_or(0), st.doc.name.clone(), st.doc.size)
    };
    if let Some(f) = recipe.frame {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        frame_of(cmd, "recipe.frame", &json!(f), &st.doc)?;
    }
    recipe.source = Some(Source { name: src_name.clone(), width: size.width, height: size.height });
    let label = if recipe.label.trim().is_empty() { "variant".to_string() } else { recipe.label.clone() };
    let name = name.map(str::to_string).unwrap_or_else(|| format!("{src_name} ({label})"));
    let checkpoint = Checkpoint::take(s);
    s.execute("image.duplicate", json!({"name": name}))?;
    let Some(variant) = s.active().map(|d| d.doc.id).filter(|id| checkpoint.strayed(s, &[], &[*id], *id).is_none()) else {
        checkpoint.restore(s);
        return Err(EngineError::Other("the variant could not be created".into()));
    };
    let mut steps: Vec<Step> = Vec::with_capacity(recipe.steps.len() + 1);
    if let Some(f) = recipe.frame {
        // The pixels outside the frame are kept, so the crop can be changed later.
        steps.push(Step { id: "image.crop".into(), params: json!({"x": f.x, "y": f.y, "width": f.width, "height": f.height, "deleteCroppedPixels": false}) });
    }
    steps.extend(recipe.steps.iter().cloned());
    let total = steps.len();
    let mut failed = None;
    for (i, step) in steps.iter().enumerate() {
        let params = if step.params.is_null() { json!({}) } else { step.params.clone() };
        let why = match s.execute(&step.id, params) {
            Ok(_) => checkpoint.strayed(s, &[], &[variant], variant),
            Err(e) => Some(e.to_string()),
        };
        if let Some(why) = why {
            failed = Some(format!("step {} of {total} (`{}`) failed: {why}", i + 1, step.id));
            break;
        }
    }
    if failed.is_none() {
        // The copy starts its own record: the source's recipe and reactions are the source's.
        let note = recipe_note(&recipe)?;
        failed = s
            .edit("Seeing Recipe", |doc, _| {
                doc.notes.retain(|n| n.author != NOTE_AUTHOR);
                doc.notes.push(note);
                Ok(())
            })
            .err()
            .map(|e| e.to_string());
    }
    if let Some(why) = failed {
        checkpoint.restore(s);
        return Err(bad(cmd, format!("{why}. The variant was discarded; the source document and every other open document are as they were")));
    }
    let st = s.active().ok_or(EngineError::NoDocument)?;
    Ok(json!({
        "document": s.active_index(),
        "id": st.doc.id.0,
        "name": st.doc.name,
        "width": st.doc.size.width,
        "height": st.doc.size.height,
        "revision": st.revision,
        "sourceDocument": source_index,
        "recipe": recipe,
    }))
}

/// `seeing.variant`: `{recipe}` to apply one, or `{kind, seed}` to generate and apply one.
fn variant(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "seeing.variant";
    known(CMD, p, &["recipe", "kind", "seed", "name"])?;
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(n)) if !n.trim().is_empty() && n.chars().count() <= 200 => Some(n.as_str()),
        Some(_) => return Err(bad(CMD, "`name` must be a text of 1 to 200 characters")),
    };
    let recipe = match (p.get("recipe"), p.get("kind")) {
        (Some(r), None) if r.is_object() => serde_json::from_value::<Recipe>(r.clone()).map_err(|e| bad(CMD, format!("`recipe` is not a recipe: {e}")))?,
        (None, Some(Value::String(kind))) => {
            let size = s.active().ok_or(EngineError::NoDocument)?.doc.size;
            let seed = seed_param(CMD, p)?.unwrap_or(0);
            experiment_recipe(kind, seed, size.width, size.height).ok_or_else(|| bad(CMD, format!("unknown kind `{kind}`: use {}", kinds_text())))?
        }
        _ => {
            return Err(bad(
                CMD,
                format!("pass either `recipe` (an object, as `seeing.experiments` and `seeing.recipe` return) or `kind` ({}) with a `seed`", kinds_text()),
            ));
        }
    };
    make_variant(s, CMD, recipe, name)
}

/// A seed for the menu's "New Chance Variant": different each time the document or the set of
/// open documents changed, and recorded in the recipe, so the result can still be made again.
fn menu_seed(s: &Session) -> u64 {
    let (id, revision) = s.active().map_or((0, 0), |d| (d.doc.id.0, d.revision));
    let mut c = Chance::new(id ^ revision.rotate_left(32) ^ (s.documents().len() as u64).rotate_left(48));
    // 2^53 at most, so the seed survives any JSON reader.
    c.next_u64() >> 11
}

fn chance_variant(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "seeing.variant.chance";
    known(CMD, p, &[])?;
    let size = s.active().ok_or(EngineError::NoDocument)?.doc.size;
    let seed = menu_seed(s);
    let recipe = experiment_recipe("chance", seed, size.width, size.height).ok_or_else(|| bad(CMD, "the document is too small for a chance frame"))?;
    make_variant(s, CMD, recipe, None)
}

/// The recipe and the reactions stored in a document's notes.
fn stored(doc: &Document) -> (Option<Recipe>, Vec<&str>) {
    let ours = || doc.notes.iter().filter(|n| n.author == NOTE_AUTHOR);
    let recipe = ours().filter_map(|n| n.text.strip_prefix(RECIPE_PREFIX)).filter_map(|t| serde_json::from_str::<Recipe>(t).ok()).next_back();
    (recipe, ours().filter_map(|n| n.text.strip_prefix(REACTION_PREFIX)).collect())
}

/// `seeing.recipe`: the recipe the active document was made with, and the reactions noted. A query.
fn recipe(s: &mut Session, p: &Value) -> Result<Value> {
    known("seeing.recipe", p, &[])?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let (recipe, reactions) = stored(&st.doc);
    Ok(json!({"recipe": recipe, "reactions": reactions, "document": st.doc.name}))
}

/// `seeing.note`: record the viewer's reaction to the active document, in their own words.
fn note(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "seeing.note";
    known(CMD, p, &["reaction", "reason"])?;
    let text = |key: &str, max: usize, required: bool| -> Result<String> {
        match p.get(key) {
            Some(Value::String(t)) if t.chars().count() <= max && !(required && t.trim().is_empty()) => Ok(t.trim().to_string()),
            None | Some(Value::Null) if !required => Ok(String::new()),
            _ => Err(bad(CMD, format!("`{key}` must be a text of at most {max} characters{}", if required { ", not empty" } else { "" }))),
        }
    };
    let reaction = text("reaction", 200, true)?;
    let reason = text("reason", 2000, false)?;
    let body = if reason.is_empty() { reaction.clone() } else { format!("{reaction} (why: {reason})") };
    let entry = Note { author: NOTE_AUTHOR.into(), text: format!("{REACTION_PREFIX}{body}"), open: false, ..Default::default() };
    s.edit("Seeing Note", |doc, _| {
        doc.notes.push(entry);
        Ok(())
    })?;
    let st = s.active().ok_or(EngineError::NoDocument)?;
    Ok(json!({"reactions": stored(&st.doc).1, "document": st.doc.name}))
}

macro_rules! spec {
    ($id:literal, $label:literal, [$($m:literal),*], $params:literal, $journal:literal, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: None, params: $params, enabled: has_doc, run: $run, journal: $journal }
    };
}

/// Seeing command specs.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "seeing.view",
            "Seeing View",
            [],
            r##"{"view":"normal|luminance|masses|shapes|lines|color"="normal","maxSide":16..2048=1024,"region":{"x":px,"y":px,"width":px,"height":px}?,"strength":0..1=0.5,"levels":2..8=3,"format":"png|jpeg"="png","quality":1..100=85,"document":index?=the active document} → {mime,base64,bytes,width,height,view,description,analysisOnly,region,document:{id,name,width,height,revision}} (a look at a document, or a region of it, that changes nothing; jpeg is smaller and shows transparency as white: luminance = tones only; masses = strongly defocused colour areas; shapes = `levels` flat tones of the picture's own range, figure against space; lines = outlines on white; color = hue and saturation at equal luminosity; strength = how far masses/shapes/lines abstract. Views are for looking only and are never part of the document or an export)"##,
            false,
            view
        ),
        spec!(
            "seeing.study",
            "Seeing Study",
            [],
            r##"{"view":"luminance|masses|shapes|lines|color","maxSide":16..2048=2048,"strength":0..1=0.5,"levels":2..8=3} → {document,name,view,width,height,description} (opens the view as a new, separate RGB document to look at next to the picture; the source document is unchanged)"##,
            true,
            |s, p| study(s, p, None)
        ),
        spec!("seeing.study.luminance", "Tones Study", ["View", "Seeing"], "{} (seeing.study with view luminance)", true, |s, p| study(
            s,
            p,
            Some(View::Luminance)
        )),
        spec!("seeing.study.masses", "Color Masses Study", ["View", "Seeing"], "{} (seeing.study with view masses)", true, |s, p| study(
            s,
            p,
            Some(View::Masses)
        )),
        spec!("seeing.study.shapes", "Shapes Study", ["View", "Seeing"], "{} (seeing.study with view shapes)", true, |s, p| study(s, p, Some(View::Shapes))),
        spec!("seeing.study.lines", "Lines Study", ["View", "Seeing"], "{} (seeing.study with view lines)", true, |s, p| study(s, p, Some(View::Lines))),
        spec!("seeing.study.color", "Color Without Tone Study", ["View", "Seeing"], "{} (seeing.study with view color)", true, |s, p| study(
            s,
            p,
            Some(View::Color)
        )),
        spec!(
            "seeing.frames",
            "Chance Frames",
            [],
            r##"{"seed":int=0,"count":1..64=6,"minFraction":0.05..1=0.2,"maxFraction":0.05..1=0.5,"aspect":0.1..10?=the document's} → {seed,frames:[{x,y,width,height}],documentWidth,documentHeight} (random rectangles inside the active document, a pure function of the seed: places to look at with seeing.view {region} or to crop a variant to; fractions are of the document's shorter side; nothing changes)"##,
            false,
            frames
        ),
        spec!(
            "seeing.experiments",
            "Seeing Experiments",
            [],
            r##"{"seed":int=0,"count":1..12=3,"kinds":["overexpose|underexpose|shake|pan|spin|zoom|defocus|chance"]?} → {seed,version,experiments:[recipe],kinds:[{kind,breaks}]} (recipes that each break one rule on purpose: exposure by ±1..3 EV as an adjustment layer, camera movement and defocus as smart filters on a stamp of the picture, chance = a random frame plus one of them; a pure function of the seed and the document size; nothing changes until a recipe is passed to seeing.variant)"##,
            false,
            experiments
        ),
        spec!(
            "seeing.variant",
            "New Variant",
            [],
            r##"{"recipe":{"label":str,"frame":{"x":px,"y":px,"width":px,"height":px}?,"steps":[{"id":command id,"params":json}]} | "kind":"overexpose|underexpose|shake|pan|spin|zoom|defocus|chance","seed":int=0,"name":str?} → {document,id,name,width,height,revision,sourceDocument,recipe} (a copy of the active document, cropped to the frame without deleting pixels, with the steps run on it in order; all or nothing: if a step fails, or reaches another document, the copy is discarded, every open document is as it was and the error names the step; the recipe is stored in the copy as a note; a recipe whose steps do not depend on tool colours, brush settings or layer ids gives the same pixels on the same source every time; at most 32 steps, which may not open, save, close or switch documents, step through history or change session-wide settings)"##,
            true,
            variant
        ),
        spec!(
            "seeing.variant.chance",
            "New Chance Variant",
            ["View", "Seeing"],
            "{} (seeing.variant with kind chance and a seed taken from the document's state; the seed is in the stored recipe)",
            true,
            chance_variant
        ),
        spec!(
            "seeing.recipe",
            "Variant Recipe",
            [],
            r##"{} → {recipe,reactions:[str],document} (the recipe the active document was made with by seeing.variant, null if none, and the reactions noted with seeing.note)"##,
            false,
            recipe
        ),
        spec!(
            "seeing.note",
            "Note Reaction",
            [],
            r##"{"reaction":str (1..200 chars),"reason":str=""} → {reactions:[str],document} (records the viewer's own reaction to the active document, and why, as a note saved with it; one history step)"##,
            true,
            note
        ),
    ]
}

#[cfg(test)]
mod tests;
