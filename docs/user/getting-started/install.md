---
title: Install MagicPic
description: MagicPic has no installers yet. Build it from source with Rust on Windows, macOS or Linux.
order: 10
verified: 0.8.2
---

MagicPic has **no installers or releases yet**. For now you build it from source:

```sh
git clone https://github.com/vansyson1308/vansysoneditphoto magicpic
cd magicpic
cargo run --release -p magicpic                  # the desktop app
cargo install --path apps/magicpic-cli --locked  # the command line and MCP server
```

You need Rust (stable) and, on Linux, the development packages listed in
[`docs/development.md`](../../development.md#prerequisites). This has been done on Linux only;
Windows and macOS builds of MagicPic are untested.

## Installers

There are none yet: no MagicPic release has been published. The packaging scripts that came with
the fork (Windows MSI, macOS disk image, Linux AppImage, `.deb`, `.rpm` and Flatpak, a FreeBSD
tarball and the web build) are in `packaging/` and have not been run for MagicPic. What they
need before a first release is listed in [`docs/releasing.md`](../../releasing.md).

## Linux and Wayland

On a Wayland session, files dropped on the MagicPic window do not open yet. Use **File › Open…** instead, or copy the image in your file manager and paste it with Ctrl+V. To drag and drop, start MagicPic under XWayland with `WAYLAND_DISPLAY= magicpic`. MagicPic shows the right command for your install in a notice until you dismiss it.

## Next step

Open an image: [Open and save files](../guides/open-and-save-files.md).
