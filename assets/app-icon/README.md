# MagicPic app icon

A **picture with a sparkle**: a rounded picture frame holding two peaks and a sun, with a large
four-point sparkle over its top-right corner and a small one beside it. It reads at 16 px.

## Palette

| Colour | Hex | Used for |
|---|---|---|
| Violet | `#6a3df0` | the field, top-left |
| Pink | `#e2469a` | the field, bottom-right |
| White | `#ffffff` | the frame, peaks and sun |
| Sparkle yellow | `#ffe36b` | the sparkles |
| Outline violet | `#7a3fe4` | the rim that lifts the large sparkle off the frame |

## Geometry

A 512-unit tile (`viewBox="0 0 512 512"`), rounded square with `rx=112`, no border. The macOS
renders pad it onto Apple's 824/1024 icon grid; the Windows and Linux renders use the whole tile.

## Provenance

Original work for MagicPic, written by hand as SVG. License: see `LICENSE.txt`.

## Files

- `magicpic.svg`: canonical master.
- `magicpic-small.svg`: the same drawing, used as the scalable theme icon.
- `magicpic-1024.png`, `magicpic.icns`, `magicpic.ico`, `hicolor/`: rendered by
  `packaging/icons.sh` (the `.icns` only on macOS, where `iconutil` exists).
