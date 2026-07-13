# StickerFit

StickerFit is a Windows-first Tauri desktop app for turning local media into a Discord-compatible APNG sticker.

## Current scope

- Desktop app first: the Tauri build performs inspection, optimization, export, and output-folder actions locally.
- Web preview mode: the browser build is for layout and interaction verification only.
  - Supported in web mode: file inspection, crop, zoom, timeline review, and overlay flows.
  - Not supported in web mode: optimization search, PNG export, and output-folder actions.
- Target output constraints:
  - `320x320` max dimensions
  - `<= 512 KiB`
  - `<= 5 seconds`

## Supported inputs

- Video or animated sources: `mp4`, `gif`, `webm`, `mov`, `m4v`, `apng`
- Still images: `png`, `jpg`, `jpeg`, `bmp`

## What it does

- Inspects local media with bundled `ffmpeg` plus native Rust parsers for still images, GIF, and APNG
- Builds a ranked candidate ladder for Discord-safe APNG output
- Chooses the best result by preferring candidates that stay closest to the source while still fitting the Discord limit
- Shows an output-size estimate before export: still PNG results and animated candidates with up to 12 output frames use exact compression measurements, while larger animated candidates use a sampled range with a confidence label
- Offers an exact near-limit candidate check that measures the encoded bytes without creating an output file
- Supports crop selection, zoom, frame review, and frame editing before export
- Converts supported still images to PNG with the current crop applied and with output capped to Discord size limits
- Reports optimization progress and lets the user cancel estimation, exact checks, and optimization without keeping a partial output
- Lets users choose an output folder, or defaults to the source file folder
- Starts in Korean on Korean systems and English otherwise

## Static image vs animated/video flow

- Still images:
  - Show crop and zoom controls
  - Offer direct `PNG` conversion
  - Do not show frame-rate or timeline transport UI
- Animated/video sources:
  - Show frame rail, playback/timeline, preview candidates, and optimizer results
  - Use ranked search to find the best APNG output under Discord limits

## Output-size estimates and safety limits

- A still-image `PNG` estimate is exact because StickerFit runs the same in-memory compression path used for export.
- An animated candidate with up to 12 output frames is measured over the full sequence and shown as exact. Larger animated candidates use a sampled lower-to-upper range whose confidence label reflects sample coverage; that range is not presented as an exact byte count.
- Near the `512 KiB` boundary, **Check exact size** encodes that candidate to a temporary measurement sink. It does not create or replace an output file.
- The completed export size is authoritative. Estimate and optimization operations can be cancelled, and cancellation does not leave a partial destination file.
- Media is rejected before unsafe work when it exceeds the current input guards: `512 MiB` source size, `16384` pixels on either dimension, `40,000,000` pixels, `160 MiB` decoded RGBA memory, `300` frames, or a `64 MiB` non-image-data PNG chunk.
- Windows video inspection prefers Media Foundation where supported. WebM and native-decoder failures use the bundled, network-disabled `ffmpeg` sidecar; StickerFit never falls back to an arbitrary system `ffmpeg`.

## Tech stack

- Frontend: React + TypeScript + Vite
- Desktop shell: Tauri v2
- Media tooling: bundled `ffmpeg` sidecar
- Backend layer: Rust commands exposed through Tauri
- Test tooling: Vitest for pure frontend logic, Playwright for web smoke checks

## Project layout

- `src/` - React UI, workflow logic, and frontend tests
- `src-tauri/` - Rust backend, Tauri config, bundled binaries, and backend tests
- `tests/` - Playwright web smoke checks
- `docs/` - beta checklist and supporting notes

## Development

Requirements:

- Node.js `22.12.x` or newer in the Node 22 LTS line, or Node.js 24 LTS (`>=22.12.0 <23 || >=24.0.0 <25`)
- Rust `1.96.1` with `rustfmt` and `clippy` (the crate's declared MSRV is Rust `1.88`)
- Microsoft C++ Build Tools on Windows (`Desktop development with C++`)
- WebView2 runtime on Windows
- 7-Zip `26.02` or newer at the canonical 64-bit path (`C:\Program Files\7-Zip\7z.exe`) for static NSIS payload verification
- Microsoft Edge installed for `npm run test:web-smoke`

Notes for Windows:

- These prerequisites apply to developers building the Tauri shell locally.
- End users running a prebuilt StickerFit app do not need Node.js, Rust, or Visual Studio Build Tools.
- StickerFit uses Tauri's default Windows MSVC toolchain path.

For a clean-machine bootstrap on Windows, run the repository entrypoint below from a **non-administrator** PowerShell window. It elevates only a dependency-install child, then returns repository verification, `npm ci`, sidecar work, and the desktop build to the original non-administrator process.

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\bootstrap-build-windows.ps1
```

Optional flags:

- `-Clean`: remove `dist/` and `src-tauri/target/release/` before rebuilding
- `-RunChecks`: run the quality gates and verify both the loose and statically extracted NSIS FFmpeg payloads after the desktop build
- `-SkipDependencyInstall`: do not install or select machine toolchains; verify the existing Node/Rust/MSYS2/Visual Studio/WebView2/7-Zip state, then continue the requested repository build steps
- `-DryRun`: emit the structured bootstrap plan as JSON without elevating, installing, deleting, downloading, building, or testing

```bash
npm install
npm run dev:desktop
```

For browser-only preview work:

```bash
npm run dev:web
```

## Build

```bash
npm run build:web
npm run tauri -- build --no-bundle -- --locked
```

For the full clean-machine bootstrap from inside the repo after Node is already available:

```bash
npm run bootstrap:windows
```

For a debug desktop build:

```bash
npm run tauri -- build --debug --no-bundle -- --locked
```

## Verification

```bash
npm run test:unit
npm run test:rust
npm run test:web-smoke
npm run test:powershell-syntax
npm run test:bootstrap-contract
npm run test:ffmpeg-source-contract
npm run test:generated-media
npm run test:tauri-policy
npm run test:vendor-policy
npm run test:rust-audit
npm run ffmpeg:verify-source
npm run ffmpeg:verify-vendor
```

For the current beta verification bundle:

```bash
npm run check:beta
```

## Notes

- Output files are saved next to the source file unless the user chooses another folder.
- The desktop app requires its verified bundled `ffmpeg` sidecar for fallback/video processing and does not fall back to system tools or network-enabled media tooling.
- The current beta release checklist is in [`docs/beta-release-checklist.md`](docs/beta-release-checklist.md).
