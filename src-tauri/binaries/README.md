Place the bundled `ffmpeg` sidecar here when packaging the desktop app.

Current production baseline:

- required: `ffmpeg-x86_64-pc-windows-msvc.exe`
- required runtime DLL: `libwinpthread-1.dll`
- no system fallback
- no bundled `ffprobe` in production builds
- current checked-in sidecar: custom FFmpeg `7.1.3` minimal mingw build
- current sidecar footprint: `ffmpeg.exe` plus `libwinpthread-1.dll`

This sidecar only keeps the demuxers, decoders, protocols, filters, and raw output path needed by StickerFit after the native PNG/GIF/APNG pipeline split.

The custom Windows x64 build flow still lives in:

- `tools/ffmpeg/build-minimal-ffmpeg.ps1`

If `-FfmpegSourceRoot` is omitted, the script downloads the official FFmpeg source archive for the configured version and builds from that cache automatically.
