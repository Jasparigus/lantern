# Lantern — slideshow video maker

Dump photos and videos, arrange them, set how long each photo shows, add a
background song, and save a single H.264 MP4 that plays on phones, TVs, and
computers. Built for memorial and family gatherings: no timelines to learn,
no codecs to pick.

Built with Tauri 2 (Rust core) + React/TypeScript, rendering via a bundled
static ffmpeg sidecar.

## What it does

- Drag-and-drop import of photos (JPEG/PNG/WebP/BMP/TIFF), videos
  (MP4/MOV/WebM/…), and a song (MP3/M4A/AAC/OGG/WAV/FLAC). HEIC is
  rejected with a friendly message until a libheif build is bundled.
- Filmstrip: drag cards to reorder, tap a time to change one photo,
  2s/3s/5s quick default for all photos, per-video trim (start + length),
  inside/fill fit per item, remove, undo (Ctrl+Z). Everything autosaves.
- Music: one song, Soft/Balanced/Loud presets, auto fade-out at the end,
  auto-loop when the song is shorter than the slideshow.
- Preview: plays the slideshow with your music before rendering.
- Export: MP4 (H.264 High profile, yuv420p, +faststart) + AAC. Resolution
  480/720/1080/2K/4K in wide, vertical, or square; quality presets; smoothness
  24/30/60 fps; black/white/blurred letterbox background. Encoding runs on the
  graphics card when one is available (NVIDIA, Apple, Intel Quick Sync, AMD)
  and falls back to the processor automatically — even mid-export. Progress
  bar, cancel anytime, segments render in parallel, and unchanged clips are
  never re-encoded (cache reuse).
- Slideshow folders (`Name.lantern`) keep copies of your media, so the
  project survives file moves; recent projects are restored on launch.

## Development

Requirements: Rust (stable), Node 20+, and on Linux: `webkit2gtk-4.1`,
`gtk3`, `libxdo`.

```sh
npm install
npm run tauri dev
```

Dev convenience: the engine resolves the sidecar from
`src-tauri/binaries/ffmpeg-<target-triple>` first, then `$LANTERN_FFMPEG`,
then `ffmpeg` on PATH — so a plain `ffmpeg` install is enough to hack. To
exercise the real sidecar path locally:

```sh
scripts/prepare-sidecars.sh          # fetches static ffmpeg+ffprobe for your platform
```

Run tests (unit + a real end-to-end export verified with ffprobe):

```sh
cargo test --manifest-path src-tauri/Cargo.toml
```

## Headless CLI

The same binary doubles as a scriptable pipeline for batch jobs and CI:

```sh
cargo run --manifest-path src-tauri/Cargo.toml -- engine
cargo run --manifest-path src-tauri/Cargo.toml -- new <parent-dir> <name>
cargo run --manifest-path src-tauri/Cargo.toml -- import <project.lantern> <file>...
cargo run --manifest-path src-tauri/Cargo.toml -- export <project.lantern> <output.mp4>
```

## Installers for all three platforms

`.github/workflows/release.yml` builds everything on GitHub's runners —
each target on its native OS, which is the only reliable way to produce
them:

- **Linux:** AppImage + `.deb` + `.rpm` (Ubuntu 24.04 runner)
- **macOS:** universal (Apple silicon + Intel) `.dmg` (macOS runner)
- **Windows:** NSIS `.exe` + `.msi` (Windows runner)

Released installers: <https://github.com/Jasparigus/lantern/releases>

Trigger it from the Actions tab ("Release builds" → *Run workflow*) or by
pushing a version tag (`git tag v0.1.3 && git push --tags`), which also
publishes the installers to the GitHub Release.

Local build for development (current OS only):

```sh
scripts/prepare-sidecars.sh          # static ffmpeg/ffprobe → src-tauri/binaries/
npm run tauri build
```

`tauri.conf.json` bundles the sidecars via `externalBin` and the app finds
them next to the executable at runtime, so every installer is self-contained
and works on machines with no ffmpeg installed.

macOS installers cannot be built from Linux: linking against Apple's SDK,
code signing, and `.dmg` assembly all require a macOS host — that's exactly
what the hosted runner provides. (Unsigned Windows/Linux installers may show
SmartScreen/`chmod +x` notes until code signing is configured; see
`tauri.conf.json > bundle > windows` and the `sign_command` option.)

## Status

- M0 scaffold: Tauri 2 wiring, sidecar resolution, engine status IPC.
- M1 media core: import → probe → normalize → thumbnails → autosaved project.
- M2 editor: filmstrip drag-reorder, durations, music card, preview player,
  undo, launch screen with recents.
- M3 export: segment cache → concat → music mux → faststart H.264, progress
  events, cancellation.
- M4 polish: video trim UI, inside/fill fit, blurred-background letterbox,
  all covered by the end-to-end test.
- M5 packaging: GPLv3 license, sidecar fetch script, `tauri build` bundles,
  3-OS release matrix in CI.

## License

GPL-3.0-only (see `LICENSE`). The bundled ffmpeg build includes libx264 (GPL),
so shipping the app under GPLv3 keeps the whole bundle coherent.
