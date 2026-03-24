# StickerFit Beta Release Checklist

This checklist is the current gate for a small Windows beta build.

## Build checks

- `npm run build:web`
- `npm run build:desktop`
- Confirm the desktop executable starts without immediately exiting
- Confirm MSI and NSIS bundles are produced

## Automated checks

- `npm run test:unit`
- `npm run test:rust`
- `npm run test:web-smoke`

## Media tool checks

- Confirm bundled `ffmpeg` exists under `src-tauri/binaries`
- Confirm the app reports the media tool as ready in the desktop shell
- If the sidecar is missing, block the beta build

## Sample file matrix

- Still image:
  - `png` input
  - `jpg` input
  - large crop that must be downscaled to `320`
  - `png` source converted again without being blocked
- Animated/video:
  - `gif`
  - `mp4`
  - `webm`
  - frame edit + crop + optimizer run
- Error and boundary:
  - invalid or unsupported file
  - no output folder chosen
  - minimum window size
  - web preview mode with desktop-only actions disabled

## UI checks

- No app-level scrollbars during normal editor use
- Frame rail scrolls when frame count is large
- Overlay panels open, close, and keep their own internal scroll
- Focus rings are visible on buttons, inputs, and selects
- Static image flow does not show video-only fields
- Long paths and labels truncate cleanly instead of wrapping

## Beta approval

- A Discord-safe result is selected when one exists
- If no result fits the size limit, the UI clearly reports that the smallest oversize output is shown
- Static image PNG conversion respects Discord size limits
- Web and desktop builds both pass without repo changes after verification
