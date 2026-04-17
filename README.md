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
- Track change notifications
- ~3 MB download (vs ~200 MB for Electron-based alternatives)

## Install

### macOS (Apple Silicon)

1. Go to the [Releases page](https://github.com/chug2k/ytmdesktop/releases/latest)
2. Download the `.dmg` file
3. Open it and drag **YTM Yagami** to your Applications folder
4. If macOS blocks it, go to **System Settings > Privacy & Security** and click **Open Anyway**

### Build from source (macOS, Linux, Windows)

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
# JavaScript tests
npm test

# Rust tests
cd src-tauri && cargo test
```
