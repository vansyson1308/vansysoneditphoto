---
title: MagicPic does not start
description: MagicPic closes or crashes at startup on Windows PCs with Intel graphics (it uses DirectX 12 and picks a safer graphics mode after a crash) or on older Intel Macs (0.8.2 fixes one cause; no fix yet for Macs patched with OpenCore Legacy Patcher).
order: 30
verified: 0.8.2
---

On Windows, MagicPic starts Intel graphics on DirectX 12. If MagicPic closed while it was starting, start it again: it picks a safer graphics mode after a crash. On an older Intel Mac that shows a `Device(Lost)` error, update to 0.8.2, which fixes one cause; on Macs patched with OpenCore Legacy Patcher there is no fix yet.

## Symptoms

- Windows with Intel graphics (for example Intel UHD Graphics 630): MagicPic closes or crashes right after you start it. Windows Event Viewer shows an application error in `igvk64.dll` with exception `0xc0000005` ([issue 2320](https://github.com/storytold/photocraft/issues/2320), [issue 4](https://github.com/storytold/photocraft/issues/4)).
- Intel Mac: the app icon bounces in the Dock and the app disappears. Started from Terminal, it prints `Error: Wgpu(RequestDeviceError(RequestDeviceError { inner: Core(Device(Lost)) }))` ([issue 1859](https://github.com/storytold/photocraft/issues/1859)).

## Fix

### Windows

1. Use MagicPic 0.7.0 or later. Older builds do not have the steps below.
2. Start MagicPic again. Double-clicking its icon or a `.pcraft` file works. MagicPic notes that the last start did not finish and uses the next safer graphics mode this time:
   - A start on Vulkan is followed by one on DirectX 12.
   - A start on DirectX 12 is followed by one on the CPU. The CPU mode does not depend on the graphics driver.
3. If it started, nothing more to do. MagicPic remembers the mode that worked. You can see or reset it under **Edit › Preferences › Performance…** (on macOS, **MagicPic › Settings › Performance…**).

If MagicPic still closes on every start, force DirectX 12 from PowerShell:

1. Open PowerShell in the folder that contains `magicpic.exe`.
2. Run these three lines:

        $env:WGPU_BACKEND = "dx12"
        & .\magicpic.exe
        Remove-Item Env:WGPU_BACKEND

The last line clears the setting so later launches use the default again. You need the first two lines each time you start MagicPic this way. While `WGPU_BACKEND` is set, MagicPic uses it and does not step down to a safer mode by itself ([issue 2320](https://github.com/storytold/photocraft/issues/2320)).

### Intel Mac

1. Update to MagicPic 0.8.2 or later. On Intel Macs it skips a set of graphics checks that macOS could not compile on older Intel graphics, such as Intel Iris Pro on macOS Monterey ([issue 3107](https://github.com/storytold/photocraft/issues/3107)).
2. Start MagicPic again.

If it still fails, there is no fix yet. This is the case reported on Macs patched with OpenCore Legacy Patcher, where setting `WGPU_BACKEND`, `--safe-gpu` and `MAGICPIC_CPU_CANVAS=1` did not change the error ([issue 1859](https://github.com/storytold/photocraft/issues/1859)). Add your Mac model, macOS version and MagicPic version to that issue.

## Why this happens

On Windows, MagicPic draws with your graphics driver. Intel's Vulkan driver (`igvk64.dll`) crashed MagicPic on a UHD 630 PC with MagicPic 0.6.0, and the DirectX 12 setting above avoided the crash ([issue 2320](https://github.com/storytold/photocraft/issues/2320)). Since 0.7.0 MagicPic prefers DirectX 12 on Intel graphics. Before it opens the window, it writes a small file called `gpu-starting.json` in its settings folder and deletes it once the first frames are drawn. A start that crashed leaves the file behind, and the next start reads it and moves to the next safer mode.

## Still stuck?

See [Get help with a problem](get-help.md). Say which graphics card you have, which MagicPic version you use and how you started it. Add one of these:

- If you can open MagicPic: the text from **Help › System Info…**.
- If MagicPic does not open at all: the log file `magicpic.log`, plus the faulting module and exception code from Event Viewer on Windows. MagicPic keeps it in a `logs` folder inside its settings folder:
  - Windows: `%APPDATA%\Magicpic\logs\magicpic.log`
  - macOS: `~/Library/Application Support/Magicpic/logs/magicpic.log`
  - Linux: `~/.config/magicpic/logs/magicpic.log`
  - Portable copy (a `portable.txt` file next to the program): the `MagicPicData\logs` folder next to the program.

  Every start moves the previous log to `magicpic.1.log`. If you already started MagicPic again after the crash, attach `magicpic.1.log` too, because it holds the run that crashed.
