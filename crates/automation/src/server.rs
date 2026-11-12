//! The MCP server (rmcp). Tool names use `snake_case` with underscores (dots
//! are not valid in every client's tool-name grammar).

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock as Content};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::bridge::BridgeClient;
use crate::budgets::{BatchReplyBudget, MAX_PREVIEW_SIDE, check_png, json_bytes};
use crate::headless::Headless;
use crate::security::MAX_BATCH_STEPS;
use crate::{AuthorizedWorkspace, AutomationError, files};

/// Where tools are executed.
pub enum Backend {
    /// In-process engine session.
    Headless(Arc<Mutex<Headless>>),
    /// A running desktop app reached over the control protocol.
    Bridge(Arc<BridgeClient>),
}

#[derive(Clone)]
pub struct MagicpicMcp {
    backend: Arc<Backend>,
    tool_router: ToolRouter<Self>,
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Which open document a tool works on: its tab `index` (a number, or `{"index": n}`) or its
/// `id` (`{"id": n}`), both from `session_list` or `doc_open`. An id stays the same while the
/// document is open; an index changes when tabs close or move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DocRef {
    Index(usize),
    Selector(DocSelector),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocSelector {
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub id: Option<u64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocIndex {
    /// Document index from `session_list` (default: the active document).
    #[serde(default)]
    pub index: Option<usize>,
    /// The document by index or `{"id": n}` (instead of `index`).
    #[serde(default)]
    pub document: Option<DocRef>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionListParams {
    /// The full state instead of the documents: in bridge mode the whole UI (`ui.inspect`: tools,
    /// panels, dock, memory; about 10k tokens), headless the session's colours and history options.
    #[serde(default)]
    pub verbose: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// Path beneath the configured automation read roots: relative (forward slashes, searched
    /// in launch order) or absolute. Only a missing file tries the next root.
    pub path: String,
    /// Explicitly approve high-memory PSD/PSB imports that fit within the hard memory budget.
    #[serde(default)]
    pub approve_high_memory: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaceParams {
    /// Path beneath the configured automation read root: relative (forward slashes) or absolute.
    pub path: String,
    /// Document index from `session_list` (default: the active document).
    #[serde(default)]
    pub index: Option<usize>,
    /// Scale as a percentage; defaults to fitting a file larger than the canvas.
    #[serde(default)]
    pub scale: Option<f64>,
    /// Place at a centred size instead of a scaled one.
    #[serde(default)]
    pub fit: Option<bool>,
    /// Centre of the placed layer, in document pixels.
    #[serde(default)]
    pub center: Option<[f64; 2]>,
    /// Explicitly approve high-memory PSD/PSB imports that fit within the hard memory budget.
    #[serde(default)]
    pub approve_high_memory: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceSmartObjectParams {
    /// Path beneath the configured automation read root: relative (forward slashes) or absolute.
    pub path: String,
    /// Headless mode only: document index from `session_list` (default: the active document).
    /// Bridge mode rejects `index`.
    #[serde(default)]
    pub index: Option<usize>,
    /// The Smart Object layer: an id or a name (default: the active layer).
    #[serde(default)]
    pub layer: Option<Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportLayersParams {
    /// Directory beneath the configured automation write root: relative (forward slashes) or absolute.
    pub dir: String,
    /// Document index from `session_list` (default: the active document).
    #[serde(default)]
    pub index: Option<usize>,
    /// Export format (default png).
    #[serde(default)]
    pub format: Option<String>,
    /// Filename prefix (default: the document name).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Export only the layers that are visible (default true).
    #[serde(default, rename = "visible_only")]
    pub visible_only: Option<bool>,
    /// Encoder quality, 0..12, for the formats that take one.
    #[serde(default)]
    pub quality: Option<f64>,
    /// PSD/PSB: write the real merged image (Photoshop's Maximize Compatibility); false writes
    /// Photoshop's white placeholder (Bitmap, Indexed Color and Duotone documents still write the
    /// real image). Unset: the engine's Preferences › File Handling decides.
    #[serde(default, rename = "maximizeCompatibility")]
    pub maximize_compatibility: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportDataSetsParams {
    /// Directory beneath the configured automation write root: relative (forward slashes) or absolute.
    pub dir: String,
    /// Document index from `session_list` (default: the active document).
    #[serde(default)]
    pub index: Option<usize>,
    /// Export format (default png).
    #[serde(default)]
    pub format: Option<String>,
    /// Only these data sets, by name; default every one of them.
    #[serde(default, rename = "data_sets")]
    pub data_sets: Option<Vec<String>>,
    /// Filename template with `{name}`, `{index}` and `{document}` (default `{name}`).
    #[serde(default)]
    pub naming: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDataSetsParams {
    /// Path beneath the configured automation read root: relative (forward slashes) or absolute.
    pub path: String,
    /// Document index from `session_list` (default: the active document).
    #[serde(default)]
    pub index: Option<usize>,
    /// Field delimiter (default `,`).
    #[serde(default)]
    pub delimiter: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewParams {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// rgb | gray | cmyk | lab
    #[serde(default)]
    pub mode: Option<String>,
    /// 8 | 16 | 32
    #[serde(default)]
    pub depth: Option<u32>,
    /// white | black | transparent | #rrggbb
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveParams {
    /// Target beneath the configured automation write root: relative (forward slashes) or absolute.
    /// The extension selects the format. Omit to write back to the document's own file, which
    /// works only for a PSD, PSB or .pcraft file kept in its own format; any other save needs
    /// `path`, so a flattened or converted copy never replaces the opened file.
    /// A document opened from a different read capability needs an explicit path first.
    #[serde(default)]
    pub path: Option<String>,
    /// Format override as an extension (pcraft, psd, png, jpg, tif, webp, exr, …).
    #[serde(default)]
    pub format: Option<String>,
    /// JPEG or WebP quality 1..100. A WebP saved with a quality is lossy; without one it is lossless.
    #[serde(default)]
    pub quality: Option<u8>,
    /// TIFF: keep the layers (Photoshop layer data). Off by default: a flat TIFF.
    #[serde(default, rename = "tiffLayers")]
    pub tiff_layers: bool,
    /// TIFF: image compression, "none" (the default), "lzw" or "zip".
    #[serde(default, rename = "tiffCompression")]
    pub tiff_compression: Option<String>,
    /// TIFF: Save Transparency. On keeps the transparency, ahead of any alpha or spot channels;
    /// off composites the image over white. Unset: kept unless the document has alpha or spot
    /// channels.
    #[serde(default, rename = "tiffTransparency")]
    pub tiff_transparency: Option<bool>,
    /// TIFF: Save Image Pyramid, half-size copies after the full image.
    #[serde(default, rename = "tiffPyramid")]
    pub tiff_pyramid: bool,
    /// TGA: 24 or 32 bits per pixel. 32 writes the alpha channel (or else the transparency) as the
    /// alpha, 24 none. Unset: 32 with an alpha channel or transparency, else 24.
    #[serde(default, rename = "tgaBits")]
    pub tga_bits: Option<u8>,
    /// DPX: 8, 10, 12 or 16 bits per sample. Unset: 8 for an 8-bit document, else 16.
    #[serde(default, rename = "dpxBits")]
    pub dpx_bits: Option<u8>,
    /// DDS: "none" (uncompressed, the default), "bc1" (DXT1: alpha on or off), "bc2" (DXT3) or
    /// "bc3" (DXT5).
    #[serde(default, rename = "ddsCompression")]
    pub dds_compression: Option<String>,
    /// DDS: also write the full mip chain, each level a box filter of the one above.
    #[serde(default, rename = "ddsMipmaps")]
    pub dds_mipmaps: bool,
    /// PDF: write the spot channels as overprinting Separation plates after the flattened image
    /// (a Multichannel document as plates alone). On by default; false writes the image only.
    #[serde(default, rename = "pdfSpotPlates")]
    pub pdf_spot_plates: Option<bool>,
    /// .pcraft: request the flattened document at its canvas size as `composite/full.png`.
    /// The save may omit it; check the manifest's `full_composite` member. Off by default.
    #[serde(default, rename = "pcraftFullComposite")]
    pub pcraft_full_composite: Option<bool>,
    /// PSD/PSB: write the real merged image (Photoshop's Maximize Compatibility). false writes
    /// Photoshop's white placeholder instead: a smaller file and a faster save, but readers that
    /// show only the merged image see white. Bitmap, Indexed Color and Duotone documents still
    /// write the real image. Unset: the real image (in bridge mode, the app's
    /// Preferences › File Handling decides).
    #[serde(default, rename = "maximizeCompatibility")]
    pub maximize_compatibility: Option<bool>,
    #[serde(default)]
    pub index: Option<usize>,
    /// The document to save, by index or `{"id": n}` (default: the active one). In bridge mode
    /// its tab comes to the front first.
    #[serde(default)]
    pub document: Option<DocRef>,
}

/// A rectangle of the document, in document pixels.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegionParams {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    /// Document index (default: the active document). In bridge mode, passing it (or `view`,
    /// `region` or `format`) renders that document instead of capturing the app window.
    #[serde(default)]
    pub index: Option<usize>,
    /// The document by index or `{"id": n}` (instead of `index`).
    #[serde(default)]
    pub document: Option<DocRef>,
    /// Render only this layer (its id from `doc_inspect`), with `solo` (default true) alone on a
    /// transparent canvas, or with `solo: false` together with every layer below it. Changes nothing:
    /// no visibility, blend mode or history step.
    #[serde(default)]
    pub layer: Option<u64>,
    /// With `layer`: true (the default) shows the layer alone; false shows it and the layers below.
    #[serde(default)]
    pub solo: Option<bool>,
    /// Longest side of the preview (default 1024, maximum 2048).
    /// Zero requests full size within that ceiling.
    #[serde(default)]
    pub max_side: Option<u32>,
    /// What to look at: `normal` (the picture as it is) or an observation view that changes
    /// nothing and is never part of the document or an export: `luminance` (tones only),
    /// `masses` (strongly defocused colour areas), `shapes` (a few flat tones: figure against
    /// empty space), `lines` (outlines on white), `color` (hue and saturation at equal
    /// luminosity).
    #[serde(default)]
    pub view: Option<String>,
    /// Render only this rectangle of the document (never enlarged).
    #[serde(default)]
    pub region: Option<RegionParams>,
    /// How far `masses`, `shapes` and `lines` abstract, 0 to 1 (default 0.5).
    #[serde(default)]
    pub strength: Option<f64>,
    /// Number of flat tones of the `shapes` view, 2 to 8 (default 3).
    #[serde(default)]
    pub levels: Option<u32>,
    /// `png` (default) or `jpeg` (much smaller; transparency shows as white).
    #[serde(default)]
    pub format: Option<String>,
    /// JPEG quality 1 to 100 (default 85).
    #[serde(default)]
    pub quality: Option<u8>,
}

impl PreviewParams {
    /// Whether the call asks for a document view (`seeing.view`) rather than the plain preview.
    fn wants_view(&self) -> bool {
        self.view.is_some()
            || self.region.is_some()
            || self.strength.is_some()
            || self.levels.is_some()
            || self.format.is_some()
            || self.quality.is_some()
            || self.layer.is_some()
    }

    /// The `seeing.view` parameters of this call.
    fn view_params(&self) -> Value {
        let mut m = serde_json::Map::new();
        let side = match self.max_side {
            None => 1024,
            Some(0) => MAX_PREVIEW_SIDE,
            Some(side) => side,
        };
        m.insert("maxSide".into(), json!(side));
        if let Some(v) = &self.view {
            m.insert("view".into(), json!(v));
        }
        if let Some(r) = self.region {
            m.insert("region".into(), json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}));
        }
        if let Some(v) = self.strength {
            m.insert("strength".into(), json!(v));
        }
        if let Some(v) = self.levels {
            m.insert("levels".into(), json!(v));
        }
        if let Some(v) = &self.format {
            m.insert("format".into(), json!(v));
        }
        if let Some(v) = self.quality {
            m.insert("quality".into(), json!(v));
        }
        if let Some(v) = self.layer {
            m.insert("layer".into(), json!(v));
            m.insert("solo".into(), json!(self.solo.unwrap_or(true)));
        }
        Value::Object(m)
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistogramParams {
    /// The document by index or `{"id": n}` (default: the active one).
    #[serde(default)]
    pub document: Option<DocRef>,
    /// Count only this layer (an id from `doc_inspect`): alone (`solo`, the default) or with the
    /// layers below it (`solo: false`).
    #[serde(default)]
    pub layer: Option<u64>,
    #[serde(default)]
    pub solo: Option<bool>,
    /// `luminosity`, `red`, `green`, `blue`, `alpha` or `all` (the default).
    #[serde(default)]
    pub channel: Option<String>,
    /// Count only this rectangle, in document pixels.
    #[serde(default)]
    pub region: Option<RegionParams>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilesListParams {
    /// Folder to list, relative to a read root (default: the roots themselves).
    #[serde(default)]
    pub dir: Option<String>,
    /// Most entries to return, newest first (default 50, maximum 500).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotParams {
    /// Downscale the window screenshot to this longest side (maximum 2048).
    /// Omit or pass zero to keep the original size.
    #[serde(default)]
    pub max_side: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectParams {
    /// The tab index from `session_list`.
    #[serde(default)]
    pub index: Option<usize>,
    /// Or the document's `id` (from `session_list` or `doc_open`).
    #[serde(default)]
    pub id: Option<u64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// Only commands whose id, label or keywords contain this text (case-insensitive).
    #[serde(default)]
    pub filter: Option<String>,
    /// Only commands that can run right now.
    #[serde(default)]
    pub enabled_only: Option<bool>,
    /// Rows of just `{id, label, enabled}` (no params docs, menus or shortcuts): a short overview.
    #[serde(default)]
    pub brief: Option<bool>,
    /// Skip this many matching rows (paging; with `offset` or `limit` the reply is
    /// `{total, offset, next, commands}`, where `next` is the next page's offset or null).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many rows.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunParams {
    /// Command id, e.g. `layer.new.layer`, `filter.blur.gaussianBlur`.
    pub id: String,
    /// Command parameters as a JSON object (see the `params` doc in `command_list`).
    #[serde(default)]
    pub params: Option<Value>,
    /// Wait for a long command (filters, Content-Aware Fill, Photomerge, …) to finish (default
    /// true). With false it runs as a background job and the result is `{"job": id}`: poll it
    /// with `jobs_list`, stop it with `jobs_cancel`.
    #[serde(default)]
    pub wait: Option<bool>,
    /// Run only if the active document is still at this `revision` (from `doc_inspect` or
    /// `session_list`); otherwise nothing runs and the error says what the revision is now. Use
    /// it when a person may be editing the same document. The command then runs to the end
    /// (no background job), so it cannot be combined with `wait: false`.
    #[serde(default)]
    pub expect_revision: Option<u64>,
    /// Run only if the active document is the one with this `id` (from `doc_inspect` or
    /// `session_list`): a revision alone cannot tell two documents apart, and in the app the
    /// person may have switched tabs. Like `expect_revision`, it makes the command run to the end.
    #[serde(default)]
    pub expect_document: Option<u64>,
    /// Run on this document (index, or `{"id": n}`): it becomes the active one first (in the app,
    /// its tab comes to the front) and stays active. Not with `wait: false`.
    #[serde(default)]
    pub document: Option<DocRef>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobCancelParams {
    /// The job id from `command_run` / `jobs_list` (omit to cancel every running job).
    #[serde(default)]
    pub job: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchParams {
    /// Commands to run in order: `[{"id": "layer.new.layer", "params": {"name": "Ink"}}, …]`. A
    /// step with `"wait": false` starts a long command as a background job (its result is
    /// `{"job": id}`, as in `command_run`); later steps that edit the same document fail until
    /// the job ends.
    pub steps: Vec<RunParams>,
    /// Stop at the first failing step (default true).
    #[serde(default)]
    pub stop_on_error: Option<bool>,
    /// All or nothing (default false): if a step fails, every earlier step is rolled back and
    /// the document, its history and its snapshots are exactly as before. The steps then run
    /// as one engine transaction on the active document: none may open, save, close or switch
    /// documents or use `wait: false`, and there is no per-step progress.
    #[serde(default)]
    pub atomic: Option<bool>,
    /// With `atomic`: run only if the active document is still at this `revision` (from
    /// `doc_inspect` or `session_list`); otherwise nothing runs.
    #[serde(default)]
    pub expect_revision: Option<u64>,
    /// With `atomic`: run only if the active document is the one with this `id` (from
    /// `doc_inspect` or `session_list`); otherwise nothing runs.
    #[serde(default)]
    pub expect_document: Option<u64>,
    /// Run every step on this document (index, or `{"id": n}`), made active first. With `atomic`
    /// the transaction also refuses to run if another document became active meanwhile.
    #[serde(default)]
    pub document: Option<DocRef>,
}

/// One `ui_pointer` event, for the tool schema only: events are forwarded to `ui.pointer` as JSON.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct PointerEvent {
    /// `down`, `move` (default) or `up`.
    kind: Option<PointerKind>,
    /// Document x in pixels.
    x: f64,
    /// Document y in pixels.
    y: f64,
    /// Pen pressure 0..1 (default 1).
    pressure: Option<f64>,
    /// A simulated pen's tilt, as a stylus reports it.
    #[serde(rename = "tiltX")]
    tilt_x: Option<f64>,
    #[serde(rename = "tiltY")]
    tilt_y: Option<f64>,
    /// A simulated pen's barrel rotation.
    rotation: Option<f64>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
enum PointerKind {
    Down,
    Move,
    Up,
}

/// Strict: an argument the tool doesn't forward is an error, not silently dropped.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointerParams {
    /// Events in document coordinates: `[{"kind":"down|move|up","x":..,"y":..,"pressure":..}]`.
    #[schemars(with = "Vec<PointerEvent>")]
    pub events: Vec<Value>,
    /// Modifier keys, e.g. `{"shift":true}` (also `alt`, `command`, `ctrl`, `space`).
    #[serde(default)]
    pub modifiers: Option<Value>,
    /// Mouse button: `left` (default), or `right` / `secondary`: with the Move tool or
    /// `{"command":true}` it opens the canvas layer menu, with a painting tool the Brush Preset
    /// picker.
    #[serde(default)]
    pub button: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MenuParams {
    /// Menu item / command id.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiSetParams {
    /// Fields for the control method `ui.set`: tool, moveShowTransform (bool, the Move tool's
    /// Show Transform Controls option), panels, dock, dockTabs, dockWidth, colorPanel
    /// ({background: bool} picks which swatch the Color panel edits), maskTarget,
    /// vectorMaskTarget, filterMaskTarget (paint the active smart object's filter mask), selectionMode, zoom, center, rotation (view angle in degrees), fit, theme (pro, proMedium, studio,
    /// studioLight, classic, solarizedDark, adwaita, adwaitaDark, breezeLight, breezeDark, softLight, softDark), brushSection, brushTab,
    /// brushesView, brushPicker ([x, y] opens the Brush Preset picker there, null closes it),
    /// brushPickerName, brushPickerStroke, brushPickerTip (the picker card's parts, at least one
    /// on), brushPickerScale (the picker footer's card-scale slider, 0.15 to 2, 1 the standard),
    /// brushSize, gradientBlendMode
    /// (a blend mode name, for the Gradient tool), gradientClassic (bool), eyedropperSampleSize
    /// ("point" or 1, 3, 5, 11, 31, 51, 101), eyedropperSample (current, currentAndBelow, all,
    /// allNoAdjustments, currentAndBelowNoAdjustments), eyedropperRing (bool), cropOverlay (thirds,
    /// grid, diagonal, triangle, goldenRatio, goldenSpiral), cropOverlayShow (auto, always, never),
    /// cropOverlayOrientation (0..3), cropShield (an object patching show_cropped_area, enabled,
    /// color ("matchCanvas" or "custom"), custom_color ([r, g, b]), opacity (0..100),
    /// auto_adjust, classic_mode and auto_center_preview), cropFill (background or contentAware,
    /// the Crop tool's Fill), penMode (path or shape, the Pen tools' mode), penPathOp (newLayer,
    /// combine, subtract, intersect or exclude: how the next Pen path joins the selected shape
    /// layer or path), shapeStroke (an object patching width, align, cap, join, miterLimit,
    /// dashes, dashOffset and opacity; null disables the stroke). Shape stroke fields use
    /// `shape.edit` names and units; colour comes from `tools.setColors`. frameShape (rectangle,
    /// ellipse, triangle, polygon or custom: the Frame tool's shape), frameSides (3..100, the
    /// polygon's sides), frameTarget (frame, content or both: what the selected frame's handles
    /// edit). camTransformOnDrop (bool,
    /// the Content-Aware Move tool's Transform On Drop). tileGrid (bool, a developer overlay of the
    /// active layer's storage tiles). layerFilter (an object patching by ("kind", "name", "effect",
    /// "mode", "attribute", "color", "smartObject" or "artboard", which lists the artboards that
    /// hold the selected layers), kinds (a list of layer kinds), name (matches part of the layer
    /// name), effect ("bevelEmboss", "stroke", "innerShadow", "innerGlow", "satin", "overlay",
    /// "outerGlow" or "dropShadow"), mode (a blend mode name), attribute ("visible", "locked",
    /// "empty", "linked", "clipped", "layerMask", "vectorMask", "layerEffects",
    /// "advancedBlending", or "notVisible" … "noAdvancedBlending"), color (a layer colour label,
    /// or "none") and smart (a list of "linked", "missing" and "embedded") for the Layers panel
    /// filter). strokeTrace (true, false, "stderr" or "off": the stroke latency trace at runtime; a
    /// CSV file is set at launch with --stroke-trace, not over this channel). updatesOpen (bool,
    /// the Help › Check for Updates window).
    /// histogramPanel (an object patching view ("compact", "expanded" or "allChannels"), channel
    /// ("composite", "red", "green", "blue", "cyan", "magenta", "yellow", "black", "a", "b",
    /// "luminosity" or "colors"), showChannelsInColor, showStatistics and source ("entireImage",
    /// "selectedLayer" or "adjustmentComposite") for the Histogram panel), histogramRange ([from,
    /// to] levels 0..255 whose Count and Percentile the panel shows, or null) and histogramRefresh
    /// (true: an Uncached Refresh); `ui.inspect` reports them and the statistics under
    /// `histogram`. Other fields are an error.
    pub fields: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlParams {
    /// Any control-protocol method, e.g. `ui.dialog.open`.
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Rewrites a tool's input schema into the subset strict tool-schema validators accept (#1782):
/// `$ref`s into `$defs` are inlined and `$defs` dropped, and a `true` schema (any value, as
/// schemars writes for `Value`) becomes `{}`. Tool schemas travel to every provider a client uses,
/// and some reject the whole request over either construct.
fn strict_schema(schema: &mut serde_json::Map<String, Value>) {
    let defs = schema.remove("$defs").and_then(|d| d.as_object().cloned()).unwrap_or_default();
    let mut root = Value::Object(std::mem::take(schema));
    strict_node(&mut root, &defs, 0, true);
    if let Value::Object(m) = root {
        *schema = m;
    }
}

/// `v` (a schema when `is_schema`, else any value that may contain schemas) made strict. Inlining
/// stops at a fixed depth so a recursive definition can't loop.
fn strict_node(v: &mut Value, defs: &serde_json::Map<String, Value>, depth: usize, is_schema: bool) {
    const MAX_DEPTH: usize = 32;
    if is_schema && *v == Value::Bool(true) {
        *v = json!({});
        return;
    }
    let Value::Object(m) = v else {
        if let Value::Array(a) = v {
            for x in a {
                strict_node(x, defs, depth, is_schema);
            }
        }
        return;
    };
    if let Some(def) = m.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix("#/$defs/")).and_then(|name| defs.get(name))
        && depth < MAX_DEPTH
    {
        let mut inlined = def.clone();
        if let Value::Object(im) = &mut inlined {
            // Keep the use site's own annotations (its description) over the definition's.
            for (k, x) in m.iter().filter(|(k, _)| k.as_str() != "$ref") {
                im.insert(k.clone(), x.clone());
            }
        }
        *v = inlined;
        strict_node(v, defs, depth + 1, true);
        return;
    }
    for (k, x) in m.iter_mut() {
        match k.as_str() {
            // Maps of name → schema.
            "properties" | "patternProperties" => {
                if let Value::Object(props) = x {
                    for p in props.values_mut() {
                        strict_node(p, defs, depth, true);
                    }
                }
            }
            "items" | "additionalProperties" | "not" | "anyOf" | "oneOf" | "allOf" | "prefixItems" => strict_node(x, defs, depth, true),
            _ => {}
        }
    }
}

/// Both the bare engine list and older bridge wrappers own their rows already.
fn into_command_rows(mut reply: Value) -> Vec<Value> {
    match reply {
        Value::Array(rows) => rows,
        _ => reply.get_mut("commands").and_then(Value::as_array_mut).map(std::mem::take).unwrap_or_default(),
    }
}

fn ok_json(v: &Value) -> CallToolResult {
    let text = match json_bytes(v).and_then(|bytes| String::from_utf8(bytes).map_err(|error| AutomationError::Other(error.to_string()))) {
        Ok(text) => text,
        Err(error) => return fail(format!("{error}; operation may have completed")),
    };
    bounded_tool_result(CallToolResult::success(vec![Content::text(text)]))
}

fn bounded_tool_result(result: CallToolResult) -> CallToolResult {
    match json_bytes(&result) {
        Ok(_) => result,
        Err(error) => fail(format!("{error}; operation may have completed")),
    }
}

fn fail(e: impl std::fmt::Display) -> CallToolResult {
    let result = CallToolResult::error(vec![Content::text(e.to_string())]);
    if json_bytes(&result).is_err() {
        return CallToolResult::error(vec![Content::text("response budget exceeded; operation may have completed")]);
    }
    result
}

/// Neither backend took the call. The backend is always headless or a bridge, so this only
/// guards against a future backend kind.
fn no_backend() -> CallToolResult {
    fail("internal error: no headless session or app bridge")
}

fn png_result(png: &[u8], note: String) -> CallToolResult {
    if let Err(error) = check_png(png.len()) {
        return fail(error);
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    bounded_tool_result(CallToolResult::success(vec![Content::image(b64, "image/png"), Content::text(note)]))
}

/// Adds the typed byte counts of a memory refusal to an error object: an approval request keeps
/// its soft limit, hard limit and (when known) capacity; a hard refusal keeps its required and
/// hard amounts, because no approval can raise that ceiling, and adds the free memory when
/// measured memory set the limit.
fn attach_memory_error_details(obj: &mut Value, err: &AutomationError) {
    let Some(obj) = obj.as_object_mut() else { return };
    match err {
        AutomationError::Import(magicpic_io::IoError::PsdApprovalRequired { required, soft_limit, hard_limit, available_ram })
        | AutomationError::Engine(magicpic_engine::EngineError::PsdApprovalRequired { required, soft_limit, hard_limit, available_ram, .. }) => {
            obj.insert("required".into(), json!(required));
            obj.insert("soft".into(), json!(soft_limit));
            obj.insert("softLimit".into(), json!(soft_limit));
            obj.insert("hard".into(), json!(hard_limit));
            obj.insert("hardLimit".into(), json!(hard_limit));
            if let Some(avail) = available_ram {
                obj.insert("available".into(), json!(avail));
                obj.insert("availableRam".into(), json!(avail));
            }
        }
        AutomationError::Import(magicpic_io::IoError::PsdImportBudget { required, limit, free_memory })
        | AutomationError::Engine(magicpic_engine::EngineError::PsdHardBudget { required, limit, free_memory, .. }) => {
            obj.insert("required".into(), json!(required));
            obj.insert("hard".into(), json!(limit));
            obj.insert("hardLimit".into(), json!(limit));
            if let Some(free) = free_memory {
                obj.insert("available".into(), json!(free));
                obj.insert("availableRam".into(), json!(free));
            }
        }
        _ => {}
    }
}

fn to_result(r: Result<Value, AutomationError>) -> Result<CallToolResult, McpError> {
    Ok(match r {
        Ok(v) => ok_json(&v),
        Err(e) => {
            let mut obj = json!({"error": e.to_string()});
            attach_memory_error_details(&mut obj, &e);
            let text = serde_json::to_string(&obj).unwrap_or_else(|_| e.to_string());
            CallToolResult::error(vec![Content::text(text)])
        }
    })
}

/// A `doc_place` call's command options, including the explicit high-memory approval.
fn place_options(p: &PlaceParams) -> Value {
    let mut opts = serde_json::Map::new();
    if let Some(v) = p.scale {
        opts.insert("scale".into(), json!(v));
    }
    if let Some(v) = p.fit {
        opts.insert("fit".into(), json!(v));
    }
    if let Some(v) = p.center {
        opts.insert("center".into(), json!(v));
    }
    if p.approve_high_memory {
        opts.insert("approveHighMemory".into(), json!(true));
    }
    Value::Object(opts)
}

fn bridge_only(name: &str) -> Result<CallToolResult, McpError> {
    Ok(fail(format!(
        "`{name}` drives the live GUI and needs bridge mode: start the app with \
         `magicpic --control <port> --control-token-file <path>`, then run \
         `magicpic-cli mcp --bridge 127.0.0.1:<port> --control-token-file <path>`"
    )))
}

impl MagicpicMcp {
    pub fn headless() -> Self {
        Self::with_backend(Backend::Headless(Arc::new(Mutex::new(Headless::new()))))
    }

    pub fn with_headless(headless: Headless) -> Self {
        Self::with_backend(Backend::Headless(Arc::new(Mutex::new(headless))))
    }

    pub fn headless_with_workspace(workspace: AuthorizedWorkspace) -> Self {
        Self::with_headless(Headless::with_workspace(workspace))
    }

    pub fn bridge(addr: &str, token: &str) -> Result<Self, AutomationError> {
        Ok(Self::with_backend(Backend::Bridge(Arc::new(BridgeClient::new(addr, token)?))))
    }

    pub fn with_backend(backend: Backend) -> Self {
        let mut tool_router = Self::tool_router();
        for route in tool_router.map.values_mut() {
            let mut schema = (*route.attr.input_schema).clone();
            strict_schema(&mut schema);
            route.attr.input_schema = Arc::new(schema);
        }
        MagicpicMcp { backend: Arc::new(backend), tool_router }
    }

    /// Serve MCP over stdin/stdout until the client disconnects.
    pub async fn serve_stdio(self) -> Result<(), AutomationError> {
        self.serve_io(tokio::io::stdin(), tokio::io::stdout()).await
    }

    /// Run `f` on the headless session on a blocking thread.
    async fn headless_op<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Headless) -> Result<T, AutomationError> + Send + 'static,
    ) -> Option<Result<T, AutomationError>> {
        let Backend::Headless(h) = &*self.backend else {
            return None;
        };
        let h = h.clone();
        Some(
            tokio::task::spawn_blocking(move || {
                // A panicking command poisons the lock, but edits run on a copy
                // of the document (`Session::edit`), so the session is still
                // consistent: keep serving instead of failing every later call.
                let mut g = h.lock().unwrap_or_else(PoisonError::into_inner);
                g.sync_jobs();
                f(&mut g)
            })
            .await
            .unwrap_or_else(|e| Err(AutomationError::Other(format!("task failed: {e}")))),
        )
    }

    fn bridge_client(&self) -> Option<&BridgeClient> {
        match &*self.backend {
            Backend::Bridge(b) => Some(b),
            Backend::Headless(_) => None,
        }
    }

    async fn screenshot(&self, b: &BridgeClient, max_side: Option<u32>) -> Result<CallToolResult, McpError> {
        if max_side.is_some_and(|side| side > MAX_PREVIEW_SIDE) {
            return Ok(fail(format!("automation preview side exceeds {MAX_PREVIEW_SIDE} pixels")));
        }
        let response = match b.call_read("ui.screenshot", json!({})).await {
            Ok(response) => response,
            Err(error) => return Ok(fail(error)),
        };
        let Some(encoded) = response.get("base64").and_then(Value::as_str) else {
            return Ok(fail("app screenshot response did not contain PNG data"));
        };
        let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
            Ok(bytes) => bytes,
            Err(error) => return Ok(fail(format!("app screenshot data: {error}"))),
        };
        let bytes = match downscale_png(&bytes, max_side.unwrap_or(0)) {
            Ok(Some(small)) => small,
            Ok(None) => bytes,
            Err(error) => return Ok(fail(error)),
        };
        if let Err(error) = check_png(bytes.len()) {
            return Ok(fail(error));
        }
        Ok(png_result(&bytes, "screenshot of the live app window".into()))
    }
}

fn downscale_png(png: &[u8], max_side: u32) -> Result<Option<Vec<u8>>, AutomationError> {
    // A compressed reply's byte limit does not bound its decoded pixel allocation.
    let opts = magicpic_codecs::DecodeOptions {
        limits: magicpic_codecs::Limits { max_width: 8192, max_height: 8192, max_pixels: 16 << 20, max_alloc: 64 << 20 },
        ..Default::default()
    };
    let img = magicpic_codecs::decode_as_with(magicpic_codecs::Format::Png, png, &opts).map_err(|error| AutomationError::Other(error.to_string()))?;
    let (w, h) = img.dimensions();
    if max_side == 0 || w.max(h) <= max_side {
        return Ok(None);
    }
    let s = max_side as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1));
    let src = img.to_rgba8();
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let sx = ((x as f32 + 0.5) / s) as u32;
            let sy = ((y as f32 + 0.5) / s) as u32;
            let si = ((sy.min(h - 1) * w + sx.min(w - 1)) * 4) as usize;
            let di = ((y * nw + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    let small =
        magicpic_codecs::Image::from_u8(nw, nh, magicpic_codecs::ChannelLayout::Rgba, out).map_err(|error| AutomationError::Other(error.to_string()))?;
    let bytes =
        magicpic_codecs::encode(&small, magicpic_codecs::Format::Png, &Default::default()).map_err(|error| AutomationError::Other(error.to_string()))?;
    Ok(Some(bytes))
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[tool_router]
impl MagicpicMcp {
    #[tool(description = "List open documents: {active, documents: [{index, id, name, path, width, height, dirty, revision}]}. \
        `verbose: true` returns the full state instead (bridge mode: the whole UI, about 10k tokens).")]
    async fn session_list(&self, Parameters(p): Parameters<SessionListParams>) -> Result<CallToolResult, McpError> {
        if p.verbose != Some(true) {
            return to_result(self.session_compact().await);
        }
        if let Some(r) = self.headless_op(|h| Ok(h.session_list())).await {
            return to_result(r);
        }
        let Some(b) = self.bridge_client() else {
            return Ok(no_backend());
        };
        to_result(b.call_read("ui.inspect", json!({})).await)
    }

    #[tool(description = "Open an image or document file and make it active. Returns its `index` (tab) and `id` (stable while \
        open; pass either as `document` to other tools), size and import warnings.")]
    async fn doc_open(&self, Parameters(p): Parameters<OpenParams>) -> Result<CallToolResult, McpError> {
        let path = PathBuf::from(&p.path);
        let approve = p.approve_high_memory;
        if let Some(r) = self.headless_op(move |h| h.open_with_approval(&path, approve)).await {
            return to_result(r);
        }
        let Some(b) = self.bridge_client() else {
            return Ok(no_backend());
        };
        let mut params = json!({"path": p.path});
        if approve {
            params["approveHighMemory"] = json!(true);
        }
        let opened = match b.call("app.open", params).await {
            Ok(v) => v,
            Err(e) => return to_result(Err(e)),
        };
        // The app answers with the name only; the opened document is the active one now.
        to_result(Ok(merge_active(opened, &self.session_state().await.unwrap_or(Value::Null))))
    }

    #[tool(
        description = "Place a file into the document as a Smart Object layer (Place Embedded), reading `path` beneath the read root. `scale` (percent), `fit`, `center` and `approve_high_memory` (one-shot PSD/PSB consent, default false) are the command's own."
    )]
    async fn doc_place(&self, Parameters(p): Parameters<PlaceParams>) -> Result<CallToolResult, McpError> {
        let opts = place_options(&p);
        // Bridge mode runs the app on this machine, which reads the path itself like any command.
        let mut bridge_params = opts.clone();
        if let Some(o) = bridge_params.as_object_mut() {
            o.insert("path".into(), Value::String(p.path.clone()));
        }
        let (path, index) = (p.path, p.index);
        if let Some(r) = self.headless_op(move |h| h.place(index, &path, &opts)).await {
            return to_result(r);
        }
        self.run_command("file.placeEmbedded".into(), bridge_params, true).await
    }

    #[tool(
        description = "Replace a Smart Object's contents with the file at `path` beneath the read root, keeping its transform and filters (the capability-scoped Replace Contents). `layer` is the Smart Object's id or name (default: the active layer)."
    )]
    async fn doc_replace_smart_object(&self, Parameters(p): Parameters<ReplaceSmartObjectParams>) -> Result<CallToolResult, McpError> {
        let mut opts = serde_json::Map::new();
        if let Some(v) = p.layer {
            opts.insert("layer".into(), v);
        }
        let opts = Value::Object(opts);
        // Bridge mode runs the app on this machine, which reads the path itself like any command.
        let mut bridge_params = opts.clone();
        if let Some(o) = bridge_params.as_object_mut() {
            o.insert("path".into(), Value::String(p.path.clone()));
        }
        let (path, index) = (p.path, p.index);
        if let Some(r) = self.headless_op(move |h| h.replace_smart_object(index, &path, &opts)).await {
            return to_result(r);
        }
        // The app's command runs on its active document: an `index` it cannot honour is refused
        // rather than dropped, or another document's contents would be replaced.
        if index.is_some() {
            return Ok(fail("`index` is only supported in headless mode; in bridge mode select the document in the app first"));
        }
        self.run_command("layer.smartObjects.replaceContents".into(), bridge_params, true).await
    }

    #[tool(
        description = "Export top-level layers as files (groups merged). Returns files and skipped: [{layer, name, reason}] for hidden layers (visible_only) and adjustments with no pixels of their own. Workspace exports resolve beneath the write root, also return count of written files, and reject existing files instead of overwriting them. Trusted-local/bridge exports use the engine command's file access policy."
    )]
    async fn doc_export_layers(&self, Parameters(p): Parameters<ExportLayersParams>) -> Result<CallToolResult, McpError> {
        let mut opts = serde_json::Map::new();
        if let Some(v) = p.format {
            opts.insert("format".into(), json!(v));
        }
        if let Some(v) = p.prefix {
            opts.insert("prefix".into(), json!(v));
        }
        if let Some(v) = p.visible_only {
            opts.insert("visibleOnly".into(), json!(v));
        }
        if let Some(v) = p.quality {
            opts.insert("quality".into(), json!(v));
        }
        if let Some(v) = p.maximize_compatibility {
            opts.insert("maximizeCompatibility".into(), json!(v));
        }
        let opts = Value::Object(opts);
        // Bridge mode runs the app on this machine, which writes its own paths like any command.
        let mut bridge_params = opts.clone();
        if let Some(o) = bridge_params.as_object_mut() {
            o.insert("dir".into(), Value::String(p.dir.clone()));
        }
        let (dir, index) = (p.dir, p.index);
        if let Some(r) = self.headless_op(move |h| h.export_layers(index, &dir, &opts)).await {
            return to_result(r);
        }
        self.run_command("file.export.layersToFiles".into(), bridge_params, true).await
    }

    #[tool(
        description = "Export every data set of the document as its own flattened file beneath the write root (the capability-scoped Export Data Sets). The batch is planned first, so colliding filenames are refused before any file is written."
    )]
    async fn doc_export_data_sets(&self, Parameters(p): Parameters<ExportDataSetsParams>) -> Result<CallToolResult, McpError> {
        let mut opts = serde_json::Map::new();
        if let Some(v) = p.format {
            opts.insert("format".into(), json!(v));
        }
        if let Some(v) = p.data_sets {
            opts.insert("dataSets".into(), json!(v));
        }
        if let Some(v) = p.naming {
            opts.insert("naming".into(), json!(v));
        }
        let opts = Value::Object(opts);
        let mut bridge_params = opts.clone();
        if let Some(o) = bridge_params.as_object_mut() {
            o.insert("dir".into(), Value::String(p.dir.clone()));
        }
        let (dir, index) = (p.dir, p.index);
        if let Some(r) = self.headless_op(move |h| h.export_data_sets(index, &dir, &opts)).await {
            return to_result(r);
        }
        self.run_command("file.export.dataSetsAsFiles".into(), bridge_params, true).await
    }

    #[tool(description = "Import a data set CSV from the read root into the document (the capability-scoped Import Variable Data Sets).")]
    async fn doc_import_data_sets(&self, Parameters(p): Parameters<ImportDataSetsParams>) -> Result<CallToolResult, McpError> {
        let mut opts = serde_json::Map::new();
        if let Some(v) = p.delimiter {
            opts.insert("delimiter".into(), json!(v));
        }
        let opts = Value::Object(opts);
        let mut bridge_params = opts.clone();
        if let Some(o) = bridge_params.as_object_mut() {
            o.insert("path".into(), Value::String(p.path.clone()));
        }
        let (path, index) = (p.path, p.index);
        if let Some(r) = self.headless_op(move |h| h.import_data_sets(index, &path, &opts)).await {
            return to_result(r);
        }
        self.run_command("file.import.variableDataSets".into(), bridge_params, true).await
    }

    #[tool(description = "Create a new document (defaults: 1920x1080 RGB 8-bit white).")]
    async fn doc_new(&self, Parameters(p): Parameters<NewParams>) -> Result<CallToolResult, McpError> {
        let mut params = serde_json::Map::new();
        if let Some(v) = p.width {
            params.insert("width".into(), v.into());
        }
        if let Some(v) = p.height {
            params.insert("height".into(), v.into());
        }
        if let Some(v) = p.mode {
            params.insert("mode".into(), v.into());
        }
        if let Some(v) = p.depth {
            params.insert("depth".into(), v.into());
        }
        if let Some(v) = p.background {
            params.insert("background".into(), v.into());
        }
        if let Some(v) = p.name {
            params.insert("name".into(), v.into());
        }
        self.run_command("file.new".into(), Value::Object(params), true).await
    }

    #[tool(description = "Save the document. `.pcraft` is the lossless native format (incremental); other extensions \
        (psd, png, jpg, tif, webp, exr, …) export. Without `path` only a PSD, PSB or .pcraft document is written back to its own file. `pcraftFullComposite` requests `composite/full.png`, which may be omitted; check the manifest's `full_composite` member. `document` (index or {id}) picks the document. `quality` and the other options apply to this save only, in both modes; preferences are not changed. Returns warnings about anything the format cannot hold.")]
    async fn doc_save(&self, Parameters(p): Parameters<SaveParams>) -> Result<CallToolResult, McpError> {
        self.save_impl(p).await
    }

    #[tool(description = "Export the document to another format (same as doc_save with an explicit path/format).")]
    async fn doc_export(&self, Parameters(p): Parameters<SaveParams>) -> Result<CallToolResult, McpError> {
        if p.path.is_none() {
            return Ok(fail("doc_export needs `path`"));
        }
        self.save_impl(p).await
    }

    #[tool(description = "Document state as JSON: layer tree (top to bottom), history, selection, active layer.")]
    async fn doc_inspect(&self, Parameters(p): Parameters<DocIndex>) -> Result<CallToolResult, McpError> {
        let p = match self.index_of(p.index, p.document).await {
            Ok(index) => DocIndex { index, document: None },
            Err(e) => return to_result(Err(e)),
        };
        if let Some(r) = self
            .headless_op(move |h| {
                if p.index.is_none() && h.session.active_index().is_none() {
                    Ok(json!({"document": null, "session": h.session_list()}))
                } else {
                    h.inspect(p.index)
                }
            })
            .await
        {
            return to_result(r);
        }
        let Some(b) = self.bridge_client() else {
            return Ok(no_backend());
        };
        let params = p.index.map(|index| json!({"document": index})).unwrap_or_else(|| json!({}));
        to_result(b.call_read("engine.execute", json!({"command": "document.inspect", "params": params})).await)
    }

    #[tool(description = "Look at a document: returns an image of it (the model sees it). `view` shows it as it is (`normal`) or \
        through an observation view that changes nothing (`luminance`, `masses`, `shapes`, `lines`, `color`); `region` renders one \
        rectangle; `index` or `document` picks a document; `layer` (an id) with `solo` shows one layer alone, or with `solo: false` \
        it and the layers below, without changing anything; `format: jpeg` is much smaller. With none of these, headless mode \
        returns the flattened PNG and bridge mode a screenshot of the app window.")]
    async fn doc_render_preview(&self, Parameters(mut p): Parameters<PreviewParams>) -> Result<CallToolResult, McpError> {
        match self.index_of(p.index, p.document.take()).await {
            Ok(index) => p.index = index,
            Err(e) => return Ok(fail(e)),
        }
        if p.solo.is_some() && p.layer.is_none() {
            return Ok(fail("`solo` needs `layer` (a layer id from doc_inspect)"));
        }
        let max = p.max_side.unwrap_or(1024);
        // A view, a region or another format is the engine's `seeing.view`, in either backend;
        // so is naming a document in bridge mode, where the plain preview is the app window.
        if p.wants_view() || (p.index.is_some() && self.bridge_client().is_some()) {
            return Ok(self.document_view(p).await);
        }
        if let Some(r) = self
            .headless_op(move |h| {
                let res = h.render_png(p.index, max);
                for d in h.drain_diagnostics() {
                    // Ignore a failed stderr write: the completed render must still be returned.
                    let _ = writeln!(std::io::stderr(), "warning: {d}");
                }
                res
            })
            .await
        {
            return Ok(match r {
                Ok(png) => png_result(&png, format!("flattened preview (max side {max})")),
                Err(e) => fail(e),
            });
        }
        let Some(b) = self.bridge_client() else {
            return Ok(no_backend());
        };
        self.screenshot(b, Some(max)).await
    }

    #[tool(description = "Look at a document (same as doc_render_preview): an image of it as it is, of one `region`, of one `layer` \
        (`solo`: alone, default; false: with the layers below), or through an observation `view` (`luminance`, `masses`, `shapes`, \
        `lines`, `color`); none of these change the document or its history. Bridge mode without `view`, `region`, `layer`, \
        `index`, `document` or `format` returns the app window screenshot.")]
    async fn render_preview(&self, p: Parameters<PreviewParams>) -> Result<CallToolResult, McpError> {
        self.doc_render_preview(p).await
    }

    #[tool(description = "Read-only: the histogram of a document (its composite), of one `layer` (alone, or with the layers below \
        when `solo` is false) and/or a `region`, at full resolution: per channel (luminosity, red, green, blue, alpha) 256 bins on \
        the 0-255 levels scale plus count, min, max, mean, stdDev, median and percentiles p50/p90/p99/p99.9. Use it to choose \
        Levels or Threshold values from numbers. Changes nothing.")]
    async fn doc_histogram(&self, Parameters(p): Parameters<HistogramParams>) -> Result<CallToolResult, McpError> {
        let index = match self.index_of(None, p.document).await {
            Ok(i) => i,
            Err(e) => return to_result(Err(e)),
        };
        let mut params = serde_json::Map::new();
        if let Some(i) = index {
            params.insert("document".into(), json!(i));
        }
        if let Some(l) = p.layer {
            params.insert("layer".into(), json!(l));
        }
        if let Some(solo) = p.solo {
            params.insert("solo".into(), json!(solo));
        }
        if let Some(c) = p.channel {
            params.insert("channel".into(), json!(c));
        }
        if let Some(r) = p.region {
            params.insert("region".into(), json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}));
        }
        let params = Value::Object(params);
        let headless_params = params.clone();
        if let Some(r) = self.headless_op(move |h| h.command_run("document.histogram", headless_params)).await {
            return to_result(r);
        }
        let Some(b) = self.bridge_client() else {
            return Ok(no_backend());
        };
        to_result(b.call_read("engine.execute", json!({"command": "document.histogram", "params": params})).await)
    }

    #[tool(description = "Headless mode: list the files and folders in a read root (or in `dir` below one), newest first, \
        with size and modification time. Use it to find a picture the person just saved into the folder, then doc_open its \
        `path` (or its `absolutePath` when two roots hold the same name).")]
    async fn files_list(&self, Parameters(p): Parameters<FilesListParams>) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(50).clamp(1, 500) as usize;
        match self.headless_op(move |h| h.list_files(p.dir.as_deref().filter(|d| !d.is_empty()), limit)).await {
            Some(r) => to_result(r),
            None => Ok(fail(
                "`files_list` is only available in headless mode: in bridge mode the app owns the folders. Open the picture in the app \
                 (drag it in, or File > Open), or pass its path to doc_open",
            )),
        }
    }

    #[tool(description = "Make a document active, by `index` or `id` (from session_list or doc_open). In bridge mode its tab \
        comes to the front, as Window › <document> does. Returns the compact session.")]
    async fn doc_select(&self, Parameters(p): Parameters<SelectParams>) -> Result<CallToolResult, McpError> {
        let r = match (p.index, p.id) {
            (None, None) => return Ok(fail("doc_select needs `index` or `id`")),
            (index, id) => DocRef::Selector(DocSelector { index, id }),
        };
        if let Err(e) = self.bring_to_front(r).await {
            return to_result(Err(e));
        }
        to_result(self.session_compact().await)
    }

    #[tool(description = "Close a document (default: the active one) without saving.")]
    async fn doc_close(&self, Parameters(p): Parameters<DocIndex>) -> Result<CallToolResult, McpError> {
        match self.headless_op(move |h| h.close(p.index)).await {
            Some(r) => to_result(r),
            None => bridge_only_headless("doc_close"),
        }
    }

    #[tool(description = "List engine commands. Returns a JSON array (no wrapper object) of {id, label, menu: [path], \
        shortcut: string|null, params: doc string, enabled: bool, disabledReason: string|null, needsDocument: bool}. \
        `disabledReason` says why a command is off (null when enabled); `needsDocument` is true when it is off because no \
        document is open: create or open one first (doc_new, doc_open). `brief: true` keeps only {id, label, enabled}; \
        `offset` / `limit` page the rows and wrap them as {total, offset, next, commands}.")]
    async fn command_list(&self, Parameters(p): Parameters<ListParams>) -> Result<CallToolResult, McpError> {
        let all = if let Some(r) = self.headless_op(|h| Ok(h.command_list())).await {
            r
        } else {
            let Some(b) = self.bridge_client() else {
                return Ok(no_backend());
            };
            b.call_read("engine.commands", json!({})).await
        };
        let all = match all {
            Ok(v) => v,
            Err(e) => return Ok(fail(e)),
        };
        let needle = p.filter.map(|f| f.to_lowercase());
        let enabled_only = p.enabled_only.unwrap_or(false);
        let items: Vec<Value> = into_command_rows(all)
            .into_iter()
            .filter(|c| {
                needle.as_ref().is_none_or(|n| magicpic_engine::commands::list_entry_matches(c, n))
                    && (!enabled_only || c.get("enabled").and_then(Value::as_bool).unwrap_or(true))
            })
            .map(|c| if p.brief == Some(true) { json!({"id": c.get("id"), "label": c.get("label"), "enabled": c.get("enabled")}) } else { c })
            .collect();
        Ok(ok_json(&page_rows(items, p.offset, p.limit)))
    }

    #[tool(description = "Run an engine command by id with JSON params (see command_list). Returns the command's JSON result. \
        Long commands (filters, Content-Aware Fill/Scale, Photomerge, brush import) wait to finish unless `wait` is false: \
        then they run as a background job and the result is {job: id} (see jobs_list, jobs_cancel). A waited long \
        command reports progress, cancelling the request cancels it, and other calls are served meanwhile. \
        `expect_revision` and `expect_document` refuse to run if the document changed since that revision or is not the active one. \
        `document` (index or {id}) runs it on that document, made active first. A second text block names the document the \
        command left active: {document: id, index, name, revision, dirty}.")]
    async fn command_run(&self, Parameters(p): Parameters<RunParams>, context: RequestContext<RoleServer>) -> Result<CallToolResult, McpError> {
        if (p.expect_revision.is_some() || p.expect_document.is_some()) && p.wait == Some(false) {
            return Ok(fail("`expect_revision` and `expect_document` run the command to the end; they cannot be combined with `wait: false`"));
        }
        if let Err(e) = self.target(p.document, p.wait, p.expect_document).await {
            return to_result(Err(e));
        }
        let mut waiter = Waiter::new(Some(context));
        let (id, params, guarded) = Self::guarded(p.id, p.params.unwrap_or_else(|| json!({})), p.expect_revision, p.expect_document);
        let result = self.step(id, params, p.wait.unwrap_or(true), &mut waiter, (0, 1)).await;
        let result = to_result(if guarded { result.map(unwrap_guarded) } else { result });
        Ok(self.with_document(result?).await)
    }

    #[tool(description = "List background jobs: running ones with progress (0-1), message and elapsed time, then the last few \
        that ended (state done|failed|cancelled with their result or error). Finished jobs are applied first.")]
    async fn jobs_list(&self) -> Result<CallToolResult, McpError> {
        self.jobs_call("jobs.list", json!({}), true).await
    }

    #[tool(description = "Cancel a background job by id (or every running job when `job` is omitted). The document is left \
        unchanged.")]
    async fn jobs_cancel(&self, Parameters(p): Parameters<JobCancelParams>) -> Result<CallToolResult, McpError> {
        self.jobs_call("jobs.cancel", p.job.map_or_else(|| json!({}), |j| json!({"job": j})), false).await
    }

    #[tool(description = "Run several engine commands in one call, in order (fewer round trips). Returns {completed, failed, \
        results:[{ok, result|error}]}; stops at the first error unless stop_on_error is false. \
        Not an atomic transaction: earlier successful steps stay applied (no rollback). \
        With `atomic: true` it is one: a failing step rolls every earlier step back, and `expect_revision` / `expect_document` \
        refuse to run if the document changed since that revision or is not the active one. `document` (index or {id}) runs the \
        batch on that document; a step's own `document` switches before that step. The reply's `document`, `index` and \
        `revision` say which document the batch left active and its revision.")]
    async fn command_batch(&self, Parameters(p): Parameters<BatchParams>, context: RequestContext<RoleServer>) -> Result<CallToolResult, McpError> {
        self.batch(p, Waiter::new(Some(context))).await
    }

    // ----- live-GUI tools (bridge mode) -----

    #[tool(
        description = "Bridge mode: full UI state of the live app (tool, panels, views, dialogs, windows). For the menu tree, call `control_call` with method `ui.menu.list`."
    )]
    async fn ui_inspect(&self) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => to_result(b.call_read("ui.inspect", json!({})).await),
            None => bridge_only("ui_inspect"),
        }
    }

    #[tool(description = "Bridge mode: screenshot of the live app window as PNG.")]
    async fn ui_screenshot(&self, Parameters(p): Parameters<ScreenshotParams>) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => self.screenshot(b, p.max_side).await,
            None => bridge_only("ui_screenshot"),
        }
    }

    #[tool(
        description = "Bridge mode: send pointer events (document coordinates) to the active tool; with a Color Picker open they sample the image into it instead."
    )]
    async fn ui_pointer(&self, Parameters(p): Parameters<PointerParams>) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => {
                let mut params = json!({"events": p.events});
                if let Some(m) = p.modifiers {
                    params["modifiers"] = m;
                }
                if let Some(button) = p.button {
                    params["button"] = button.into();
                }
                to_result(b.call("ui.pointer", params).await)
            }
            None => bridge_only("ui_pointer"),
        }
    }

    #[tool(description = "Bridge mode: activate a menu item by id.")]
    async fn ui_menu_invoke(&self, Parameters(p): Parameters<MenuParams>) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => to_result(b.call("ui.menu.invoke", json!({"id": p.id})).await),
            None => bridge_only("ui_menu_invoke"),
        }
    }

    #[tool(
        description = "Bridge mode: change UI state (tool, panels, dock, zoom, center, fit, theme, brush settings; see `fields`). Unknown fields are an error."
    )]
    async fn ui_set(&self, Parameters(p): Parameters<UiSetParams>) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => to_result(b.call("ui.set", p.fields).await),
            None => bridge_only("ui_set"),
        }
    }

    #[tool(description = "Bridge mode: call any control-protocol method (docs/control-protocol.md) with raw params.")]
    async fn control_call(&self, Parameters(p): Parameters<ControlParams>) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => to_result(b.call(&p.method, p.params.unwrap_or_else(|| json!({}))).await),
            None => bridge_only("control_call"),
        }
    }
}

/// An `app.open` reply with the opened (now active) document's `index`, `id`, size and layers.
fn merge_active(mut opened: Value, session: &Value) -> Value {
    let active = session.get("active").and_then(Value::as_u64);
    let doc = session.get("documents").and_then(Value::as_array).and_then(|d| d.iter().find(|d| d.get("index").and_then(Value::as_u64) == active));
    if let (Some(obj), Some(doc)) = (opened.as_object_mut(), doc) {
        for key in ["index", "id", "width", "height"] {
            if let Some(v) = doc.get(key) {
                obj.entry(key).or_insert_with(|| v.clone());
            }
        }
    }
    opened
}

/// `command_list` rows, paged when `offset` or `limit` is given.
fn page_rows(rows: Vec<Value>, offset: Option<usize>, limit: Option<usize>) -> Value {
    if offset.is_none() && limit.is_none() {
        return Value::Array(rows);
    }
    let total = rows.len();
    let start = offset.unwrap_or(0).min(total);
    let page: Vec<Value> = rows.into_iter().skip(start).take(limit.unwrap_or(usize::MAX)).collect();
    let end = start + page.len();
    json!({"total": total, "offset": start, "next": (end < total).then_some(end), "commands": page})
}

/// The command's own result out of a one-step `document.transaction` reply.
fn unwrap_guarded(mut reply: Value) -> Value {
    reply.get_mut("results").and_then(Value::as_array_mut).and_then(|r| r.first_mut()).map(Value::take).unwrap_or(reply)
}

fn bridge_only_headless(name: &str) -> Result<CallToolResult, McpError> {
    Ok(fail(format!("`{name}` is only available in headless mode (in the GUI use the window/tab UI or control_call)")))
}

impl MagicpicMcp {
    /// The index `index` or `document` names (at most one), or `None` for the active document.
    async fn index_of(&self, index: Option<usize>, document: Option<DocRef>) -> Result<Option<usize>, AutomationError> {
        match targets::one_ref(index, document)? {
            None => Ok(None),
            // An index is checked where it is used; only an id needs the session.
            Some(DocRef::Index(i) | DocRef::Selector(DocSelector { index: Some(i), id: None })) => Ok(Some(i)),
            Some(r) => self.resolve(r).await.map(|(i, _)| Some(i)),
        }
    }

    /// Before a command: make `document` active. Checks it against `expect_document`.
    async fn target(&self, document: Option<DocRef>, wait: Option<bool>, expect_document: Option<u64>) -> Result<(), AutomationError> {
        let Some(r) = document else { return Ok(()) };
        if wait == Some(false) {
            return Err(AutomationError::BadRequest(
                "`document` runs the command on that document to the end; it cannot be combined with `wait: false` (doc_select it first)".into(),
            ));
        }
        let (_, id) = self.bring_to_front(r).await?;
        match expect_document {
            Some(expected) if expected != id => {
                Err(AutomationError::BadRequest(format!("`document` names document {id} but `expect_document` is {expected}; nothing was run")))
            }
            _ => Ok(()),
        }
    }

    /// A successful command result with a second text block naming the active document.
    async fn with_document(&self, mut result: CallToolResult) -> CallToolResult {
        if result.is_error == Some(true) {
            return result;
        }
        let summary = self.active_summary().await;
        result.content.push(Content::text(summary.to_string()));
        bounded_tool_result(result)
    }

    /// `seeing.view` of a document as image content, in either backend.
    async fn document_view(&self, p: PreviewParams) -> CallToolResult {
        let params = p.view_params();
        let index = p.index;
        let reply = match self
            .headless_op({
                let params = params.clone();
                move |h| h.seeing_view(index, params)
            })
            .await
        {
            Some(r) => r,
            None => {
                let mut params = params;
                if let (Some(i), Some(obj)) = (index, params.as_object_mut()) {
                    obj.insert("document".into(), json!(i));
                }
                match self.bridge_client() {
                    // A view applies nothing, so one retry on a fresh connection is safe.
                    Some(b) => b.call_read("engine.execute", json!({"command": "seeing.view", "params": params})).await,
                    None => return no_backend(),
                }
            }
        };
        let v = match reply {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let (Some(encoded), Some(mime)) = (v.get("base64").and_then(Value::as_str), v.get("mime").and_then(Value::as_str)) else {
            return fail("the view reply did not contain image data");
        };
        // Base64 is 4 characters per 3 bytes: the same ceiling as a PNG preview.
        if let Err(error) = check_png(encoded.len() / 4 * 3) {
            return fail(format!("the image is too large for one reply ({error}); pass a smaller `max_side`, a `region`, or `format: \"jpeg\"`"));
        }
        let mut note = v.clone();
        if let Some(obj) = note.as_object_mut() {
            obj.remove("base64");
        }
        bounded_tool_result(CallToolResult::success(vec![Content::image(encoded.to_string(), mime.to_string()), Content::text(note.to_string())]))
    }

    /// A command that must only run on a known document at a known revision becomes a one-step
    /// transaction; the flag says whether the reply then needs unwrapping ([`unwrap_guarded`]).
    fn guarded(id: String, params: Value, expect_revision: Option<u64>, expect_document: Option<u64>) -> (String, Value, bool) {
        if expect_revision.is_none() && expect_document.is_none() {
            return (id, params, false);
        }
        let mut transaction = json!({"steps": [{"id": id, "params": params}]});
        if let Some(obj) = transaction.as_object_mut() {
            if let Some(revision) = expect_revision {
                obj.insert("expectRevision".into(), json!(revision));
            }
            if let Some(document) = expect_document {
                obj.insert("expectDocument".into(), json!(document));
            }
        }
        ("document.transaction".into(), transaction, true)
    }

    async fn run_command(&self, id: String, params: Value, wait: bool) -> Result<CallToolResult, McpError> {
        to_result(self.step(id, params, wait, &mut Waiter::new(None), (0, 1)).await)
    }

    /// Run one command in either backend; `at` is its place in a batch, for progress.
    async fn step(&self, id: String, params: Value, wait: bool, waiter: &mut Waiter, at: (usize, usize)) -> Result<Value, AutomationError> {
        let none = || Err(AutomationError::Other("internal error: no headless session or app bridge".into()));
        match &*self.backend {
            Backend::Headless(_) if wait => self.headless_wait(id, params, waiter, at).await.unwrap_or_else(none),
            Backend::Headless(_) => self.headless_op(move |h| h.command_start(&id, params, false)).await.unwrap_or_else(none),
            Backend::Bridge(b) if wait => self.bridge_wait(b, id, params, waiter, at).await,
            Backend::Bridge(b) => b.call("engine.execute", json!({"command": id, "params": params, "wait": false})).await,
        }
    }

    async fn batch(&self, mut p: BatchParams, mut waiter: Waiter) -> Result<CallToolResult, McpError> {
        if p.steps.len() > MAX_BATCH_STEPS {
            return Ok(fail(format!("batch contains {} steps; maximum is {MAX_BATCH_STEPS}", p.steps.len())));
        }
        if let Some(r) = p.document {
            match self.bring_to_front(r).await {
                Ok((_, id)) if p.atomic == Some(true) => match p.expect_document {
                    Some(expected) if expected != id => {
                        return Ok(fail(format!("`document` names document {id} but `expect_document` is {expected}; nothing was run")));
                    }
                    _ => p.expect_document = Some(id),
                },
                Ok(_) => {}
                Err(e) => return to_result(Err(e)),
            }
        }
        if p.atomic == Some(true) {
            return Ok(self.atomic_batch(p, &mut waiter).await);
        }
        if p.expect_revision.is_some() || p.expect_document.is_some() {
            return Ok(fail("`expect_revision` and `expect_document` on a batch need `atomic: true` (or put them on a step)"));
        }
        let stop = p.stop_on_error.unwrap_or(true);
        let total = p.steps.len();
        let mut results = Vec::new();
        let mut failed = 0;
        // The reply travels as escaped JSON text, so steps are charged their escaped size.
        let mut reply_budget = BatchReplyBudget::escaped();
        for (i, s) in p.steps.into_iter().enumerate() {
            // A cancelled request runs no further steps; the client no longer reads the reply.
            if waiter.is_cancelled() {
                break;
            }
            let response = if (s.expect_revision.is_some() || s.expect_document.is_some()) && s.wait == Some(false) {
                Err(AutomationError::BadRequest(
                    "`expect_revision` and `expect_document` run the command to the end; they cannot be combined with `wait: false`".into(),
                ))
            } else if let Err(e) = self.target(s.document, s.wait, s.expect_document).await {
                Err(e)
            } else {
                let (id, params, guarded) = Self::guarded(s.id, s.params.unwrap_or_else(|| json!({})), s.expect_revision, s.expect_document);
                let response = self.step(id, params, s.wait.unwrap_or(true), &mut waiter, (i, total)).await;
                if guarded { response.map(unwrap_guarded) } else { response }
            };
            let was_error = response.is_err();
            let result = match response {
                Ok(value) => json!({"ok": true, "result": value}),
                Err(error) => {
                    let mut obj = json!({"ok": false, "error": error.to_string()});
                    attach_memory_error_details(&mut obj, &error);
                    obj
                }
            };
            if let Err(error) = reply_budget.charge(&result) {
                failed += 1;
                results.push(json!({"ok": false, "error": format!("batch response budget exceeded; current step may have completed: {error}")}));
                break;
            }
            results.push(result);
            if was_error {
                failed += 1;
                if stop {
                    break;
                }
            }
            waiter.report((i + 1) as f64, total as f64, "").await;
        }
        let after = self.active_summary().await;
        Ok(ok_json(&json!({
            "completed": results.len() - failed,
            "failed": failed,
            "results": results,
            "document": after.get("document"),
            "index": after.get("index"),
            "revision": after.get("revision"),
        })))
    }

    /// `command_batch {atomic: true}`: the steps as one `document.transaction`.
    async fn atomic_batch(&self, p: BatchParams, waiter: &mut Waiter) -> CallToolResult {
        if p.stop_on_error == Some(false) {
            return fail("an atomic batch stops and rolls back at the first error; omit `stop_on_error: false`");
        }
        if let Some(step) = p.steps.iter().find(|s| s.wait == Some(false) || s.expect_revision.is_some() || s.expect_document.is_some() || s.document.is_some())
        {
            return fail(format!(
                "step `{}`: an atomic batch runs every step to the end on one document; `wait: false` and a step's own `expect_revision`, `expect_document` or `document` are not allowed (give the batch a `document`)",
                step.id
            ));
        }
        let total = p.steps.len();
        let steps: Vec<Value> = p.steps.into_iter().map(|s| json!({"id": s.id, "params": s.params.unwrap_or_else(|| json!({}))})).collect();
        let mut params = json!({"steps": steps});
        if let Some(obj) = params.as_object_mut() {
            if let Some(revision) = p.expect_revision {
                obj.insert("expectRevision".into(), json!(revision));
            }
            if let Some(document) = p.expect_document {
                obj.insert("expectDocument".into(), json!(document));
            }
        }
        match self.step("document.transaction".into(), params, true, waiter, (0, 1)).await {
            Ok(v) => {
                let results: Vec<Value> =
                    v.get("results").and_then(Value::as_array).map(|r| r.iter().map(|x| json!({"ok": true, "result": x})).collect()).unwrap_or_default();
                let index = self.active_summary().await.get("index").cloned();
                let reply = json!({"completed": total, "failed": 0, "atomic": true, "outcome": "applied", "revision": v.get("revision"), "document": v.get("document"), "index": index, "results": results});
                match json_bytes(&reply) {
                    Ok(_) => ok_json(&reply),
                    // The edit is applied; only its results are too large to send back.
                    Err(_) => ok_json(
                        &json!({"completed": total, "failed": 0, "atomic": true, "outcome": "applied", "revision": v.get("revision"), "document": v.get("document"),
                        "results": [], "note": "every step was applied; the step results are too large for one reply (inspect the document)"}),
                    ),
                }
            }
            Err(error) => {
                // Only the engine's own message says what happened to the document. Anything
                // else (a lost or late bridge reply) leaves the outcome unknown, and the reply
                // must not claim that nothing ran.
                let text = error.to_string();
                let outcome = if text.contains("Nothing was run") || text.contains("was not sent") {
                    "not_run"
                } else if text.contains("was rolled back") {
                    "rolled_back"
                } else {
                    "unknown"
                };
                let known = outcome != "unknown";
                ok_json(&json!({
                    "completed": if known { json!(0) } else { Value::Null },
                    "failed": 1,
                    "atomic": true,
                    "outcome": outcome,
                    "results": [{"ok": false, "error": if known { text } else { format!("{text}; the batch either applied completely or not at all: inspect the document before retrying") }}],
                }))
            }
        }
    }

    /// `jobs.list` / `jobs.cancel` in either backend; `read` picks the bridge's retry rule.
    async fn jobs_call(&self, method: &'static str, params: Value, read: bool) -> Result<CallToolResult, McpError> {
        let p2 = params.clone();
        if let Some(r) = self.headless_op(move |h| h.command_start(method, p2, true)).await {
            return to_result(r);
        }
        let Some(b) = self.bridge_client() else {
            return Ok(no_backend());
        };
        if read { to_result(b.call_read(method, params).await) } else { to_result(b.call(method, params).await) }
    }

    async fn save_impl(&self, mut p: SaveParams) -> Result<CallToolResult, McpError> {
        let target = match targets::one_ref(p.index.take(), p.document.take()) {
            Ok(t) => t,
            Err(e) => return to_result(Err(e)),
        };
        let tga_bits = match p.tga_bits.map(|b| (b, magicpic_io::TgaBits::from_bits(u64::from(b)))) {
            None => None,
            Some((_, Some(bits))) => Some(bits),
            Some((b, None)) => return Ok(fail(format!("`tgaBits` must be 24 or 32, got {b}"))),
        };
        let dpx_bits = match p.dpx_bits.map(|b| (b, magicpic_io::DpxBits::from_bits(u64::from(b)))) {
            None => None,
            Some((_, Some(bits))) => Some(bits),
            Some((b, None)) => return Ok(fail(format!("`dpxBits` must be 8, 10, 12 or 16, got {b}"))),
        };
        let tiff_compression = match magicpic_engine::file_cmds::tiff_compression_param(&json!({"tiffCompression": p.tiff_compression}), "save") {
            Ok(c) => c,
            Err(e) => return Ok(fail(e.to_string())),
        };
        if let Some(q) = p.quality.filter(|q| !(1..=100).contains(q)) {
            return Ok(fail(format!("`quality` must be a whole number from 1 to 100, got {q}")));
        }
        let dds_compression = match p.dds_compression.as_deref().map(|c| (c, magicpic_io::DdsCompression::from_name(c))) {
            None => None,
            Some((_, Some(c))) => Some(c),
            Some((c, None)) => return Ok(fail(format!("`ddsCompression` must be none, bc1, bc2 or bc3, got {c:?}"))),
        };
        if let Some(b) = self.bridge_client() {
            // The bridge forwards to the running app's `app.save`. It takes an optional path
            // (without one it writes back to the document's own layered file, as headless does)
            // and this save's answers: quality (`exportQuality`, 1–100), the Targa bits, the DDS
            // options, the TIFF layers, compression, transparency and pyramid, the .pcraft full
            // composite and Maximize Compatibility. Preferences are not changed. DPX bits and PDF
            // spot plates are headless-only: saying so beats saving with defaults while the caller
            // believes their option was applied.
            let unsupported: Vec<&str> =
                [dpx_bits.is_some().then_some("dpxBits"), p.pdf_spot_plates.is_some().then_some("pdfSpotPlates")].into_iter().flatten().collect();
            if !unsupported.is_empty() {
                return Ok(fail(format!("bridge mode cannot set `{}` on a save; it needs headless mode", unsupported.join("`, `"))));
            }
            // The app picks the format from the extension: a different `format` would be ignored.
            if let Some(format) = &p.format {
                let ext = p.path.as_deref().and_then(|path| std::path::Path::new(path).extension()).and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
                let wanted = format.trim_start_matches('.').to_ascii_lowercase();
                let same = ext
                    .as_deref()
                    .is_some_and(|e| e == wanted || matches!((e, wanted.as_str()), ("jpeg", "jpg") | ("jpg", "jpeg") | ("tiff", "tif") | ("tif", "tiff")));
                if !same {
                    return Ok(fail(format!("in bridge mode the extension of `path` picks the format: give a path ending in .{wanted}")));
                }
            }
            if let Some(r) = target
                && let Err(e) = self.bring_to_front(r).await
            {
                return to_result(Err(e));
            }
            let mut params = json!({});
            if let Some(q) = p.quality {
                params["exportQuality"] = json!(q);
            }
            if p.tiff_layers {
                params["tiffLayers"] = json!(true);
            }
            if let Some(full) = p.pcraft_full_composite {
                params["pcraftFullComposite"] = json!(full);
            }
            if let Some(path) = p.path {
                params["path"] = json!(path);
            }
            if let Some(bits) = tga_bits {
                params["tgaBits"] = json!(bits.bits());
            }
            if let Some(c) = dds_compression {
                params["ddsCompression"] = json!(c.name());
            }
            if p.dds_mipmaps {
                params["ddsMipmaps"] = json!(true);
            }
            // The app's TIFF Options answer, as its Save As takes it.
            if let Some(c) = &p.tiff_compression {
                params["tiffCompression"] = json!(c);
            }
            if let Some(t) = p.tiff_transparency {
                params["tiffTransparency"] = json!(t);
            }
            if p.tiff_pyramid {
                params["tiffPyramid"] = json!(true);
            }
            if let Some(maximize) = p.maximize_compatibility {
                params["maximizeCompatibility"] = json!(maximize);
            }
            return to_result(b.call("app.save", params).await);
        }
        let index = match target {
            Some(r) => match self.resolve(r).await {
                Ok((i, _)) => Some(i),
                Err(e) => return to_result(Err(e)),
            },
            None => None,
        };
        let Some(r) = self
            .headless_op(move |h| {
                let mut opts = magicpic_io::ExportOptions {
                    tiff_layers: p.tiff_layers,
                    tiff_transparency: p.tiff_transparency,
                    tga_bits,
                    pdf_spot_plates: p.pdf_spot_plates.unwrap_or(true),
                    pcraft_full_composite: p.pcraft_full_composite.unwrap_or(false),
                    ..Default::default()
                };
                opts.encode.dpx_bits = dpx_bits;
                if let Some(c) = tiff_compression {
                    opts.encode.tiff_compression = c;
                }
                opts.encode.tiff_pyramid = p.tiff_pyramid;
                opts.encode.dds = magicpic_io::DdsOptions { compression: dds_compression.unwrap_or_default(), mipmaps: p.dds_mipmaps };
                if let Some(maximize) = p.maximize_compatibility {
                    opts.psd_maximize_compatibility = maximize;
                }
                if let Some(q) = p.quality {
                    opts.encode.jpeg_quality = q;
                    opts.encode.webp_quality = q;
                    opts.encode.webp_lossless = false;
                }
                let path = p.path.map(PathBuf::from);
                let res = h.save(index, path.as_deref(), p.format.as_deref(), &opts);
                for d in h.drain_diagnostics() {
                    // Ignore a failed stderr write: the completed save must still be returned.
                    let _ = writeln!(std::io::stderr(), "warning: {d}");
                }
                res
            })
            .await
        else {
            return Ok(no_backend());
        };
        to_result(r)
    }
}

#[cfg(test)]
mod agent_tests;
mod conventions;
mod progress;
mod targets;
mod transport;

use progress::Waiter;

/// Used by the render helper in tests and the CLI.
pub fn render_document_png(doc: &magicpic_doc::Document, max_side: u32) -> Result<Vec<u8>, AutomationError> {
    files::render_png(doc, max_side)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_rows_keep_bare_and_wrapped_replies_compatible() {
        let rows = json!([{"id":"first","params":{"nested":[1,2]}}, {"id":"second","enabled":false}]);
        assert_eq!(into_command_rows(rows.clone()), rows.as_array().unwrap().clone());
        assert_eq!(into_command_rows(json!({"commands":rows,"other":"ignored"})), rows.as_array().unwrap().clone());
        for reply in [json!(null), json!(false), json!(3), json!("invalid"), json!({}), json!({"commands":null}), json!({"commands":{}}), json!([])] {
            assert!(into_command_rows(reply).is_empty());
        }
    }

    #[test]
    fn command_rows_reuse_the_owned_array_and_nested_strings() {
        for wrapped in [false, true] {
            let rows = json!([{"id":"first","params":"documentation".repeat(64)}]);
            let array = rows.as_array().unwrap().as_ptr();
            let text = rows[0]["params"].as_str().unwrap().as_ptr();
            let reply = if wrapped { Value::Object([("commands".into(), rows)].into_iter().collect()) } else { rows };
            let moved = into_command_rows(reply);
            assert_eq!(moved.as_ptr(), array, "wrapped={wrapped}: move the original allocation");
            assert_eq!(moved[0]["params"].as_str().unwrap().as_ptr(), text, "wrapped={wrapped}: retain nested allocations");
        }
    }

    #[test]
    fn strict_schema_inlines_refs_and_stops_on_recursive_definitions() {
        // A definition that refers to itself: inlining stops at a depth instead of looping.
        let mut s = json!({
            "type": "object",
            "properties": {
                "node": {"$ref": "#/$defs/Node", "description": "use-site text"},
                "any": true,
                "list": {"type": "array", "items": true}
            },
            "$defs": {"Node": {"type": "object", "description": "def text", "properties": {"child": {"$ref": "#/$defs/Node"}}}}
        });
        let m = s.as_object_mut().unwrap();
        strict_schema(m);
        assert!(m.get("$defs").is_none());
        let props = &m["properties"];
        assert_eq!(props["node"]["type"], "object");
        assert_eq!(props["node"]["description"], "use-site text", "the use site's description wins");
        assert_eq!(props["node"]["properties"]["child"]["type"], "object", "nested refs inline too");
        assert_eq!((&props["any"], &props["list"]["items"]), (&json!({}), &json!({})));
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.len() < 1 << 20, "bounded: {} bytes", text.len());
    }

    #[test]
    fn mcp_text_checks_the_encoded_tool_envelope() {
        // JSON escaping in the outer text-content envelope also consumes the budget.
        let result = ok_json(&json!("\n".repeat(crate::budgets::MAX_RESPONSE_BYTES / 3)));
        assert_eq!(result.is_error, Some(true));
        assert!(json_bytes(&result).is_ok());
        let error = fail("x".repeat(crate::budgets::MAX_RESPONSE_BYTES));
        assert_eq!(error.is_error, Some(true));
        assert!(json_bytes(&error).is_ok());
    }

    #[test]
    fn bridge_screenshot_decode_limits_apply_even_without_downscaling() {
        let image = magicpic_codecs::Image::from_u8(8193, 1, magicpic_codecs::ChannelLayout::Rgba, vec![0; 8193 * 4]).unwrap();
        let png = magicpic_codecs::encode(&image, magicpic_codecs::Format::Png, &Default::default()).unwrap();
        assert!(downscale_png(b"not a png", 32).is_err());
        assert!(downscale_png(&png, 0).is_err());
        assert!(downscale_png(&png, 1024).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_reports_invalid_filter_parameters_without_changing_history() {
        let mcp = MagicpicMcp::headless();
        mcp.headless_op(|h| h.command_run("file.new", json!({"width":8,"height":8}))).await.unwrap().unwrap();
        let before = mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap();
        for (params, key) in
            [(json!({"radius":"big"}), "radius"), (json!({"radus":3}), "radus"), (json!({"radius":5000}), "radius"), (json!({"radius":-4}), "radius")]
        {
            for wait in [true, false] {
                let result = mcp.run_command("filter.blur.gaussianBlur".into(), params.clone(), wait).await.unwrap();
                assert_eq!(result.is_error, Some(true));
                let error = &result.content.first().and_then(|c| c.as_text()).unwrap().text;
                assert!(error.contains("filter.blur.gaussianBlur") && error.contains(key), "{error}");
                assert_eq!(mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap(), before);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_panicking_command_does_not_wedge_the_session() {
        let mcp = MagicpicMcp::headless();
        let r = mcp.headless_op(|_| -> Result<(), AutomationError> { panic!("boom") }).await.unwrap();
        assert!(r.is_err());
        let r = mcp.headless_op(|h| h.command_run("file.new", json!({"width": 8, "height": 8}))).await.unwrap();
        assert!(r.is_ok(), "session still usable after a panic: {r:?}");
    }

    /// Regression (#504): the batch budget counted plain JSON, but MCP sends the reply as escaped
    /// text, so a batch the budget stopped could still exceed the tool-result ceiling and the
    /// client lost every step result.
    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_command_batch_stopped_by_the_budget_keeps_its_step_results() {
        let mcp = MagicpicMcp::headless();
        let step = |id: &str, params: Value| RunParams {
            id: id.into(),
            params: Some(params),
            wait: None,
            expect_revision: None,
            expect_document: None,
            document: None,
        };
        let mut steps: Vec<RunParams> = (0..MAX_BATCH_STEPS - 1).map(|_| step("command.list", json!({}))).collect();
        steps.push(step("file.new", json!({"width": 8, "height": 8})));
        let result = mcp
            .batch(
                BatchParams { steps, stop_on_error: Some(false), atomic: None, expect_revision: None, expect_document: None, document: None },
                Waiter::new(None),
            )
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{:?}", result.content.first().and_then(|c| c.as_text()).map(|t| &t.text[..200.min(t.text.len())]));
        let text = &result.content.first().and_then(|c| c.as_text()).unwrap().text;
        let reply: Value = serde_json::from_str(text).unwrap();
        let results = reply["results"].as_array().unwrap();
        assert!(results.last().unwrap()["error"].as_str().unwrap().contains("batch response budget exceeded"), "{}", results.last().unwrap());
        assert_eq!(reply["completed"].as_u64().unwrap() as usize, results.len() - 1);
        assert_eq!(reply["failed"], 1);
        let docs = mcp.headless_op(|h| Ok(h.session.documents().len())).await.unwrap().unwrap();
        assert_eq!(docs, 0, "the step after the budget ran out did not run");
    }

    /// rmcp may drop a cancelled request's future without polling it again; its job must still
    /// stop and leave the document unchanged (#8129).
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_a_waited_command_cancels_its_job() {
        let mcp = MagicpicMcp::headless();
        mcp.headless_op(|h| {
            h.command_run("file.new", json!({"width": 6000, "height": 6000}))?;
            h.command_run("filter.noise.addNoise", json!({"amount": 50}))
        })
        .await
        .unwrap()
        .unwrap();
        let before = mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap();
        let mut waiter = Waiter::new(None);
        let waited = mcp.headless_wait("filter.blur.gaussianBlur".into(), json!({"radius": 1000}), &mut waiter, (0, 1));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(100), waited).await.is_err(), "the blur ended before it was dropped");
        let t = std::time::Instant::now();
        loop {
            let (running, cancelled) =
                mcp.headless_op(|h| Ok((h.session.has_jobs(), h.session.jobs_with_recent().iter().any(|j| j.state == "cancelled")))).await.unwrap().unwrap();
            if !running {
                assert!(cancelled, "the job ended without being cancelled");
                break;
            }
            assert!(t.elapsed().as_secs() < 30, "the dropped job kept running");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap(), before);
    }

    /// Dropped while its start still waits for the session lock (held here as a long synchronous
    /// command would): the job that starts afterwards is cancelled too, not left running.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_wait_dropped_before_its_job_starts_cancels_the_job() {
        let mcp = MagicpicMcp::headless();
        mcp.headless_op(|h| {
            h.command_run("file.new", json!({"width": 6000, "height": 6000}))?;
            h.command_run("filter.noise.addNoise", json!({"amount": 50}))
        })
        .await
        .unwrap()
        .unwrap();
        let before = mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap();
        let Backend::Headless(session) = &*mcp.backend else { panic!("headless") };
        let (session, (locked, is_locked)) = (session.clone(), std::sync::mpsc::channel());
        let holder = std::thread::spawn(move || {
            let _held = session.lock().unwrap();
            locked.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
        });
        is_locked.recv().unwrap();
        let mut waiter = Waiter::new(None);
        let waited = mcp.headless_wait("filter.blur.gaussianBlur".into(), json!({"radius": 1000}), &mut waiter, (0, 1));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), waited).await.is_err(), "the start waits for the lock");
        holder.join().unwrap();
        let t = std::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let (started, running) = mcp.headless_op(|h| Ok((!h.session.jobs_with_recent().is_empty(), h.session.has_jobs()))).await.unwrap().unwrap();
            if started && !running {
                break;
            }
            assert!(t.elapsed().as_secs() < 30, "the job kept running");
        }
        let cancelled = mcp.headless_op(|h| Ok(h.session.jobs_with_recent().iter().any(|j| j.state == "cancelled"))).await.unwrap().unwrap();
        assert!(cancelled, "the job ended without being cancelled");
        assert_eq!(mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap(), before);
    }

    /// Cancellation notifications only cancel the token: the handler remains alive while
    /// its blocking start waits for the session lock.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_queued_commands_never_start() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mcp = MagicpicMcp::headless();
        mcp.headless_op(|h| h.command_run("file.new", json!({"width": 8, "height": 8}))).await.unwrap().unwrap();
        let before = mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap();
        let Backend::Headless(session) = &*mcp.backend else { panic!("headless") };
        let session = session.clone();
        let (locked, acquired) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _held = session.lock().unwrap();
            locked.send(()).unwrap();
            released.recv().unwrap();
        });
        acquired.recv().unwrap();

        let (mut input, server_in) = tokio::io::duplex(65536);
        let (server_out, output) = tokio::io::duplex(65536);
        let server = tokio::spawn(mcp.clone().serve_io(server_in, server_out));
        let mut lines = BufReader::new(output).lines();
        async fn send(input: &mut tokio::io::DuplexStream, value: Value) {
            input.write_all(format!("{value}\n").as_bytes()).await.unwrap();
        }
        send(&mut input, json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"protocolVersion":"2025-06-18", "capabilities":{}, "clientInfo":{"name":"test", "version":"1"}}})).await;
        lines.next_line().await.unwrap().unwrap();
        send(&mut input, json!({"jsonrpc":"2.0", "method":"notifications/initialized"})).await;
        for (id, command, params) in [(2, "layer.new.layer", json!({})), (3, "filter.blur.gaussianBlur", json!({"radius": 10}))] {
            send(
                &mut input,
                json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":"command_run", "arguments":{"id":command, "params":params}}}),
            )
            .await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        for id in [2, 3] {
            send(&mut input, json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":id}})).await;
        }
        send(&mut input, json!({"jsonrpc":"2.0", "id":4, "method":"ping"})).await;
        loop {
            let reply: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            if reply["id"] == 4 {
                break;
            }
        }
        release.send(()).unwrap();
        holder.join().unwrap();
        // Let both blocked starts acquire the mutex before inspecting their effects.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (after, jobs) = mcp.headless_op(|h| Ok((h.inspect(None)?, h.session.jobs_with_recent()))).await.unwrap().unwrap();
        assert_eq!(after, before);
        assert!(jobs.is_empty(), "cancelled queued job started: {jobs:?}");
        drop(input);
        server.await.unwrap().unwrap();
    }

    /// Answer the bridge's challenge authentication on one fake-app connection, frame by frame,
    /// as the real control servers do. Returns whether the connection authenticated.
    async fn fake_app_handshake<R, W>(lines: &mut tokio::io::Lines<R>, write: &mut W, token: &str) -> bool
    where
        R: tokio::io::AsyncBufRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        use tokio::io::AsyncWriteExt;
        let mut handshake = crate::security::ServerHandshake::new();
        while let Ok(Some(line)) = lines.next_line().await {
            let (reply, step) = handshake.reply(&line, token);
            write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            if step != crate::security::AuthStep::Challenged {
                return step == crate::security::AuthStep::Authenticated;
            }
        }
        false
    }

    /// The same over the bridge (#8130): a dropped wait sends `jobs.cancel` for the app's job.
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_a_bridged_wait_cancels_the_apps_job() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (cancels, mut cancelled) = tokio::sync::mpsc::unbounded_channel();
        let polled = Arc::new(tokio::sync::Notify::new());
        let polling = polled.clone();
        // A fake app whose job 3 never ends and whose poll never replies. Dropping the wait
        // discards that connection, so the cancellation must arrive on a fresh one (#8130).
        let app = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let (read, mut write) = socket.into_split();
                let mut lines = tokio::io::BufReader::new(read).lines();
                fake_app_handshake(&mut lines, &mut write, TOKEN).await;
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let result = match request["method"].as_str().unwrap() {
                        "engine.execute" => json!({"job": 3, "pending": true}),
                        "jobs.list" => {
                            polling.notify_one();
                            continue;
                        }
                        "jobs.cancel" => json!({"cancelled": 1}),
                        method => panic!("unexpected method {method}"),
                    };
                    write.write_all(format!("{}\n", json!({"id": request["id"], "ok": true, "result": result})).as_bytes()).await.unwrap();
                    if request["method"] == "jobs.cancel" {
                        cancels.send(request).unwrap();
                        return;
                    }
                }
            }
        });
        let mcp = MagicpicMcp::with_backend(Backend::Bridge(Arc::new(BridgeClient::new(addr, TOKEN).unwrap())));
        let mut waited = Box::pin(mcp.run_command("filter.blur.gaussianBlur".into(), json!({"radius": 4}), true));
        tokio::select! {
            result = &mut waited => panic!("the running job answered before its wait was dropped: {result:?}"),
            ready = tokio::time::timeout(std::time::Duration::from_secs(10), polled.notified()) => ready.expect("the app received the poll"),
        }
        drop(waited);
        let cancel = tokio::time::timeout(std::time::Duration::from_secs(10), cancelled.recv()).await.unwrap().unwrap();
        assert_eq!((&cancel["method"], &cancel["params"]), (&json!("jobs.cancel"), &json!({"job": 3})));
        tokio::time::timeout(std::time::Duration::from_secs(10), app).await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_doc_open_schema_and_structured_error() {
        let schema = &MagicpicMcp::tool_router().map["doc_open"].attr.input_schema;
        assert_eq!(schema["properties"]["approve_high_memory"]["type"], "boolean");
        let omitted: OpenParams = serde_json::from_value(json!({"path": "file.psd"})).unwrap();
        assert!(!omitted.approve_high_memory);
        let approved: OpenParams = serde_json::from_value(json!({"path": "file.psd", "approve_high_memory": true})).unwrap();
        assert!(approved.approve_high_memory);
        for invalid in [Value::Null, json!("true"), json!(1)] {
            assert!(serde_json::from_value::<OpenParams>(json!({"path": "file.psd", "approve_high_memory": invalid})).is_err());
        }

        let err = AutomationError::Import(magicpic_io::IoError::PsdApprovalRequired {
            required: 6000,
            soft_limit: 3000,
            hard_limit: 9000,
            available_ram: Some(12000),
        });
        let res = to_result(Err(err)).unwrap();
        assert_eq!(res.is_error, Some(true));
        let text = &res.content.first().and_then(|c| c.as_text()).unwrap().text;
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["required"], 6000);
        assert_eq!(v["soft"], 3000);
        assert_eq!(v["hard"], 9000);
        assert_eq!(v["available"], 12000);
    }

    /// #10897: the MCP `doc_place` tool takes the same strict, defaulted approval as `doc_open`
    /// and forwards it as the command's `approveHighMemory` option. A hard refusal keeps its
    /// typed `required`/`hardLimit` fields and claims nothing about a soft limit or capacity.
    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_doc_place_schema_approval_and_structured_hard_error() {
        let schema = &MagicpicMcp::tool_router().map["doc_place"].attr.input_schema;
        assert_eq!(schema["properties"]["approve_high_memory"]["type"], "boolean");
        let omitted: PlaceParams = serde_json::from_value(json!({"path": "file.psd"})).unwrap();
        assert!(!omitted.approve_high_memory);
        assert!(place_options(&omitted).get("approveHighMemory").is_none(), "false is not forwarded");
        let approved: PlaceParams = serde_json::from_value(json!({"path": "file.psd", "approve_high_memory": true})).unwrap();
        assert!(approved.approve_high_memory);
        assert_eq!(place_options(&approved)["approveHighMemory"], true);
        let explicit_false: PlaceParams = serde_json::from_value(json!({"path": "file.psd", "approve_high_memory": false})).unwrap();
        assert!(!explicit_false.approve_high_memory);
        for invalid in [Value::Null, json!("true"), json!(1)] {
            assert!(serde_json::from_value::<PlaceParams>(json!({"path": "file.psd", "approve_high_memory": invalid})).is_err(), "{invalid}");
        }

        let err =
            AutomationError::Engine(magicpic_engine::EngineError::PsdHardBudget { name: "huge.psd".into(), required: 9_000, limit: 8_000, free_memory: None });
        let res = to_result(Err(err)).unwrap();
        assert_eq!(res.is_error, Some(true));
        let text = &res.content.first().and_then(|c| c.as_text()).unwrap().text;
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["required"], 9_000);
        assert_eq!(v["hard"], 8_000);
        assert_eq!(v["hardLimit"], 8_000);
        for absent in ["soft", "softLimit", "available", "availableRam"] {
            assert!(v.get(absent).is_none(), "a hard refusal has no {absent}: {v}");
        }

        // With measured free memory behind the limit, the refusal reports it too.
        let err = AutomationError::Engine(magicpic_engine::EngineError::PsdHardBudget {
            name: "huge.psd".into(),
            required: 9_000,
            limit: 8_000,
            free_memory: Some(10_000),
        });
        let res = to_result(Err(err)).unwrap();
        let text = &res.content.first().and_then(|c| c.as_text()).unwrap().text;
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["hardLimit"], 8_000);
        assert_eq!(v["available"], 10_000);
        assert_eq!(v["availableRam"], 10_000);
        assert!(v.get("soft").is_none(), "a hard refusal has no soft limit: {v}");
    }

    /// The MCP server applies finished jobs before every tool call too (#503).
    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_tools_see_a_finished_background_job() {
        let mcp = MagicpicMcp::headless();
        mcp.headless_op(|h| {
            h.command_run("file.new", json!({"width": 64, "height": 48}))?;
            h.command_run("filter.noise.addNoise", json!({"amount": 50}))?;
            h.command_start("filter.blur.gaussianBlur", json!({"radius": 4}), false)
        })
        .await
        .unwrap()
        .unwrap();
        let t = std::time::Instant::now();
        loop {
            let inspected = mcp.headless_op(|h| h.inspect(None)).await.unwrap().unwrap();
            if inspected["history"].as_array().unwrap().last().unwrap() == "Gaussian Blur" {
                break;
            }
            assert!(t.elapsed().as_secs() < 60, "doc_inspect never saw the finished job: {inspected}");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}
