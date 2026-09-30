# Spec: Tauri YouTube Music App

## Objective
Build a lightweight, performant desktop wrapper for YouTube Music using Tauri. The goal is to replace the existing heavy Electron application with a fast, low-memory alternative that still provides essential native OS integrations (media controls and notifications).

## Tech Stack
- **Backend/Shell:** Rust, Tauri
- **Frontend/Injection:** Vanilla JavaScript (for interacting with the YouTube Music DOM)
- **Testing:** `cargo test` (Rust), `vitest` (JS unit testing for DOM extraction logic)

## Commands
- **Dev:** `npx tauri dev`
- **Build:** `npx tauri build`
- **Test Backend:** `cd src-tauri && cargo test`
- **Test Frontend:** `npm test`
- **Lint:** `cd src-tauri && cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`

## Project Structure
```
src-tauri/                    → Rust application source (Tauri backend)
  src/main.rs                 → Thin binary entry point; delegates to lib.rs
  src/lib.rs                  → App setup, window/navigation policy, IPC commands
  src/media.rs                → OS media control integration (souvlaki)
  src/macos_notifications.rs  → Rust side of the macOS notification bridge
  src/macos_notifications.m   → ObjC UNUserNotificationCenter implementation
  src/user_agent.rs           → macOS Safari UA (Google sign-in rejects a bare WKWebView)
  capabilities/default.json   → Tauri ACL: which origins may reach IPC
src/                          → Injected frontend scripts (no bundler, no HTML entry;
                                the window loads music.youtube.com directly)
  inject.js                   → Extracts track state, forwards it over IPC
  ipc_transport.js            → Rejects ipc:// fetches so invoke() uses postMessage
tests/                        → JS unit tests; these load and execute src/inject.js
docs/handoff/                 → Design investigations for the open hard problems
```

Note there is no `src/index.html`. `tauri.conf.json` points `frontendDist` at
`src/` only to satisfy the build; the window is opened against an external URL.

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
- **Unit Tests (Small, 80%):** Rust logic for notification de-duplication, host
  allowlisting, and IPC payload parsing. JS logic for DOM extraction.
- **Frameworks:** `cargo test` for Rust, `vitest` with `jsdom` for JS.
- **Tests must exercise shipped code.** `tests/inject.test.js` reads
  `src/inject.js` off disk and executes it, then asserts against the functions
  the file exposes on `window.__ytmYagamiInternals`. Do not copy a function into
  a test file to make it testable — an earlier version of this suite did exactly
  that, drifted out of sync with the injector, and ended up green while
  asserting the *opposite* of production behaviour. If something is hard to
  test, expose it; do not duplicate it.
- **Verify tests can fail.** After writing a test, break the code it covers and
  confirm it goes red.

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
