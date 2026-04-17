# Shrinking YouTube Music from 200 MB to 3 MB

*How I rewrote a bloated Electron app in Tauri, and the macOS rabbit holes I fell into along the way.*

---

## For everyone: what is this?

YouTube Music doesn't have an official desktop app. The best community alternative was [ytmdesktop](https://github.com/ytmdesktop/ytmdesktop) — a great project, but it ships about **200 MB** of Chromium on disk and holds hundreds of MB of RAM, because that's what Electron apps do. They bundle an entire copy of the Chrome browser inside every app.

I rewrote it with [Tauri](https://tauri.app/), which uses your operating system's *existing* webview instead of bundling a browser. The result — [YTM Yagami](https://github.com/chug2k/ytmdesktop) — is about **3 MB to download**.

That's a ~65x size reduction for the same app: a window that plays YouTube Music, shows you native notifications when the song changes, and responds to the play/pause/next/previous keys on your keyboard.

### Why this matters (even if you don't care about the tech)

- **Less disk space.** 200 MB → 3 MB is real savings, especially if you have a bunch of Electron apps already.
- **Less RAM.** Sharing one system webview across apps is cheaper than every app launching its own Chromium.
- **Faster startup.** The app launches in a blink because there's no giant browser to boot.
- **Less "why is my fan spinning."** This matters more than people admit.

The tradeoff: on macOS you're running Safari's engine instead of Chrome's, and on Windows you're running Edge's. Most of the time this is invisible. Sometimes — as I'll explain below — it's a small nightmare.

---

## For developers: the technical story

### The stack

- **Rust + Tauri 2** for the native shell
- **Vanilla JS** injected into `music.youtube.com` to read the player state
- **ObjC bridge** via the `cc` crate for macOS notifications
- **[`souvlaki`](https://github.com/Sinono3/souvlaki)** for OS media key integration (play/pause/next/prev in macOS Control Center)

The architecture is simple: Tauri opens a `WebviewWindow` pointing directly at `https://music.youtube.com`. A small JS snippet gets injected on every page load, watches the player bar with a `MutationObserver`, and forwards track changes to Rust via IPC. Rust handles notifications and media keys.

That's the clean description. Here's what actually happened.

### Rabbit hole 1: Google OAuth hates you if you're not Chrome

The first thing that broke: signing in. YouTube Music's login flow goes through Google OAuth, and Google's auth pages *aggressively* sniff the browser. If you're a system webview — WKWebView on macOS, WebView2 on Windows — Google refuses to log you in. "This browser or app may not be secure."

Fix: lie about being Chrome. This means both:

1. Setting a Chrome `User-Agent` string on the webview:
   ```rust
   .user_agent(CHROME_UA)
   ```
2. Spoofing the *JavaScript* fingerprints Google checks — `window.chrome`, `navigator.userAgentData`, `navigator.plugins`, `navigator.webdriver`. See [`src/chrome_spoof.js`](../src/chrome_spoof.js).

And crucially, the spoof script has to run on **every navigation**, on both `youtube.com` *and* `google.com` subdomains, because OAuth redirects through several origins and each one starts a fresh JS context.

### Rabbit hole 2: `initialization_script` doesn't do what you think

Tauri exposes `WebviewWindowBuilder::initialization_script()`, which the docs describe as running before every page load. In practice, for an `External` URL, it runs once and then silently stops injecting on subsequent navigations.

What actually works: `on_page_load(PageLoadEvent::Finished)` + `window.eval()`, which fires for every load:

```rust
.on_page_load(|window, payload| {
    if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
        let host = payload.url().host_str().unwrap_or_default();
        if host.ends_with("google.com") || host.ends_with("youtube.com") {
            let _ = window.eval(include_str!("../../src/chrome_spoof.js"));
        }
        if host == "music.youtube.com" || host.ends_with(".youtube.com") {
            let _ = window.eval(include_str!("../../src/inject.js"));
        }
    }
})
```

### Rabbit hole 3: `window.__TAURI__` doesn't exist on remote URLs

The Tauri docs show this pattern everywhere:

```js
window.__TAURI__.invoke('my_command', { ... })
```

This does not work when your webview is pointing at an external origin like `music.youtube.com`. The global is only injected for local (bundled) content. For remote pages, the actual IPC entrypoint is:

```js
window.__TAURI_INTERNALS__.invoke('my_command', { payload: state })
```

This is an undocumented(-ish) internal API, but it's the only thing that works. I found this by having Codex grep `~/.cargo/registry/src/.../tauri-*/` for the actual injection site.

You also need to explicitly grant the `core:webview:allow-internal-toggle-devtools` capability and list every Rust command you expose in `src-tauri/capabilities/default.json`, otherwise the invoke is silently rejected by ACL.

### Rabbit hole 4: the `ipc://` protocol gets CORS-blocked

Tauri's default IPC transport is a custom protocol (`ipc://localhost/...`). On a secure origin like `music.youtube.com`, the browser's fetch policy blocks that scheme. Worse, YouTube Music's own code *tries* `fetch('ipc://...')` during init in a way that causes visible errors.

The fix is to monkey-patch `window.fetch` at the top of `chrome_spoof.js` to reject `ipc://` URLs immediately:

```js
var originalFetch = window.fetch;
window.fetch = function(url) {
  if (typeof url === 'string' && url.startsWith('ipc://')) {
    return Promise.reject(new TypeError('IPC blocked on remote pages'));
  }
  return originalFetch.apply(this, arguments);
};
```

Tauri then falls back to `postMessage` transport, which works fine across origins.

### Rabbit hole 5: `tauri-plugin-notification` is broken on macOS

This one cost me the most time.

`tauri-plugin-notification` is the official, documented way to send OS notifications. On macOS, calling `.show()` returns `Ok(())` and then... absolutely nothing happens. No notification, no error, no log line. Silent.

I gave up on figuring out why and replaced it with a small ObjC file ([`src-tauri/src/macos_notifications.m`](../src-tauri/src/macos_notifications.m)) compiled via the `cc` crate, exposing two C functions:

```c
void ytm_notifications_request_authorization(void);
void ytm_notifications_show(const char *id, const char *title,
                            const char *subtitle, const char *image_path);
```

These call `UNUserNotificationCenter` directly, attach album art via `UNNotificationAttachment`, and clear stale notifications before each new one so your Notification Center doesn't turn into a song history log.

### Rabbit hole 6: notifications need code signing

Even after switching to `UNUserNotificationCenter`, notifications *still* didn't show up. This one's a killer:

**`UNUserNotificationCenter` silently no-ops on unsigned or ad-hoc-signed app bundles.**

Symptoms:
- `requestAuthorizationWithOptions` completion handler never fires
- `addNotificationRequest` fails with `UNErrorDomain Code=1`
- No prompt ever appears in System Settings

Fix: properly sign the bundle with a real Apple Developer identity *after* `tauri build`:

```sh
codesign --force --deep --sign "Apple Development: ..." \
  src-tauri/target/release/bundle/macos/YTM\ Yagami.app
```

And a subtle gotcha: macOS caches notification permission decisions per bundle ID. If you ever denied permission during the unsigned debugging phase, you're locked out until you change the bundle ID. I had to rename mine once to get a fresh prompt.

### Rabbit hole 7: debugging a webview inside a Rust app

Two tools saved me:

- `window.open_devtools()` — opens Safari's DevTools against the webview. Essential for seeing JS errors and network requests.
- `log stream --predicate 'processImagePath contains "ytm"'` — streams `NSLog` output from the ObjC notification code, since those messages never hit stdout.

---

## What actually ships

After all that, the app is genuinely nice:

- **~3 MB `.dmg`** vs ~200 MB installed for Electron-based alternatives
- **Instant startup** — no Chromium cold start
- **Native notifications** with album art, one per song change, not a backlog
- **Media keys** work in macOS Control Center
- **Google login works** without having to open Safari separately

### The core tradeoff, honestly

Tauri trades *compatibility* for *lightness*. Electron gives you the exact Chrome you tested against. Tauri gives you whatever webview your user happens to have, which means you will — guaranteed — hit weird edge cases that don't exist in Chrome. For an app this size, the tradeoff was worth it. For something more complex, I'd think harder.

## Links

- **Code:** [github.com/chug2k/ytmdesktop](https://github.com/chug2k/ytmdesktop)
- **Tauri:** [tauri.app](https://tauri.app)
- **souvlaki (media keys):** [github.com/Sinono3/souvlaki](https://github.com/Sinono3/souvlaki)

The name, by the way, is a *Death Note* joke. Yagami. It's light. You're welcome.
