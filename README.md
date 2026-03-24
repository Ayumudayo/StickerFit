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
- Supports crop selection, zoom, frame review, and frame editing before export
- Converts supported still images to PNG with the current crop applied and with output capped to Discord size limits
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

- Node.js 20+
- Rust stable toolchain
- Microsoft C++ Build Tools on Windows (`Desktop development with C++`)
- WebView2 runtime on Windows
- Microsoft Edge installed for `npm run test:web-smoke`

Notes for Windows:

- These prerequisites apply to developers building the Tauri shell locally.
- End users running a prebuilt StickerFit app do not need Node.js, Rust, or Visual Studio Build Tools.
- StickerFit uses Tauri's default Windows MSVC toolchain path.

For a clean-machine bootstrap on Windows, run the repository entrypoint below from an elevated PowerShell window. It installs the required toolchain with `winget`, ensures the MSYS2 packages needed for the minimal `ffmpeg` sidecar build, builds the sidecar, runs `npm ci`, and produces the desktop bundles.

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\bootstrap-build-windows.ps1
```

Optional flags:

- `-Clean`: remove `dist/` and `src-tauri/target/release/` before rebuilding
- `-RunChecks`: run unit, Rust, and web smoke tests after the desktop build
- `-SkipDependencyInstall`: use the current machine state as-is and only run the build steps

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
npm run build:desktop -- --no-bundle
```

For the full clean-machine bootstrap from inside the repo after Node is already available:

```bash
npm run bootstrap:windows
```

For a debug desktop build:

```bash
npm run tauri build -- --debug --no-bundle
```

## Verification

```bash
npm run test:unit
npm run test:rust
npm run test:web-smoke
```

For the current beta verification bundle:

```bash
npm run check:beta
```

## Notes

- Output files are saved next to the source file unless the user chooses another folder.
- The desktop app requires the bundled `ffmpeg` sidecar for desktop processing and does not fall back to system tools.
- The current beta release checklist is in [`docs/beta-release-checklist.md`](docs/beta-release-checklist.md).
