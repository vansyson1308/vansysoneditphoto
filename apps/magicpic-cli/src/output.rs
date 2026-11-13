use magicpic_io::ExportOptions;
use serde_json::Value;
use std::io::Write;

use crate::{Result, args::ExportArgs};

/// The `--format` family's options. A bad value never gets here: clap refused it.
impl From<&ExportArgs> for ExportOptions {
    fn from(a: &ExportArgs) -> Self {
        // TIFF output is flat unless asked: scripted conversions keep predictable sizes.
        let mut o = ExportOptions {
            tiff_layers: a.tiff_layers,
            tiff_transparency: a.tiff_transparency,
            tga_bits: a.tga_bits,
            pcraft_full_composite: a.pcraft_full_composite,
            ..Default::default()
        };
        if let Some(q) = a.quality {
            o.encode.jpeg_quality = q;
            // Asking for a quality asks for a lossy WebP; the default WebP stays lossless.
            o.encode.webp_quality = q;
            o.encode.webp_lossless = false;
        }
        o.encode.dpx_bits = a.dpx_bits;
        if let Some(c) = a.tiff_compression {
            o.encode.tiff_compression = c;
        }
        o.encode.tiff_pyramid = a.tiff_pyramid;
        if let Some(f) = a.jpeg_format {
            o.encode.jpeg_format = f;
        }
        if let Some(c) = a.dds_compression {
            o.encode.dds.compression = c;
        }
        o.encode.dds.mipmaps = a.dds_mipmaps;
        o.psd_maximize_compatibility = !a.no_maximize_compatibility;
        // Conversions keep the whole packet unless asked, as Save As does (#733, #9466).
        if let Some(m) = a.metadata {
            o.xmp = m;
        }
        o
    }
}

pub(crate) fn warn_all(err: &mut dyn Write, ws: &[String]) {
    for w in ws {
        let _ = writeln!(err, "warning: {w}");
    }
}

pub(crate) fn warn_dedup(err: &mut dyn Write, ws: &[String], seen: &mut std::collections::HashSet<String>) {
    for w in ws {
        if seen.insert(w.clone()) {
            let _ = writeln!(err, "warning: {w}");
        }
    }
}

/// How [`print_json`] lays the JSON out.
pub(crate) enum JsonStyle {
    Pretty,
    Compact,
}

pub(crate) fn print_json(out: &mut dyn Write, v: &Value, style: JsonStyle) -> Result {
    let s = match style {
        JsonStyle::Pretty => serde_json::to_string_pretty(v),
        JsonStyle::Compact => serde_json::to_string(v),
    }?;
    Ok(writeln!(out, "{s}")?)
}

/// [`print_json`] (pretty) with every non-ASCII character escaped (`\u203a`), for what agents
/// read through a console: Windows PowerShell 5.1 decodes a native program's output in the
/// console's code page, which turns UTF-8 paths and punctuation into `?`. JSON readers decode
/// the escapes back.
pub(crate) fn print_json_ascii(out: &mut dyn Write, v: &Value) -> Result {
    let s = serde_json::to_string_pretty(v)?;
    Ok(writeln!(out, "{}", ascii_json(&s))?)
}

/// `json` with non-ASCII characters as `\uXXXX` escapes (UTF-16, so astral ones become a
/// surrogate pair). Only valid for serialized JSON, where such characters are inside strings.
pub(crate) fn ascii_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_json_is_ascii_and_reads_back_the_same() {
        let v = serde_json::json!({"message": "tray icon \u{203a} Quit", "path": "C:\\Users\\S\u{01a1}n\\\u{1f4f7}.png"});
        let mut out = Vec::new();
        print_json_ascii(&mut out, &v).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.is_ascii(), "{text}");
        assert!(text.contains("\\u203a") && text.contains("\\ud83d\\udcf7"), "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), v);
    }
}
