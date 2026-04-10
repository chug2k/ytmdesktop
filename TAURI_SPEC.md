# Spec: Tauri YouTube Music App

## Objective
Build a lightweight, performant desktop wrapper for YouTube Music using Tauri. The goal is to replace the existing heavy Electron application with a fast, low-memory alternative that still provides essential native OS integrations (media controls and notifications).

## Tech Stack
- **Backend/Shell:** Rust, Tauri
- **Frontend/Injection:** Vanilla JavaScript (for interacting with the YouTube Music DOM)
- **Testing:** `cargo test` (Rust), `vitest` (JS unit testing for DOM extraction logic)

## Commands
- **Dev:** `cargo tauri dev`
- **Build:** `cargo tauri build`
- **Test Backend:** `cargo test`
- **Test Frontend:** `npm run test`
- **Lint:** `cargo clippy -- -D warnings` and `cargo fmt -- --check`

## Project Structure
```
src-tauri/          → Rust application source code (Tauri backend)
  src/main.rs       → Application entry point
  src/media.rs      → Media key integration logic
  src/notify.rs     → OS Notification logic
  tests/            → Rust integration tests
src/                → Frontend source code
  index.html        → Minimal entry HTML (redirects/wraps)
  inject.js         → Script injected into YouTube Music to extract track state
tests/              → JS unit tests for `inject.js`
```

## Code Style
**Rust Example:**
```rust
// Use explicit, descriptive variable names. Handle errors gracefully.
pub fn parse_track_info(payload: &str) -> Result<Track, ParseError> {
    let parsed: Track = serde_json::from_str(payload)
        .map_err(|_| ParseError::InvalidFormat)?;
    Ok(parsed)
}
```
**JS Example:**
```javascript
// DAMP tests: descriptive and independent.
it('extracts track title from DOM', () => {
  document.body.innerHTML = '<yt-formatted-string class="title">Song Name</yt-formatted-string>';
  const title = extractTitle(document);
  expect(title).toBe('Song Name');
});
```

## Testing Strategy
- **Unit Tests (Small, 80%):** Rust logic for parsing IPC messages, managing internal state. JS logic for parsing DOM elements.
- **Integration Tests (Medium, 20%):** Rust handlers mocking the OS notification/media key libraries to ensure they are called when specific IPC events are received.
- **Frameworks:** `cargo test` for Rust, `vitest` with `jsdom` for JS.

## Boundaries
- **Always do:** Write failing tests before implementation (TDD). Run `cargo fmt` and `cargo clippy` before commits. Validate IPC payloads.
- **Ask first:** Adding heavy Rust crates (dependencies) that might bloat the binary.
- **Never do:** Write business logic without a test. Build complex frontend UI (rely on YouTube Music's UI).

## Success Criteria
- App successfully loads `https://music.youtube.com`.
- Track changes trigger a native OS notification with the track title and artist.
- Hardware media keys (Play, Pause, Next, Prev) successfully control the YouTube Music playback.
- Rust tests pass (`cargo test`).
- JS tests pass (`npm run test`).
