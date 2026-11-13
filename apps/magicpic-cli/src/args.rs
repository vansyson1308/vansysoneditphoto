//! The command line, as clap types. Values are parsed into their types here, so a bad one is a
//! usage error naming its flag (exit 2) and the handlers never see it (#7071). A flag a
//! subcommand doesn't take is a usage error too, so a typo such as `--fromat` can't be silently
//! ignored (#423), and so is a repeated one: `--quality 80 --quality 60` is more likely a mistake
//! than a choice. A value starting with `-` is written `--out=-image.png` (or `./-image.png`), so a
//! value flag never takes the next flag as its value; negative numbers are kept where they mean
//! something.

use std::ffi::OsString;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

pub(crate) use crate::gpu::GpuArgs;
use clap::error::ErrorKind;
use clap::parser::ValueSource;
use clap::{ArgAction, ArgGroup, ArgMatches, Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use magicpic_automation::args::{AutomationArgs, ControlArgs, blank_env_is_unset, conflict, control_port, name_env_vars};
use magicpic_codecs::{JpegFormat, TiffCompression};
use magicpic_io::{DdsCompression, DpxBits, TgaBits, XmpEmbed};
use serde_json::Value;

/// magicpic-cli: headless MagicPic
#[derive(Parser)]
#[command(name = "magicpic-cli", version = magicpic_engine::build_info::long_version_static(), arg_required_else_help = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    Convert(ConvertArgs),
    Info(InfoArgs),
    Run(RunArgs),
    Batch(BatchArgs),
    Droplet(DropletArgs),
    Commands(CommandsArgs),
    Mcp(McpArgs),
    Serve(ServeArgs),
    Setup(SetupArgs),
    Launch(LaunchArgs),
    Doctor(DoctorArgs),
    Install(InstallArgs),
    QuitApp(QuitAppArgs),
    /// Start a program detached and write its pid; what `mcp register --restart` and `launch` use
    /// on Windows. Not for direct use
    #[command(hide = true)]
    SpawnDetached(SpawnDetachedArgs),
}

// How an image is written: shared by `convert`, `run` and `batch`.
#[derive(Args)]
pub(crate) struct ExportArgs {
    /// Output format, overriding the output's extension (png, jpg, tif, webp, …)
    #[arg(long, value_name = "EXT")]
    pub(crate) format: Option<String>,
    /// JPEG or WebP quality, 1-100. A WebP written with a quality is lossy, without one lossless
    // allow_negative_numbers makes `--quality -1` an out-of-range value rather than an unexpected argument.
    #[arg(long, value_name = "1-100", value_parser = clap::value_parser!(u8).range(1..=100), allow_negative_numbers = true)]
    pub(crate) quality: Option<u8>,
    /// Keep the layers in TIFF output (Photoshop layer data); TIFF output is flat without it
    #[arg(long)]
    pub(crate) tiff_layers: bool,
    /// Compress TIFF output: none (the default), lzw or zip
    #[arg(long, value_name = "none|lzw|zip", value_parser = tiff_compression)]
    pub(crate) tiff_compression: Option<TiffCompression>,
    /// TIFF Save Transparency: on keeps the transparency (ahead of any alpha or spot channels),
    /// off composites the image over white. Without it: kept unless the document has alpha or
    /// spot channels
    #[arg(long, value_name = "on|off", value_parser = on_off)]
    pub(crate) tiff_transparency: Option<bool>,
    /// Also write a TIFF image pyramid: half-size copies after the full image
    #[arg(long)]
    pub(crate) tiff_pyramid: bool,
    /// A TGA's bits per pixel: 32 writes the alpha channel (or else the transparency) as its
    /// alpha, 24 none. Without it: 32 with an alpha channel or transparency, else 24
    #[arg(long, value_name = "24|32", value_parser = tga_bits)]
    pub(crate) tga_bits: Option<TgaBits>,
    /// A DPX's bits per sample. Without it: 8 for an 8-bit document, else 16
    #[arg(long, value_name = "8|10|12|16", value_parser = dpx_bits)]
    pub(crate) dpx_bits: Option<DpxBits>,
    /// A DDS texture's compression: none (uncompressed, the default), bc1 (DXT1: alpha on or
    /// off), bc2 (DXT3) or bc3 (DXT5)
    #[arg(long, value_name = "none|bc1|bc2|bc3", value_parser = dds_compression)]
    pub(crate) dds_compression: Option<DdsCompression>,
    /// Also write a DDS texture's full mip chain
    #[arg(long)]
    pub(crate) dds_mipmaps: bool,
    /// A JPEG's scans, as JPEG Options picks them: baseline (the default), optimized (Huffman
    /// tables fitted to the image; RGB is written 4:2:0 and keeps the standard tables, so only Gray
    /// and CMYK shrink) or progressive
    #[arg(long, value_name = "baseline|optimized|progressive", value_parser = jpeg_format)]
    pub(crate) jpeg_format: Option<JpegFormat>,
    /// Request the flattened document at its canvas size in .pcraft output as
    /// composite/full.png; it may be omitted. Check the manifest's full_composite member.
    #[arg(long)]
    pub(crate) pcraft_full_composite: bool,
    /// A layered PSD/PSB stores Photoshop's white placeholder instead of the merged image
    /// (Maximize Compatibility off): a smaller file and a faster save, but readers that show only
    /// the merged image see white. Bitmap, Indexed Color and Duotone documents still write the
    /// real image
    #[arg(long)]
    pub(crate) no_maximize_compatibility: bool,
    /// The document metadata a flat export embeds: all (the default) keeps the document's XMP
    /// packet and free-form text where the format holds them, as Save As does; none leaves them
    /// out, as Export As's Metadata: None. Nothing else changes: the ICC profile, EXIF and
    /// resolution are written as they are without it. PSD, PSB and .pcraft always keep the XMP
    #[arg(long, value_name = "none|all", value_parser = metadata)]
    pub(crate) metadata: Option<XmpEmbed>,
}

/// Convert between formats (.pcraft, .psd, .png, .jpg, .tif, .webp, .exr, …)
///
/// An <OUT> that is the <IN> file is refused, as the result would replace the original;
/// --in-place allows it.
#[derive(Args)]
pub(crate) struct ConvertArgs {
    /// The file to read
    #[arg(value_name = "IN")]
    pub(crate) input: PathBuf,
    /// The file to write; its extension picks the format unless --format is given
    #[arg(value_name = "OUT")]
    pub(crate) output: PathBuf,
    /// Allow an <OUT> that is the <IN> file, replacing the original
    #[arg(long)]
    pub(crate) in_place: bool,
    /// Explicitly approve high-memory PSD/PSB imports within the hard memory budget
    #[arg(long)]
    pub(crate) approve_high_memory: bool,
    /// PDF import resolution in DPI (default 300)
    #[arg(long, value_name = "DPI", value_parser = pdf_dpi)]
    pub(crate) pdf_dpi: Option<f32>,
    /// PDF pages to import, 1-based: "3" or "2-7" (default all)
    #[arg(long)]
    pub(crate) pdf_pages: Option<String>,
    /// Import PDF paths and embedded-font text as editable layers (experimental; unsupported
    /// pages fall back to raster, and embedded fonts may change when edited or reopened)
    #[arg(long)]
    pub(crate) pdf_editable: bool,
    #[command(flatten)]
    pub(crate) gpu: GpuArgs,
    #[command(flatten)]
    pub(crate) export: ExportArgs,
}

/// Print the document as JSON (size, mode, depth, layer tree)
#[derive(Args)]
pub(crate) struct InfoArgs {
    pub(crate) file: PathBuf,
    /// One line of JSON instead of pretty-printed
    #[arg(long)]
    pub(crate) compact: bool,
    /// Explicitly approve high-memory PSD/PSB imports within the hard memory budget
    #[arg(long)]
    pub(crate) approve_high_memory: bool,
    /// Also list every text run and paragraph run, anti-aliasing and full layer-effect settings
    #[arg(long)]
    pub(crate) detail: bool,
}

/// Open a file (or start a new document), run engine commands in order, save the result
///
/// Prints each command's JSON result. Give exactly one of <FILE> or --new, and at least one --cmd
/// or --out. An --out that is the <FILE> is refused, as the result would replace the original;
/// --in-place allows it.
#[derive(Args)]
#[command(group(ArgGroup::new("source").args(["file", "new"]).required(true)))]
#[command(group(ArgGroup::new("work").args(["cmd", "out"]).required(true).multiple(true)))]
pub(crate) struct RunArgs {
    /// The document to open
    pub(crate) file: Option<PathBuf>,
    /// Start from a new document with these JSON parameters instead of opening a file
    #[arg(long, value_name = "JSON", value_parser = json)]
    pub(crate) new: Option<Value>,
    /// An engine command to run; repeat for several, run in order
    #[arg(long, value_name = "ID", action = ArgAction::Append)]
    pub(crate) cmd: Vec<String>,
    /// JSON parameters for the --cmd before it (default {})
    #[arg(long, value_name = "JSON", action = ArgAction::Append, value_parser = json)]
    pub(crate) params: Vec<Value>,
    /// Save the result to this file
    #[arg(long, value_name = "FILE")]
    pub(crate) out: Option<PathBuf>,
    /// Allow an --out that is the <FILE>, replacing the original
    #[arg(long)]
    pub(crate) in_place: bool,
    /// Explicitly approve high-memory PSD/PSB imports within the hard memory budget
    #[arg(long)]
    pub(crate) approve_high_memory: bool,
    #[command(flatten)]
    pub(crate) gpu: GpuArgs,
    #[command(flatten)]
    pub(crate) export: ExportArgs,
    /// `--cmd` and `--params` values in command-line order, set by [`parse`].
    #[arg(skip)]
    pub(crate) steps: Vec<Step>,
}

/// One `--cmd` or `--params` of `run`, in command-line order.
pub(crate) enum Step {
    Cmd(String),
    Params(Value),
}

/// Apply an action list to every image in a directory
///
/// A Photoshop Actions file (.atn) is a set: its action --action names plays, as below. JSON steps
/// are [id, params] pairs, {"command": id, "params": {…}} objects or bare ids, as a recorded
/// action or droplet stores them (a list, or wrapped in {"actions": …}, {"steps": …} or a
/// droplet). An Actions set (the Actions panel's actions.json, {"actions": [{"name", "steps"}…]})
/// plays the action --action names (needed when the set holds more than one) with the whole set
/// loaded, so its steps can play other actions of the set.
#[derive(Args)]
pub(crate) struct BatchArgs {
    /// The action list: actions.json, a droplet, or a Photoshop Actions file (.atn)
    #[arg(long, value_name = "FILE")]
    pub(crate) actions: PathBuf,
    /// Which action of an Actions set to play
    #[arg(long, value_name = "NAME")]
    pub(crate) action: Option<String>,
    /// The folder of images to process
    #[arg(long = "in", value_name = "DIR")]
    pub(crate) in_dir: PathBuf,
    /// The folder to write the results to, as <stem>.<ext>
    #[arg(long, value_name = "DIR")]
    pub(crate) out: PathBuf,
    /// An --out folder that is the --in folder is refused, as the results would replace the
    /// originals; --in-place allows it
    #[arg(long)]
    pub(crate) in_place: bool,
    /// Include images in subfolders; every result goes straight into --out, so a later file of
    /// the same name replaces an earlier one
    #[arg(long)]
    pub(crate) recursive: bool,
    /// Name each result with Batch tokens such as {name}, {serial}, {date}, and {ext}
    #[arg(long, value_name = "TEMPLATE")]
    pub(crate) naming: Option<String>,
    /// First serial number used by --naming
    #[arg(long, value_name = "1-4294967295", value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) start_serial: Option<u32>,
    /// Stop after the first failed input
    #[arg(long)]
    pub(crate) stop_on_error: bool,
    /// Write failed inputs and reasons to this text file
    #[arg(long, value_name = "FILE")]
    pub(crate) error_log: Option<PathBuf>,
    /// Explicitly approve high-memory PSD/PSB imports within the hard memory budget
    #[arg(long)]
    pub(crate) approve_high_memory: bool,
    #[command(flatten)]
    pub(crate) gpu: GpuArgs,
    #[command(flatten)]
    pub(crate) export: ExportArgs,
}

/// Run a droplet (File › Automate › Create Droplet) on images and folders
#[derive(Args)]
pub(crate) struct DropletArgs {
    /// The droplet to run
    #[arg(value_name = "FILE.pcdroplet")]
    pub(crate) file: String,
    /// Images and folders to run it on
    #[arg(value_name = "FILE-OR-DIR", required = true)]
    pub(crate) inputs: Vec<String>,
    /// The folder to write the results to
    #[arg(long, value_name = "DIR")]
    pub(crate) out: Option<String>,
}

/// List the engine command registry
#[derive(Args)]
pub(crate) struct CommandsArgs {
    /// Print the entries as JSON
    #[arg(long)]
    pub(crate) json: bool,
    /// Only the commands matching this text
    #[arg(long, value_name = "TEXT", allow_negative_numbers = true)]
    pub(crate) filter: Option<String>,
}

/// Run the MCP server on stdio (headless engine, or bridge to a running `magicpic --control <port>`)
///
/// Headless: each automation root grants that direction of file access; an omitted root denies it.
/// Prefer absolute root paths in MCP client configurations. Bridge: set the roots on the desktop
/// app; root flags are refused. The token flags require --bridge (or MAGICPIC_CONTROL_PORT).
#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct McpArgs {
    #[command(subcommand)]
    pub(crate) action: Option<McpAction>,
    /// Bridge to the MagicPic app listening here instead of a headless engine; a bare port means
    /// 127.0.0.1:<PORT>
    #[arg(long, value_name = "127.0.0.1:PORT", env = "MAGICPIC_CONTROL_PORT", value_parser = bridge)]
    pub(crate) bridge: Option<Bridge>,
    /// Bridge: when nothing listens on the port, start the app with the launcher settings that
    /// `magicpic-cli setup` recorded, then connect
    #[arg(long, requires = "bridge")]
    pub(crate) start_app_if_needed: bool,
    /// `--bridge` came from MAGICPIC_CONTROL_PORT, set by [`parse`].
    #[arg(skip)]
    pub(crate) bridge_from_env: bool,
    #[command(flatten)]
    pub(crate) gpu: GpuArgs,
    #[command(flatten)]
    pub(crate) control: ControlArgs,
    #[command(flatten)]
    pub(crate) automation: AutomationArgs,
}

/// Register MagicPic's MCP servers with an agent client, remove them, or show them
#[derive(Subcommand)]
pub(crate) enum McpAction {
    Register(RegisterArgs),
    Unregister(UnregisterArgs),
    Status(StatusArgs),
}

/// An agent client whose MCP configuration `mcp register` edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub(crate) enum Client {
    /// Claude Desktop, the chat app (claude_desktop_config.json)
    ClaudeDesktop,
    /// Claude Code at user scope (~/.claude.json, or $CLAUDE_CONFIG_DIR)
    ClaudeCode,
    /// Codex (~/.codex/config.toml, or $CODEX_HOME)
    Codex,
}

impl Client {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Client::ClaudeDesktop => "claude-desktop",
            Client::ClaudeCode => "claude-code",
            Client::Codex => "codex",
        }
    }
}

#[derive(Args)]
pub(crate) struct ClientArgs {
    /// Which client's configuration to change
    #[arg(long, value_enum)]
    pub(crate) client: Client,
    /// Edit this file instead of the client's own (tests, unusual installs)
    #[arg(long, value_name = "FILE")]
    pub(crate) config: Option<PathBuf>,
    /// Claude Desktop: if it is running, wait in the background for it to quit, then write the
    /// file and open it again (Windows Store builds through their app id)
    #[arg(long)]
    pub(crate) restart: bool,
    /// Claude Desktop: write even though it is running (it may overwrite the change)
    #[arg(long, conflicts_with = "restart")]
    pub(crate) force: bool,
    /// Minutes `--restart` waits for every Claude Desktop process to quit (default 60)
    #[arg(long, value_name = "MINUTES", value_parser = clap::value_parser!(u64).range(1..=1440), conflicts_with = "timeout")]
    pub(crate) wait_minutes: Option<u64>,
    /// The same in seconds (older spelling of --wait-minutes)
    #[arg(long, value_name = "SECONDS", hide = true)]
    pub(crate) timeout: Option<u64>,
    /// The detached helper `--restart` starts; not for direct use
    #[arg(long, hide = true)]
    pub(crate) detached_wait: bool,
    /// Which pending registration the helper owns; not for direct use
    #[arg(long, hide = true, requires = "detached_wait")]
    pub(crate) pending_id: Option<String>,
}

/// Add the `magicpic` (files) and `magicpic-app` (open window) servers to a client's config
///
/// Merges only those two entries: other servers and settings are kept. The file is backed up as
/// `<name>.magicpic-backup-<time>` and written as UTF-8 without a byte order mark. Paths come from
/// `magicpic-cli setup` (config.json); the flags override them.
#[derive(Args)]
pub(crate) struct RegisterArgs {
    #[command(flatten)]
    pub(crate) client: ClientArgs,
    /// The one photo folder MagicPic may read and write
    #[arg(long, value_name = "DIR")]
    pub(crate) photos: Option<PathBuf>,
    /// The app's control port (default: setup's, else 7878)
    #[arg(long, value_name = "PORT")]
    pub(crate) port: Option<u16>,
    /// Don't let `magicpic-app` start the app when it is not running
    #[arg(long)]
    pub(crate) no_start_app: bool,
}

/// Remove the `magicpic` and `magicpic-app` servers from a client's config
#[derive(Args)]
pub(crate) struct UnregisterArgs {
    #[command(flatten)]
    pub(crate) client: ClientArgs,
}

/// Show which clients have the servers, and whether their paths still exist
#[derive(Args)]
pub(crate) struct StatusArgs {
    /// Only this client
    #[arg(long, value_enum)]
    pub(crate) client: Option<Client>,
    /// Read this file instead of the client's own (with --client)
    #[arg(long, value_name = "FILE", requires = "client")]
    pub(crate) config: Option<PathBuf>,
}

/// Create the photo folder, ~/.magicpic, the launcher and config.json; print the paths as JSON
#[derive(Args)]
pub(crate) struct SetupArgs {
    /// The one photo folder MagicPic may read and write (default ~/Pictures/MagicPic)
    #[arg(long, value_name = "DIR")]
    pub(crate) photos: Option<PathBuf>,
    /// The desktop app (default: next to this program, else ~/.magicpic/bin)
    #[arg(long, value_name = "FILE")]
    pub(crate) app: Option<PathBuf>,
    /// The app's control port on 127.0.0.1 (default 7878)
    #[arg(long, value_name = "PORT")]
    pub(crate) port: Option<u16>,
    /// Also put a MagicPic shortcut on the desktop
    #[arg(long)]
    pub(crate) desktop_shortcut: bool,
    /// Start the app afterwards and wait until it listens
    #[arg(long)]
    pub(crate) start: bool,
    /// Seconds --start waits for the app (default 30)
    #[arg(long, value_name = "SECONDS")]
    pub(crate) wait: Option<u64>,
    /// Clean up an older install that `legacy` lists: point registrations that still run an older
    /// magicpic-cli at this one, then remove the old programs and launcher. Ask the person first
    #[arg(long)]
    pub(crate) migrate: bool,
}

/// Copy magicpic and magicpic-cli from a folder (a release archive or `target/release`) into
/// ~/.magicpic/bin, while they may be running
///
/// Asks a running app to quit first and refuses, changing nothing, while it has unsaved documents.
/// A program still in use (magicpic-cli held by an agent client's MCP servers) is renamed to
/// `<name>.old-<time>` and replaced; the old copies are deleted on a later run. Prints JSON
#[derive(Args)]
pub(crate) struct InstallArgs {
    /// The folder with the new magicpic and magicpic-cli
    #[arg(long, value_name = "DIR")]
    pub(crate) from: PathBuf,
    /// Don't ask the running app to quit (it keeps running the old version until restarted)
    #[arg(long)]
    pub(crate) keep_app_running: bool,
}

/// Ask the running app to quit over its control channel; refuses while a document has unsaved
/// changes. Prints JSON
#[derive(Args)]
pub(crate) struct QuitAppArgs {
    /// Seconds to wait for the app to close (default 20)
    #[arg(long, value_name = "SECONDS")]
    pub(crate) wait: Option<u64>,
}

#[derive(Args)]
pub(crate) struct SpawnDetachedArgs {
    /// Write the started program's pid here
    #[arg(long, value_name = "FILE")]
    pub(crate) pid_file: Option<PathBuf>,
    /// Append its output here (else it is dropped)
    #[arg(long, value_name = "FILE")]
    pub(crate) log: Option<PathBuf>,
    /// Its working folder
    #[arg(long, value_name = "DIR")]
    pub(crate) cwd: Option<PathBuf>,
    /// The program and its arguments, after `--`
    #[arg(required = true, num_args = 1.., last = true, value_name = "PROGRAM")]
    pub(crate) command: Vec<OsString>,
}

/// Start the app with its control channel (what the launcher runs); print JSON
#[derive(Args)]
pub(crate) struct LaunchArgs {
    /// Seconds to wait for the app to listen (default 30)
    #[arg(long, value_name = "SECONDS")]
    pub(crate) wait: Option<u64>,
}

/// Check the install and print a JSON report: versions, the app's control port, each MCP server's
/// handshake, the photo folder, and which clients have the servers. Never prints the token
#[derive(Args)]
pub(crate) struct DoctorArgs {
    /// Seconds each MCP server may take to answer (default 20)
    #[arg(long, value_name = "SECONDS")]
    pub(crate) timeout: Option<u64>,
}

/// Keep one headless session open and answer JSON lines on stdio, or on 127.0.0.1:<PORT>
///
/// Each request is a JSON line {"id", "method", "params"}. Methods: engine.execute,
/// jobs.list/cancel, engine.commands,
/// doc.open/new/place/save/exportLayers/exportDataSets/importDataSets/inspect/render/select/close,
/// session.list, batch, methods (docs/control-protocol.md#headless-server). The token flags require
/// --port (or MAGICPIC_CONTROL_PORT); stdio needs no control token.
#[derive(Args)]
pub(crate) struct ServeArgs {
    /// Listen on 127.0.0.1:<PORT> instead of stdio
    #[arg(long, value_name = "PORT", env = "MAGICPIC_CONTROL_PORT", value_parser = control_port)]
    pub(crate) port: Option<u16>,
    /// `--port` came from MAGICPIC_CONTROL_PORT, set by [`parse`].
    #[arg(skip)]
    pub(crate) port_from_env: bool,
    #[command(flatten)]
    pub(crate) gpu: GpuArgs,
    #[command(flatten)]
    pub(crate) control: ControlArgs,
    #[command(flatten)]
    pub(crate) automation: AutomationArgs,
}

/// Why a value was refused: clap prints it after the flag and its value (exit 2).
#[derive(Debug, thiserror::Error)]
enum ValueError {
    #[error("expected 24 or 32")]
    TgaBits,
    #[error("expected 8, 10, 12 or 16")]
    DpxBits,
    #[error("expected none, bc1, bc2 or bc3")]
    DdsCompression,
    #[error("expected none, lzw or zip")]
    TiffCompression,
    #[error("expected baseline, optimized or progressive")]
    JpegFormat,
    #[error("expected none or all")]
    Metadata,
    #[error("expected on or off")]
    OnOff,
    #[error("expected a positive number")]
    PdfDpi,
    /// Why a `--bridge` value, or a MAGICPIC_CONTROL_PORT value for `mcp`, is refused.
    #[error("expected a port, 0-65535 (--bridge also takes 127.0.0.1:<PORT>)")]
    Bridge,
}

/// Why `parse` refuses `mcp` or `serve` options clap accepted: clap prints it as a usage error
/// (exit 2).
#[derive(Debug, thiserror::Error)]
enum ServerOptionError {
    /// MAGICPIC_CONTROL_PORT holds a port, as for the desktop app's --control: only the flag
    /// takes host:port.
    #[error("invalid value '{0}' for 'MAGICPIC_CONTROL_PORT': {bridge}", bridge = ValueError::Bridge)]
    BridgeVar(String),
    /// With --bridge the roots are the desktop process's to set: accepting them here promises
    /// authority the bridge can't give (#8035).
    #[error("configure automation roots on the desktop app; --automation-read-root and --automation-write-root require headless MCP mode")]
    BridgeRoots,
    /// Invalid value for MAGICPIC_GPU.
    #[error("invalid value '{0}' for 'MAGICPIC_GPU': expected '1' or '0'")]
    GpuVar(String),
    /// Explicit GPU flags require headless MCP mode, not bridge mode.
    #[error("configure rendering on the desktop app; --gpu and --no-gpu require headless MCP mode")]
    BridgeGpu,
    /// A token flag without the bridge or port that reads it would be ignored (#8035).
    #[error("{flag} requires {transport}")]
    NeedsTransport { flag: &'static str, transport: &'static str },
}

impl ServerOptionError {
    fn into_clap(self, cmd: &mut clap::Command) -> clap::Error {
        let kind = match self {
            Self::BridgeVar(_) | Self::GpuVar(_) => ErrorKind::ValueValidation,
            Self::BridgeRoots | Self::BridgeGpu => ErrorKind::ArgumentConflict,
            Self::NeedsTransport { .. } => ErrorKind::MissingRequiredArgument,
        };
        cmd.error(kind, self)
    }
}

fn tga_bits(s: &str) -> Result<TgaBits, ValueError> {
    s.parse().ok().and_then(TgaBits::from_bits).ok_or(ValueError::TgaBits)
}

fn dpx_bits(s: &str) -> Result<DpxBits, ValueError> {
    s.parse().ok().and_then(DpxBits::from_bits).ok_or(ValueError::DpxBits)
}

fn dds_compression(s: &str) -> Result<DdsCompression, ValueError> {
    DdsCompression::from_name(s).ok_or(ValueError::DdsCompression)
}

fn tiff_compression(s: &str) -> Result<TiffCompression, ValueError> {
    match s {
        "none" => Ok(TiffCompression::None),
        "lzw" => Ok(TiffCompression::Lzw),
        "zip" => Ok(TiffCompression::Deflate),
        _ => Err(ValueError::TiffCompression),
    }
}

fn on_off(s: &str) -> Result<bool, ValueError> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(ValueError::OnOff),
    }
}

/// A PDF import DPI: a finite number greater than zero. The importer's fallback to 300 DPI is
/// for absent preferences, not for a mistyped flag.
fn pdf_dpi(s: &str) -> Result<f32, ValueError> {
    s.parse::<f32>().ok().filter(|dpi| dpi.is_finite() && *dpi > 0.0).ok_or(ValueError::PdfDpi)
}

fn jpeg_format(s: &str) -> Result<JpegFormat, ValueError> {
    JpegFormat::parse(s).ok_or(ValueError::JpegFormat)
}

fn metadata(s: &str) -> Result<XmpEmbed, ValueError> {
    match s {
        "none" => Ok(XmpEmbed::None),
        "all" => Ok(XmpEmbed::All),
        _ => Err(ValueError::Metadata),
    }
}

/// Where `mcp --bridge` connects. Shown as the address: `127.0.0.1:<PORT>` for a port.
#[derive(Clone)]
pub(crate) enum Bridge {
    /// A bare port: the app on 127.0.0.1.
    Port(u16),
    /// `host:port`, as given. Whether the host is loopback is the bridge's to say.
    Addr(String),
}

impl fmt::Display for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bridge::Port(port) => SocketAddr::from((Ipv4Addr::LOCALHOST, *port)).fmt(f),
            Bridge::Addr(addr) => f.write_str(addr),
        }
    }
}

/// `mcp --bridge`, or MAGICPIC_CONTROL_PORT, which holds a port.
fn bridge(s: &str) -> Result<Bridge, ValueError> {
    match control_port(s) {
        Ok(port) => Ok(Bridge::Port(port)),
        Err(_) if s.contains(':') => Ok(Bridge::Addr(s.to_owned())),
        Err(_) => Err(ValueError::Bridge),
    }
}

/// `--new` and `--params`: bad JSON is a usage error, before any work. Whether the value is an
/// object is the command's to say.
fn json(s: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(s)
}

/// `--cmd` and `--params` values back in command-line order: clap keeps each flag's values apart,
/// their indices say how they interleave.
fn steps(m: &ArgMatches) -> Vec<Step> {
    let mut steps: Vec<(usize, Step)> = Vec::new();
    if let (Some(at), Ok(Some(ids))) = (m.indices_of("cmd"), m.try_get_many::<String>("cmd")) {
        steps.extend(at.zip(ids).map(|(i, id)| (i, Step::Cmd(id.clone()))));
    }
    if let (Some(at), Ok(Some(params))) = (m.indices_of("params"), m.try_get_many::<Value>("params")) {
        steps.extend(at.zip(params).map(|(i, p)| (i, Step::Params(p.clone()))));
    }
    steps.sort_by_key(|s| s.0);
    steps.into_iter().map(|(_, s)| s).collect()
}

/// Parse the arguments (without the binary name). clap reads them as `OsString`s: a path need not
/// be valid Unicode (#1108), text must. A flag left out falls back to its environment variable,
/// unless that is empty, and a bad value from one is a usage error naming the variable (#2491). A
/// bare `--help` or `-h` is help even after a mistake on the same line (`--out --help`, `mcp
/// stray.psd --help`): clap reports the mistake first.
pub(crate) fn parse(args: &[OsString]) -> Result<Cli, clap::Error> {
    let mut cmd = Cli::command().mut_subcommands(|sub| sub.mut_args(blank_env_is_unset));
    let argv = std::iter::once(OsString::from("magicpic-cli")).chain(args.iter().cloned());
    let matches = match cmd.try_get_matches_from_mut(argv) {
        Err(e) if args.iter().take_while(|a| *a != "--").any(|a| *a == "--help" || *a == "-h") => {
            let help = [OsString::from("magicpic-cli"), args.first().cloned().unwrap_or_default(), OsString::from("--help")];
            return Err(cmd.try_get_matches_from_mut(help).err().unwrap_or(e));
        }
        matches => matches.map_err(|e| name_env_vars(e, &cmd, args))?,
    };
    let mut cli = Cli::from_arg_matches(&matches)?;
    let from_env = |m: &ArgMatches, id: &str| m.value_source(id) == Some(ValueSource::EnvVariable);
    match (&mut cli.command, matches.subcommand()) {
        (Command::Run(r), Some((_, m))) => {
            r.steps = steps(m);
            check_gpu_env(&mut cmd, m)?;
        }
        // `mcp register | unregister | status` take none of the server's flags.
        (Command::Mcp(a), Some((_, m))) if a.action.is_some() || m.subcommand().is_some() => {
            a.bridge = None;
        }
        (Command::Mcp(a), Some((name, m))) => {
            a.bridge_from_env = from_env(m, "bridge");
            if a.bridge_from_env
                && let Some(Bridge::Addr(v)) = &a.bridge
            {
                return Err(ServerOptionError::BridgeVar(v.clone()).into_clap(&mut cmd));
            }
            if let Some(sub) = cmd.find_subcommand_mut(name) {
                // Only the bridge reads a token, so a stale exported pair doesn't stop a headless
                // run.
                if a.bridge.is_some() {
                    conflict(sub, m, "control_token", "control_token_file")?;
                }
                // The roots are the headless engine's: the variable switching to the bridge would
                // drop them, and the error names it.
                if a.bridge_from_env {
                    conflict(sub, m, "automation_read_root", "bridge")?;
                    conflict(sub, m, "automation_write_root", "bridge")?;
                }
                if a.bridge.is_some() && (!a.automation.automation_read_root.is_empty() || a.automation.automation_write_root.is_some()) {
                    return Err(ServerOptionError::BridgeRoots.into_clap(sub));
                }
                if a.bridge.is_some() && (m.value_source("gpu") == Some(ValueSource::CommandLine) || m.value_source("no_gpu") == Some(ValueSource::CommandLine))
                {
                    return Err(ServerOptionError::BridgeGpu.into_clap(sub));
                }
                check_server(sub, m, a.bridge.is_some(), "--bridge")?;
            }
            if a.bridge.is_none() {
                check_gpu_env(&mut cmd, m)?;
            }
        }
        (Command::Serve(a), Some((name, m))) => {
            a.port_from_env = from_env(m, "port");
            if let Some(sub) = cmd.find_subcommand_mut(name) {
                // Only the TCP server reads a token, so a stale exported pair doesn't stop stdio.
                if a.port.is_some() {
                    conflict(sub, m, "control_token", "control_token_file")?;
                }
                check_server(sub, m, a.port.is_some(), "--port")?;
            }
            check_gpu_env(&mut cmd, m)?;
        }
        (Command::Convert(_), Some((_, m))) | (Command::Batch(_), Some((_, m))) => {
            check_gpu_env(&mut cmd, m)?;
        }
        _ => {}
    }
    Ok(cli)
}

fn check_gpu_env(cmd: &mut clap::Command, m: &ArgMatches) -> Result<(), clap::Error> {
    if m.value_source("gpu") == Some(ValueSource::CommandLine) || m.value_source("no_gpu") == Some(ValueSource::CommandLine) {
        return Ok(());
    }
    if let Some(v) = std::env::var_os("MAGICPIC_GPU") {
        // A non-UTF-8 value is still a value other than empty, "0" or "1": reject it rather
        // than silently treating it as unset and running on the CPU.
        let v = v.to_string_lossy();
        let trimmed = v.trim();
        if !trimmed.is_empty() && trimmed != "1" && trimmed != "0" {
            return Err(ServerOptionError::GpuVar(trimmed.to_string()).into_clap(cmd));
        }
    }
    Ok(())
}

/// Refuse a token flag without the bridge or port that reads it (as a flag or its variable). A
/// token variable alone may be a stale export, so it isn't refused.
fn check_server(sub: &mut clap::Command, m: &ArgMatches, has_transport: bool, transport: &'static str) -> Result<(), clap::Error> {
    for (id, flag) in [("control_token", "--control-token"), ("control_token_file", "--control-token-file")] {
        if m.value_source(id) == Some(ValueSource::CommandLine) && !has_transport {
            return Err(ServerOptionError::NeedsTransport { flag, transport }.into_clap(sub));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--pdf-dpi` only takes a positive finite number: a mistyped value is refused at parse time
    /// instead of silently becoming the importer's 300 DPI fallback.
    #[test]
    fn pdf_dpi_rejects_non_positive_and_non_finite_values() {
        assert!(pdf_dpi("300").is_ok());
        assert!(pdf_dpi("0.5").is_ok());
        for bad in ["0", "-1", "nan", "inf", "-inf", "abc"] {
            assert!(pdf_dpi(bad).is_err(), "{bad} should be refused");
        }
    }
}
