This directory contains the FFmpeg sidecar payload packaged with the desktop app.

The authoritative state is the combination of
`tools/ffmpeg/ffmpeg-version.json` and `ffmpeg-provenance.json`; do not infer it
from filenames alone.

Legacy bootstrap baseline (`legacyBootstrap: true`):

- required: `ffmpeg-x86_64-pc-windows-msvc.exe`
- required runtime DLL: `libwinpthread-1.dll`
- no system fallback
- no bundled `ffprobe` in production builds
- sidecar: custom FFmpeg `7.1.3` minimal mingw build
- sidecar footprint: `ffmpeg.exe` plus `libwinpthread-1.dll`

This sidecar only keeps the demuxers, decoders, protocols, filters, and raw output path needed by StickerFit after the native PNG/GIF/APNG pipeline split.

That legacy executable predates the protected vendor workflow. Its repository
hash and embedded configure string are known, but its original build toolchain,
source date epoch, workflow run, and exact relationship to the separately
verified source archive are not. Those unknowns remain explicit `null` values in
`ffmpeg-provenance.json`; this payload must not be treated as a reproducible or
policy-verified release build.

Supply-chain metadata and verification entry points:

- `tools/ffmpeg/build-minimal-ffmpeg.ps1`
- `tools/ffmpeg/ffmpeg-version.json`
- `tools/ffmpeg/ffmpeg-release-signing-key.asc`
- `src-tauri/binaries/ffmpeg-provenance.json`

Routine verification may accept a legacy payload only through the explicit
`-VerifyVendorArtifacts` mode. Release validation rejects it. A replacement is
produced only by the protected, staging-only `-UpdateVendorArtifacts` workflow;
the build script never writes a newly built binary directly into this directory.

Policy-verified replacement state (`legacyBootstrap: false`):

- `ffmpeg-x86_64-pc-windows-msvc.exe` is the only FFmpeg runtime file
- `runtimeDependencies` and `vendor.expectedRuntimeDependencies` are empty
- Tauri packages only `LICENSE-ffmpeg.txt` and `ffmpeg-provenance.json` as
  FFmpeg resources
- `libwinpthread-1.dll`, `LICENSE-libwinpthread.txt`, and
  `LICENSE-ffmpeg-BtbN.txt` are absent

The protected update artifact carries the exact `src-tauri/tauri.conf.json`
resource-map replacement together with the executable, manifest, provenance,
license, and explicit legacy-file deletions. Its fragment is applied only after
protected-policy validation; release validation rejects any other resource membership.

`LICENSE-ffmpeg.txt` is the normalized FFmpeg source license kept for both
states. In the legacy state, `LICENSE-ffmpeg-BtbN.txt` additionally describes
that distribution and `LICENSE-libwinpthread.txt` covers its runtime DLL, whose
repository bytes match the inspected MSYS2 installation. The two legacy-specific
licenses stay paired with the DLL and are removed together by the policy-verified
replacement.
