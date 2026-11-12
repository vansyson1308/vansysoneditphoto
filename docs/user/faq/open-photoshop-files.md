---
title: Can MagicPic open my Photoshop files?
description: MagicPic opens and saves layered PSD and PSB files and keeps what it does not understand yet.
order: 20
verified: 0.8.2
---

Yes. MagicPic opens, edits and saves layered PSD and PSB files, including 16-bit, 32-bit, CMYK and Lab documents.

When it saves the file again, the picture renders the same for 307 of the 309 files in the psd-tools test set the project checks against. Parts MagicPic does not model yet, such as raw blocks and descriptors, are carried over instead of being dropped. A re-saved file is not byte for byte the same as the original, because MagicPic rewrites image resources, layer records and the composite.

Text layers stay editable.

Results vary from file to file. If a PSD does not open or looks wrong, search the issue tracker or report it with the file.

MagicPic also opens layered TIFF, Paint.NET, OpenRaster, SVG, Krita and Affinity files (Krita and Affinity read only), and many flat formats. The full list is in [Open and save files](../guides/open-and-save-files.md).
