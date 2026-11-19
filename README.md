<p align="center">
  <img src="app-icon.png" alt="YTM Yagami icon" width="128" />
</p>

# YTM Yagami

> *It's light.*

<p align="center">
  <img src="yagami.png" alt="YTM Yagami" width="600" />
</p>

A lightweight desktop app for YouTube Music, built with [Tauri](https://tauri.app). Uses your system's native webview instead of bundling Chromium — so it's fast, tiny, and won't eat your RAM.

## Features

- Native desktop window for YouTube Music
- OS media controls (play/pause/next/previous via keyboard media keys)
- Track change notifications with album art
- ~5 MB macOS download (vs ~200 MB for Electron-based alternatives)

## Install

Open the [YTM Yagami v0.1.0 release](https://github.com/chug2k/ytmdesktop/releases/tag/v0.1.0)
and choose a download under **Assets**.

| Your system | File |
| --- | --- |
| macOS on Apple silicon (arm64) | `YTM.Yagami_0.1.0_aarch64.dmg` |
| macOS on Intel (x86_64) | `YTM.Yagami_0.1.0_x64.dmg` |
| Windows x64 | `YTM.Yagami_0.1.0_x64-setup.exe` |
| Linux x86_64 (Debian/Ubuntu package) | `YTM.Yagami_0.1.0_amd64.deb` |
| Linux x86_64 (RPM package) | `YTM.Yagami-0.1.0-1.x86_64.rpm` |
| Linux x86_64 (AppImage) | `YTM.Yagami_0.1.0_amd64.AppImage` |

macOS is the supported platform and the only one with native track notifications.
The v0.1.0 macOS apps lack a Developer ID signature and notarization, so Gatekeeper
may block them. The Windows installer is unsigned.

## Build from source

You'll need [Node.js](https://nodejs.org) (v18+) and [Rust](https://rustup.rs) installed.

```sh
git clone https://github.com/chug2k/ytmdesktop.git
cd ytmdesktop
npm install
npx tauri build
```

The built app will be in `src-tauri/target/release/bundle/`.

## Development

```sh
npm install
npx tauri dev
```

## Tests

```sh
npm test                      # JS (vitest + jsdom)
cd src-tauri && cargo test    # Rust
```

Lint, matching what CI enforces:

```sh
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Lineage

This started life as a fork of [ytmdesktop/ytmdesktop](https://github.com/ytmdesktop/ytmdesktop),
an excellent Electron-based YouTube Music desktop app, and the original is worth
your attention if you want a more featureful, more mature application.

YTM Yagami no longer shares any code with it. The Electron application was
replaced wholesale by a Rust/Tauri one; nothing from the original source tree
survives. The debt is one of inspiration and prior art, not of code.

Not affiliated with Google or YouTube.
