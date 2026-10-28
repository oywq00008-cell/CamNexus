# CamNexus

> All your LAN cameras in one window — watch, record, play back.
> Built for old PCs: no transcoding, low memory, works out of the box.

**English | [简体中文](README.md)**

---

## What it is

CamNexus is a Windows desktop app for watching and recording cameras on your local network.

Any device that exposes an **RTSP address** can be added — IP cameras, NVRs, even a phone running a camera app. There is no vendor cloud involved: video and recordings stay on your own machine.

The goal is simple: **turn an old PC into a surveillance recorder**. That's why it records the original stream without re-encoding, renders with a lightweight UI, and keeps memory in the low hundreds of megabytes.

![CamNexus main window](assets/screenshot.png)

---

## Features

### Watching

- **Many cameras at once** — any number of channels, arranged in a grid
- **Free-form fullscreen layout** — drag, resize and snap each view; positions are remembered
- **Auto layout** — tidy scattered views into an even grid
- **Single-channel fullscreen** — double-click or use the right-click menu
- **Sub-stream support** — main stream for big views, sub-stream for small ones
- **Audio** — enable sound for any individual channel
- **Hardware decoding** — CUDA / D3D11VA / QSV / AMF, handing decoding to the GPU

### Recording & playback

- **Records the original stream** — the camera's own bitstream is packaged into MP4, no re-encoding, near-zero CPU
- **Segmented files** — organised by channel and date, with timestamps in the name
- **Storage cap** — 500 GB by default; the oldest files are deleted first, and the cap never exceeds the free disk space
- **Playback** — built-in player, browsable by date and channel
- **Snapshots** — save the current frame as an image

### Image adjustments

- **Rotate / flip** — 0° / 90° / 180° / 270°, horizontal and vertical flips
- **Trim black borders** — detects and removes the pure-black padding baked into a stream (common with portrait phone streams)
- **Aspect lock** — keep proportions while resizing; unlock to stretch freely
- **Restore original ratio** — put the view back to the picture's own aspect

### Discovery & connection

- **LAN scan** — finds candidate cameras and open ports on your subnet
- **Address probing** — tries common RTSP paths and reports working URLs
- **Connection diagnostics** — frame rate, stall reason and status for each channel

### Interface

- Simplified Chinese / English
- Dark and light themes
- First-run guided tour

### Experimental (off by default)

- **Person / vehicle detection** — draws boxes on the picture. Requires a separate model download and can run on CPU or GPU; it's heavy for older machines, so it stays off unless you turn it on.

---

## Requirements

| | |
|---|---|
| OS | Windows 10 / 11 (64-bit) |
| CPU | Dual-core is enough; four cores or more for several 1080p streams |
| RAM | 2 GB or more |
| GPU | Not required; integrated or discrete graphics can enable hardware decoding |
| Network | Same LAN as the cameras |

**FFmpeg is not required up front** — the app downloads and verifies it on first launch, storing it under `%LOCALAPPDATA%\CamNexus`.

---

## Getting started

1. **Download and unzip** to any folder (avoid paths that need administrator rights)
2. **Run** `camnexus.exe`. On first launch it offers to download FFmpeg — let it finish
3. **Add a channel**: top bar → Settings → Channels → enter the address, username and password
   - Not sure about the address? Click **Scan** and the app will look for cameras on your subnet
4. **Save and go back** — the picture appears

> A typical camera address looks like `rtsp://192.168.1.10:554/Streaming/Channels/101`.
> Enter credentials in the separate fields, not inside the URL.

---

## A quick look at the UI

```
┌─────────────────────────────────────────────────┐
│ CamNexus  2/2 online   Playback  Log …   Settings│  ← top bar
├─────────────────────────────────────────────────┤
│ ┌───────────┐ ┌───────────┐                     │
│ │ Channel 1 │ │ Channel 2 │                     │  ← video area
│ └───────────┘ └───────────┘                     │
└─────────────────────────────────────────────────┘
```

| What you want | Where |
|---|---|
| Add or edit channels | Top bar → Settings → Channels |
| Recording options, storage cap | Top bar → Settings → Recording |
| Resolution, decoding, theme, language | Top bar → Settings → Display / General |
| Browse recordings | Top bar → Playback |
| Zoom one channel | Double-click it |
| Enter / leave fullscreen | Bottom-right button; `Esc` to exit |

### Right-click menu

Right-click any view to adjust that channel:

```
Picture
  ├ Rotation (preview)   0° / 90° / 180° / 270°
  ├ Flip horizontally
  ├ Flip vertically
  ├ Lock aspect ratio
  ├ Trim black borders
  └ Restore original ratio
──────────────
View
  ├ Auto layout
  └ Fullscreen this channel
──────────────
Actions
  ├ Snapshot
  └ Reconnect
```

> Rotation and flipping affect the **preview only** — recordings always keep the camera's original picture.

---

## Recording and storage

| | Default | Notes |
|---|---|---|
| Location | `Videos\CamNexus` | Can be changed to any disk in Settings |
| Storage cap | 500 GB | Oldest files are removed first; set 0 for no limit |
| Mode | Original stream (no re-encode) | Near-zero CPU, identical to the camera's own quality |
| Audio | On | The camera's microphone or audio track is stored in the MP4 |

Files are organised as `channel / date / time`, and can be opened with any media player — no dependency on this app.

---

## Companion app

### CamNexusRTSP — turn an Android phone into a camera

Got a spare Android phone? Install [CamNexusRTSP](https://github.com/oywq00008-cell/CamNexus-RTSP): it runs an RTSP server on the phone itself and streams the camera straight onto your LAN — no PC relay, no cloud.

- A phone camera usually looks far better than a budget IP camera
- Standard RTSP output, so you can add it here as a channel directly
- Keeps streaming in the background and while locked; optional credentials; port 8554 by default

> To connect: start streaming on the phone, then paste the `rtsp://phone-ip:8554/...` address shown there into **Settings → Channels**.

---

## FAQ

**A camera won't connect.**

Check three things: the address is complete, the credentials are correct, and RTSP is enabled on the device. On Hikvision and Dahua devices the *device account* and the *ONVIF account* are separate — being able to log into the web UI does not mean RTSP will accept the same login.

**The picture is a narrow strip with black bars.**

The stream itself is probably portrait video padded into a landscape frame. Right-click → **Trim black borders**, then rotate 90°.

**Recordings take too much space.**

By default the camera's own bitstream is stored. To save space, lower the bitrate on the camera or in the phone app, or pick a lower recording quality (that re-encodes and costs CPU).

**Which cameras are supported?**

Any device with a standard RTSP URL. Hikvision, Dahua, Uniview, TP-LINK and similar brands work; on Android you can use [CamNexusRTSP](https://github.com/oywq00008-cell/CamNexus-RTSP) to serve the phone camera as a stream.

**Does it need internet access?**

Only for the initial FFmpeg download. After that it runs fully offline.

**Antivirus flags it as suspicious.**

The executable isn't code-signed, and some scanners flag unsigned binaries. You can build it yourself from source, or whitelist it.

---

## Configuration file

On first run a `config.json` is created next to the executable. All settings live there (including camera passwords — don't share it). Delete it to reset to defaults.

`config.example.json` in the repository is a template you can refer to.

---

## Building from source

You need Rust **1.88+**, Visual Studio Build Tools (for the MSVC linker) and the Windows SDK (for `rc.exe`, used to embed the icon).

```bash
cargo build --release     # output: target/release/camnexus.exe
cargo test                # unit tests
```

A missing Windows SDK only produces a warning — the build still succeeds, just without an icon and version info. The repository contains only what is needed to build and run the app.

---

## License

[MIT](LICENSE) © 2026 oywq0008-cell

CamNexus does not bundle or redistribute FFmpeg; it is downloaded on the user's machine at first launch.
