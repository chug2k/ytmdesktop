# Security boundary review — YTM Yagami

Audit of the native/web trust boundary in this Tauri 2 app, which loads
`https://music.youtube.com` (fully remote, third-party, ad-serving) into a webview
that has been granted Tauri IPC access.

Reviewed at commit `2bfcaf4`, with the uncommitted working tree as of this writing
also taken into account (`src-tauri/src/{lib,media,macos_notifications}.rs` and
`src/inject.js` were being edited concurrently while this was written; where the
working tree already fixes something, that is called out).

## Method — what is verified vs. reasoned

**Verified by reading source or manifests** (every claim below with a file:line
citation): the contents of `src-tauri/gen/schemas/acl-manifests.json`; the enabled
Cargo feature set, resolved with `cargo tree -e features` (no build run); the Tauri
2.10.3, wry 0.54.4, reqwest 0.12.28 and souvlaki 0.8.3 sources in the local cargo
registry, which are the exact versions pinned in `src-tauri/Cargo.lock`.

**Reasoned about without running the app**: the runtime behaviour of
`-[NSImage initWithContentsOfURL:]` for non-HTTP schemes; whether a nil `NSURL`
throws; the practical reachability of any given internal HTTP endpoint. These are
flagged inline as unverified.

**Not done**: no build, no run, no exploitation. Nothing was written outside this
file.

## Trust model

The premise of this app is "run Google's JavaScript". If Google is hostile, the
game is already over in ways this report cannot address — they serve the player
itself. So the interesting question is not "what can Google's own code do", it is:

1. What can a **non-Google party** who gets script execution on the page do —
   an ad, a compromised CDN asset, an XSS on a `*.youtube.com` property?
2. What can that party do **that page JavaScript could not already do on its own**
   in an ordinary browser tab?

Only capabilities that clear bar 2 are real findings. Several things that look
alarming in this codebase do not clear it, and are marked as such.

Two structural facts constrain everything below, both verified:

- **Tauri IPC bootstrap scripts are injected into the main frame only**
  (`tauri-2.10.3/src/manager/webview.rs:156-192`, each script wrapped in
  `main_frame_script()` which sets `for_main_frame_only: true`). Ad iframes do not
  receive `window.__TAURI_INTERNALS__` or the invoke key.
- **The ACL origin is the sending frame's own URL, not the main frame's.** On
  macOS wry reads `msg.frameInfo().request().URL()` from the WKScriptMessage
  (`wry-0.54.4/src/wkwebview/class/wry_web_view_delegate.rs:49-53`), and Tauri
  matches the remote-URL patterns against exactly that
  (`tauri-2.10.3/src/webview/mod.rs:1769-1775`). A cross-origin ad frame that
  reached the message handler directly would still be denied on origin — and it
  would also need `invoke_key`, a per-run random value it cannot read across
  origins (`tauri-2.10.3/src/webview/mod.rs:1731-1748`).

So the realistic hostile-script precondition is **script execution in the main
frame of a page the window is showing** — not merely "an ad ran".

---

## Part 1 — What `core:default` actually permits

Read from `src-tauri/gen/schemas/acl-manifests.json`. `core:default` expands to
nine sub-defaults (`core.default_permission.permissions`), and each of those
expands to a concrete command list. The full grant, command by command:

| Permission set | Commands actually allowed |
|---|---|
| `core:app:default` | `version`, `name`, `tauri_version`, `identifier`, `bundle_type`, `register_listener`, `remove_listener` |
| `core:event:default` | `listen`, `unlisten`, `emit`, `emit_to` |
| `core:image:default` | `new`, `from_bytes`, `from_path`, `rgba`, `size` |
| `core:menu:default` | `new`, `append`, `prepend`, `insert`, `remove`, `remove_at`, `items`, `get`, `popup`, `create_default`, `set_as_app_menu`, `set_as_window_menu`, `text`, `set_text`, `is_enabled`, `set_enabled`, `set_accelerator`, `set_as_windows_menu_for_nsapp`, `set_as_help_menu_for_nsapp`, `is_checked`, `set_checked`, `set_icon` |
| `core:path:default` | `resolve_directory`, `resolve`, `normalize`, `join`, `dirname`, `extname`, `basename`, `is_absolute` |
| `core:resources:default` | `close` |
| `core:tray:default` | `new`, `get_by_id`, `remove_by_id`, `set_icon`, `set_menu`, `set_tooltip`, `set_title`, `set_visible`, `set_temp_dir_path`, `set_icon_as_template`, `set_show_menu_on_left_click` |
| `core:webview:default` | `get_all_webviews`, `webview_position`, `webview_size`, `internal_toggle_devtools` |
| `core:window:default` | `get_all_windows`, `scale_factor`, `inner_position`, `outer_position`, `inner_size`, `outer_size`, `is_fullscreen`, `is_minimized`, `is_maximized`, `is_focused`, `is_decorated`, `is_resizable`, `is_maximizable`, `is_minimizable`, `is_closable`, `is_visible`, `is_enabled`, `title`, `current_monitor`, `primary_monitor`, `monitor_from_point`, `available_monitors`, `cursor_position`, `theme`, `is_always_on_top` |

### The window/webview concern does not hold up

**`core:window:default` and `core:webview:default` are read-only.** They contain no
`create`, no `close`, no `navigate`, no `set_position`, no `set_focus`, no `eval`.
Those commands exist in Tauri — `desktop_commands::create_webview_window` is at
`tauri-2.10.3/src/webview/plugin.rs:44-51` and `window::create` at
`window/plugin.rs` — but they are simply **not in the default permission set**, so
the ACL rejects them (`webview/mod.rs:1801-1829`).

A hostile script can enumerate windows and read their geometry, title, theme and
the cursor position. It cannot create a window, navigate one, move one, or close
one. That is a fingerprinting-grade leak, not a control-grade one.

### Two whole permission sets are dead code in this build

Resolved feature set for `tauri` in this project (`cargo tree -e features`):
`common-controls-v6`, `compression`, `devtools`, `dynamic-acl`, `wry`
(+ `tauri-runtime-wry`, `webkit2gtk`, `webview2-com`), `x11`. Notably **absent**:
`tray-icon`, `image-png`, `image-ico`, `isolation`, `unstable`,
`protocol-asset`.

- **All eleven `core:tray:*` commands are unreachable.** The tray plugin is only
  registered under `#[cfg(all(desktop, feature = "tray-icon"))]`
  (`tauri-2.10.3/src/app.rs:1132-1133`), and that feature is off. Invoking any of
  them returns "plugin tray not found".
- **`core:image:from_path` and `from_bytes` are compiled as error stubs.** Both
  have a `#[cfg(not(any(feature = "image-ico", feature = "image-png")))]` variant
  that returns `Err("... only supported if the image-ico or image-png Cargo
  features are enabled")` (`tauri-2.10.3/src/image/plugin.rs:32-36, 50-54`), and
  neither feature is on — the `image` crate is not even in `Cargo.lock`.

That last one matters, because `from_path` + `rgba` would otherwise have been a
genuine arbitrary-file-read primitive: `rgba` returns the decoded pixel buffer
straight to JavaScript (`image/plugin.rs:57-62`). In *this* build it is dead. It
would come alive the moment anyone adds a tray icon or an image feature, which is
exactly the kind of change nobody thinks of as security-relevant. That is the
argument for dropping `core:default` — not the current exposure.

### What is actually live and useful to an attacker

**`core:menu:*` is the only meaningful native surface.** The menu plugin *is*
registered on desktop unconditionally (`app.rs:1130-1131`). A script can build an
arbitrary menu, set arbitrary text and accelerators on it, install it as the
**application menu bar** (`set_as_app_menu`), and pop it up at a chosen screen
position (`popup`, `menu/plugin.rs:663-691`). See F7.

**`core:path:resolve_directory`** returns real filesystem paths (home, app-data,
app-cache, …), which leak the local username and the app's on-disk layout. Pure
information disclosure; see F8.

**`core:event:emit` / `emit_to`** let the page emit arbitrary Tauri events. This
app registers no Rust-side event listeners and has only one window, so there is
nothing to confuse. Currently inert.

---

## Part 2 — Findings, ranked by real exploitability

### F1 — `payload.art` is handed to `NSImage initWithContentsOfURL:` with no validation — Medium-High

**This is the most serious issue and it was not on the original list.**

`handle_track_changed` calls `state.media_controls.update(&payload)` before any
notification gating (`src-tauri/src/lib.rs:48` at HEAD; still first in the working
tree). That reaches:

`src-tauri/src/media.rs:59` → `cover_url: Some(&state.art)` → souvlaki
`set_metadata` → `set_playback_metadata`
(`souvlaki-0.8.3/src/platform/macos/mod.rs:134-139`), which dispatches to a global
GCD queue and calls:

```rust
// souvlaki-0.8.3/src/platform/macos/mod.rs:325-333
unsafe fn load_image_from_url(url: &str) -> (id, CGSize) {
    let url = ns_url(url);                                    // [NSURL URLWithString:]
    let image: id = msg_send!(class!(NSImage), alloc);
    let image: id = msg_send!(image, initWithContentsOfURL: url);
    ...
}
```

Why this is worse than the `reqwest` path everyone was looking at:

| | `reqwest` path (`macos_notifications.rs`) | `NSImage` path (`souvlaki`) |
|---|---|---|
| Gated on track change? | yes | **no — fires on every invoke** |
| Scheme restriction | http/https only, enforced by reqwest | **whatever `NSURL`/`NSData` accepts, including `file://`** |
| Timeout | 30 s default; 10 s in working tree | **none** |
| Size cap | 8 MB in working tree | **none** |
| De-duplication | title+artist key | **none** |
| Fixed in working tree? | yes | **no — untouched** |

`[NSImage initWithContentsOfURL:]` loads through the URL loading system, so
`file:///…`, `http://…` and `ftp://…` are all plausible inputs. I did not run this,
so treat "file:// is read" as reasoned rather than verified — but reqwest's own
scheme guard exists precisely because this class of API does not have one.

**Attack scenario.** Script in the main frame calls, in a loop:

```js
__TAURI_INTERNALS__.invoke('handle_track_changed', {
  payload: { title:'x', artist:'y', isPlaying:true,
             art:'http://192.168.1.1/admin/reboot?confirm=1' }})
```

Each call issues a request **from the native process**, which is outside the
webview's CORS, Private Network Access, mixed-content and CSP enforcement, and
outside any ad-blocking extension. Reachable targets that page JS cannot hit
cleanly: `http://127.0.0.1:*` (dev servers, Ollama, Docker-adjacent daemons,
Prometheus), the RFC1918 LAN (router admin panels), and `http://169.254.169.254/`
if the machine is a cloud VM.

**Preconditions and how realistic they are.** Main-frame script execution is
required — see F2 and F4 for how a non-Google party gets it. The channel is
**blind**: nothing about the response returns to JavaScript, so this is
request *forgery*, not data exfiltration. That is what keeps it out of High: it
gets you state-changing GETs against internal hosts and a blind local-file probe,
not readable content. The link-local metadata endpoint specifically is a stretch —
it needs the user to be running a desktop music player on a cloud VM, and IMDSv2
requires a PUT that this path cannot issue.

**Fix**: validate `art` before it reaches `TrackState` at all. See Part 4.

### F2 — This app's own commands are not gated by the ACL at all — Medium

`remote.urls` in `capabilities/default.json` does **not** constrain
`handle_track_changed`. Tauri only enforces the ACL on plugin commands, or on app
commands when the app has declared its own ACL manifest:

```rust
// tauri-2.10.3/src/webview/mod.rs:1800-1802
// we only check ACL on plugin commands or if the app defined its ACL manifest
if (plugin_command.is_some() || has_app_acl_manifest) && ... && invoke.acl.is_none() {
```

There is no `src-tauri/permissions/` directory and `build.rs` does not call
`AppManifest::commands(...)`, so `has_app_acl_manifest` is false. Meanwhile the
IPC bootstrap and invoke key are injected into **every** main-frame navigation,
unconditionally — unlike `chrome_spoof.js`/`inject.js`, which `on_page_load`
gates by host (`lib.rs:87-97` at HEAD).

**Consequence**: any page that becomes the main frame — not just
`music.youtube.com`, not just `*.youtube.com` — can call `handle_track_changed`
and drive F1 and F3. The carefully-written remote-URL allowlist provides zero
protection for the only two commands this app actually exposes. It protects only
the `core:*` commands, which as shown above are mostly read-only or dead.

**Attack scenario.** Anything that makes the window navigate to attacker HTML is a
full precondition-satisfier. At HEAD that was trivial (`url_str.contains("youtube.com")`
— `https://evil.com/?r=youtube.com`). In the working tree it is fixed, but see F4:
`googleusercontent.com` is on the navigable list and serves user-controlled content.

### F3 — `payload.art` reaches `reqwest` with no scheme or host allowlist — Medium-Low

`macos_notifications.rs:65` at HEAD: `reqwest::blocking::get(url)`. The working
tree already improved this substantially — a shared client with a 5 s connect / 10 s
total timeout and an 8 MB response cap. What remains missing in both versions is
any **scheme or host restriction**.

Two corrections to the brief:

- **There was never "NO timeout".** `reqwest::blocking`'s client has a 30-second
  default total timeout (`reqwest-0.12.28/src/blocking/client.rs:1498-1505`), which
  applied to the HEAD code. The real gap was the missing size cap, now fixed.
- **`file://` was never reachable through reqwest.** It rejects non-http(s) schemes
  on both the initial request (`src/async_impl/client.rs:2581-2584`) and on every
  redirect hop (`src/redirect.rs:319-321`).

What *is* reachable: any `http://` or `https://` URL, including loopback,
RFC1918 and link-local, with up to 10 redirects followed by default
(`redirect.rs:160-165`) — so `https://attacker.example/r` → `http://127.0.0.1:11434/…`
works. Same blind-SSRF caveat as F1, and gated on a track change, so it is the
lesser of the two paths. It is listed separately because the fix is the same one
input validation.

### F4 — The navigation allowlist admits user-content domains — Low-Medium

The working tree replaced the substring match with a correct registrable-domain
check. **Confirming the form is right**: `is_allowed_host` (`lib.rs:56-61`) strips
a trailing root dot, lowercases, and tests `host == domain || host.ends_with(".{domain}")`.
That is the correct shape — it rejects both `youtube.com.evil.com` and
`evilgoogle.com`, and the same helper now backs the page-load check, so the old
missing-leading-dot bug on `ends_with("google.com")` is gone. The scheme guard
(`!matches!(url.scheme(), "http" | "https") → return true`) also closes a bug
nobody flagged: at HEAD, *any* non-matching URL including custom app schemes was
passed to `open::that`, turning page content into an arbitrary-URI-scheme launcher
for whatever handlers the user has installed.

**What to reconsider**: `ALLOWED_DOMAINS` (`lib.rs:12-20`) includes
`googleusercontent.com` and bare `google.com`. `*.googleusercontent.com` is
Google's *user-content* CDN — it serves content uploaded by arbitrary third
parties, and Apps Script web apps render there. It is not a Google-authored origin
in any meaningful sense. Combined with F2 (app commands are ungated), a page an
attacker got hosted there loads in the main window with a live IPC channel.

This is Low-Medium rather than higher because getting attacker HTML served from
that origin *and* getting the window to navigate there is a real chain, not a
one-liner. The remediation is cheap: navigation and IPC do not need the same list.
Keep the CDN domains for navigation if OAuth/media needs them, but make sure the
IPC gate (F2's fix) is `music.youtube.com` only.

### F5 — Notification spoofing — Low

`payload.title` and `payload.artist` flow unmodified into
`UNMutableNotificationContent.title` / `.subtitle`
(`macos_notifications.m:80-83`), with an attacker-chosen image attached
(F1/F3 fetch it). The result is an OS-level notification, attributed to "YTM
Yagami", with fully attacker-controlled text and artwork — outside the app window,
where the user has no origin context.

Bounding it honestly:

- **Not a memory-safety or injection issue.** `stringWithUTF8String:` and direct
  property assignment; no `stringWithFormat:`, so no format-string bug. Input is
  always valid UTF-8 (it comes from a Rust `String`), so the nil-return path cannot
  fire. Interior NULs truncated at HEAD via `CString::new` returning `Err`; the
  working tree's `to_cstring` strips them instead (`macos_notifications.rs:56-58`),
  which is better behaviour and no worse for safety.
- **Not clickable to a URL.** The delegate implements only
  `willPresentNotification:` (`macos_notifications.m:9-18`). There is no
  `didReceiveNotificationResponse:` handler and no `userInfo`, so clicking the
  notification just activates the app. A phishing notification can *say* anything
  but cannot navigate anywhere.
- **No length cap** on either field in any version. macOS truncates in display, so
  the practical effect is memory churn, not overflow.

Realistic worst case: a convincing "Your session expired — sign in at …" banner
that the user must then act on manually. That is a real social-engineering
primitive and it does clear bar 2 (page JS cannot post an OS notification
attributed to a signed desktop app). It stays Low because it requires the user to
take a second, independent action.

### F6 — Unbounded thread spawn and notification flood — Low

`std::thread::spawn` per notification-worthy invoke (`lib.rs:56` at HEAD,
`lib.rs:93` in the working tree), with no pool, no cap, no in-flight counter. The
`should_notify_track_change` gate does not help: a script just increments the title
each call. Each thread does a blocking HTTP GET (10–30 s), allocates up to 8 MB,
and writes a file.

10,000 invokes → 10,000 OS threads, each with an 8 MB stack reservation, plus up to
80 GB of downloads if the attacker also controls the art host. Also, every one posts
a `UNNotificationRequest`, so macOS displays a banner per call — an OS-level
annoyance that escapes the app window even though
`removeAllDeliveredNotifications` keeps the list itself clean.

Low, because this is fundamentally the page DoSing the app the user chose to run —
a `while(true)` in JS achieves most of it. The part that clears bar 2 is the native
thread and memory exhaustion, and the notification banners. A bounded worker
(single thread with a one-slot mailbox, latest-wins) fixes it and is also the right
design regardless of security.

### F7 — `core:menu:*` allows app-menu-bar spoofing — Low

Live and unconstrained (see Part 1). A script can call `set_as_app_menu` with a
menu it authored and `popup` a context menu at arbitrary screen coordinates.
Menu activations only deliver a `menu` event back to JavaScript — they cannot
invoke native actions the page could not otherwise invoke — so the ceiling is a
convincing fake "YTM Yagami ▸ Preferences ▸ Sign in again…" that leads back into
page-controlled UI. `create_default` can also assemble predefined items (Copy,
Paste, Quit, …), but activating any of them still requires a deliberate user click
on a menu.

Low. Nuisance and social engineering, not privilege. Worth removing simply because
the app never uses the menu API at all.

### F8 — `core:path:resolve_directory` leaks local paths — Low

Returns real absolute paths for the standard directories, which include the local
username (`/Users/<name>/…`) and the app-data layout. Pure information disclosure
into a page that already knows a great deal about the user. Notable only as
fingerprinting material and because the app has no use for it.

### F9 — `csp: null` — Informational, and not the right lever

`csp: null` in `tauri.conf.json:12` is very close to a no-op for this app, and a
CSP is **not** an available control here. Verified: the configured CSP is applied
in exactly one place, `AppManager::get_asset`
(`tauri-2.10.3/src/manager/mod.rs:423-455`), whose only caller is the `tauri://`
custom-protocol handler (`src/protocol/tauri.rs:215-217`). It rewrites the
`<head>` of *bundled* HTML and sets a `Content-Security-Policy` response header on
*bundled* responses.

This window is built with `WebviewUrl::External("https://music.youtube.com")`
(`lib.rs:78-82` at HEAD). Nothing is served through the `tauri://` protocol, so
the CSP config never executes. The page's CSP is whatever Google's servers send,
and there is no supported way to inject one into a remote response.

**Setting a CSP would buy nothing.** Do not spend effort here. The lever that
matters is the ACL, not content policy — which is Part 3.

### F10 — `withGlobalTauri: true` — Informational, and also not a control

Verified: `with_global_tauri` gates only `plugin_global_api_scripts`
(`tauri-codegen-2.5.5/src/context.rs:430-431`), the convenience `window.__TAURI__`
JS API bundle. The bootstrap that actually matters — `window.__TAURI_INTERNALS__`
with `invoke`, `transformCallback` and the invoke key — is pushed unconditionally
for every webview (`tauri-2.10.3/src/manager/webview.rs:165-190`), and this app's
own `inject.js:33-35` calls `__TAURI_INTERNALS__.invoke` directly, not the sugar.

So turning `withGlobalTauri` off removes an attacker's convenience wrapper and
breaks nothing. Do it — it costs nothing — but do not count it as a mitigation. A
hostile script writes `__TAURI_INTERNALS__.invoke(...)` and is entirely unaffected.

### F11 — devtools feature enabled in release builds — Low

`Cargo.toml:22`: `tauri = { version = "2", features = ["devtools"] }`, with no
`cfg(debug_assertions)` restriction. That compiles the Web Inspector into shipped
release binaries and injects the devtools toggle hotkey script into every page
(`tauri-2.10.3/src/webview/plugin.rs:205-222`), backed by
`core:webview:internal_toggle_devtools`, which *is* in the default permission set.

Mostly a footprint and support-surface issue rather than an attack: opening the
inspector requires physical access, which already implies more capability. Move it
behind a debug-only dependency anyway.

### Not a finding — the shared temp path

The brief flags `std::env::temp_dir()/com.ytmyagami.desktop-notifications/album-art.{ext}`
as a symlink/TOCTOU target on a multi-user machine. **It is not, on the only
platform this code compiles for.** `macos_notifications.rs` is entirely behind
`#[cfg(target_os = "macos")]`, and on macOS `std::env::temp_dir()` resolves
`$TMPDIR`, which launchd sets to a per-user, per-boot directory under
`/var/folders/…/T/` with mode 0700 owned by the user. Another user on the machine
cannot create the directory or plant symlinks in it. A same-user attacker who could
already has the ability to write the target file directly.

Two secondary notes, both benign: the extension is not attacker-controlled (it
comes from `infer`'s fixed compile-time table, so no traversal via `{ext}`), and
`fs::remove_file` on a symlink removes the link rather than the target. On Linux,
where `temp_dir()` is world-writable `/tmp`, this would be a real finding — worth
remembering if the notification code is ever ported.

### Not a finding — `*.youtube.com` wildcard depth

The brief asks about the wildcard subdomain match. Verified: Tauri parses
`remote.urls` entries as WHATWG URLPatterns (`tauri-utils-2.8.3/src/acl/mod.rs:280-301`),
and a bare `*` compiles to the full-wildcard regex `.*`
(`urlpattern-0.3.0/src/parser.rs:9`), so `https://*.youtube.com/*` matches any
subdomain at any depth. This is worth knowing but is not itself the problem — the
problem is that `www.youtube.com` (a comment-bearing, far larger attack surface
than the music player) is in scope at all, and more importantly that per F2 this
list does not gate the app's own commands regardless.

---

## Part 3 — Remediation plan

The controlling insight is F2: **the capability file currently governs only
commands this app does not use, and does not govern the two commands it does.**
Fixing that is worth more than everything else in the ACL combined.

Three changes, in priority order.

### 3.1 Validate `TrackState` in Rust (fixes F1, F3, F5, and half of F6)

Do this first. It is the only fix that does not depend on getting an origin check
right, and it holds even if an attacker satisfies every origin precondition.

Add to `src-tauri/Cargo.toml` (`url` is already in the graph transitively; declare
it explicitly):

```toml
url = "2"
```

Then in `src-tauri/src/lib.rs`, alongside `TrackState`:

```rust
/// Track text is attacker-influenced: it comes from page script, which is not
/// necessarily Google's. Cap it before it reaches an OS notification.
const MAX_TEXT_CHARS: usize = 200;
/// Longer than any real artwork URL; a cheap bound before parsing.
const MAX_ART_URL_BYTES: usize = 2048;

/// Registrable domains Google serves YouTube Music artwork from.
///
/// `art` is not just displayed — it is fetched by the *native* process, by
/// `reqwest` for the notification attachment and by `NSImage
/// initWithContentsOfURL:` inside souvlaki for the now-playing artwork. Both run
/// outside every restriction the webview enforces (CORS, PNA, mixed content,
/// the page CSP), so an unvalidated URL here is a request-forgery primitive
/// pointed at loopback and the LAN. Anything not on this list is dropped.
const ART_DOMAINS: &[&str] = &["googleusercontent.com", "ggpht.com", "ytimg.com"];

/// Strip control characters (which corrupt notification layout and cannot cross
/// the Objective-C string bridge) and cap the length.
fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Return the URL only if it is an https URL on a known Google artwork host.
/// Anything else becomes empty, which both consumers already treat as
/// "no artwork" rather than as an error.
fn sanitize_art_url(value: &str) -> String {
    if value.len() > MAX_ART_URL_BYTES {
        return String::new();
    }
    let Ok(url) = url::Url::parse(value) else {
        return String::new();
    };
    // https only: an http URL on a hijackable network is attacker-controlled,
    // and a non-http scheme is what makes the NSImage path a file-read probe.
    if url.scheme() != "https" {
        return String::new();
    }
    // Credentials and non-default ports are never present on real artwork URLs
    // and are how a lookalike host is smuggled past a naive check.
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return String::new();
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let allowed = ART_DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if allowed {
        url.into()
    } else {
        String::new()
    }
}

impl TrackState {
    /// Normalize page-supplied state before any of it reaches the OS.
    fn sanitized(self) -> Self {
        Self {
            title: sanitize_text(&self.title),
            artist: sanitize_text(&self.artist),
            art: sanitize_art_url(&self.art),
            is_playing: self.is_playing,
        }
    }
}
```

and make it the first thing the command does:

```rust
#[tauri::command(rename_all = "snake_case")]
fn handle_track_changed(payload: TrackState, state: tauri::State<'_, AppState>) {
    let payload = payload.sanitized();
    ...
}
```

Notes on the choices:

- **`take(MAX_TEXT_CHARS)` over `chars()`, not bytes.** Truncating a UTF-8 byte
  slice mid-codepoint panics; truncating by `char` cannot.
- **Empty string rather than rejecting the whole payload.** Both consumers already
  treat empty `art` as "no artwork" — `macos_notifications.rs` checks
  `art_url.trim().is_empty()` and the working tree's `media.rs:update` maps empty
  to `cover_url: None`. So a bad URL degrades to a text-only notification instead
  of dropping the track update, which is the right failure mode for something
  purely decorative.
- **The allowlist is by registrable domain**, using the same `host == d ||
  ends_with(".{d}")` shape as `is_allowed_host`. Do not use `contains`.
- `lh3.googleusercontent.com` is the usual host; the suffix form covers
  `lh4`/`lh5`/… without churn.

Also cap the concurrency (F6). Replace the per-invoke `std::thread::spawn` with a
single long-lived worker thread fed by a one-slot latest-wins mailbox
(`Mutex<Option<Job>>` + `Condvar`, or a `sync_channel(1)` with `try_send` that
drops on full). One notification at a time is correct behaviour anyway — the app
already calls `removeAllDeliveredNotifications` before each one.

### 3.2 Give the app an ACL manifest and shrink the capability (fixes F2, F7, F8, and the latent F1-style image issue)

Right now the capability grants a large set of commands the app never calls, and
grants nothing for the two it does. Invert that.

**`src-tauri/build.rs`** — declare the app's commands so they become ACL-gated:

```rust
fn main() {
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("src/macos_notifications.m")
            .flag("-fobjc-arc")
            .flag("-mmacosx-version-min=11.0")
            .compile("macos_notifications");

        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=UserNotifications");
    }

    // Declaring the app's commands here creates an app ACL manifest. Without it
    // Tauri skips the ACL entirely for non-plugin commands, so the capability's
    // `remote.urls` allowlist does not apply to them at all.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["handle_track_changed"]),
        ),
    )
    .expect("failed to run tauri-build");
}
```

This autogenerates `src-tauri/permissions/autogenerated/handle_track_changed.toml`
defining `allow-handle-track-changed` / `deny-handle-track-changed`
(`tauri-utils-2.8.3/src/acl/build.rs`, `autogenerate_command_permissions` — the
identifier is `allow-` plus the command name with `_` → `-`). Commit that file.

**`src-tauri/capabilities/default.json`** — proposed contents in full:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "The remote page needs no core Tauri APIs. It invokes exactly one app command, from exactly one origin.",
  "windows": ["main"],
  "remote": {
    "urls": ["https://music.youtube.com/*"]
  },
  "permissions": ["allow-handle-track-changed"]
}
```

What changed and why:

- **`core:default` is gone entirely.** The app calls no core command from
  JavaScript — `inject.js` uses only `__TAURI_INTERNALS__.invoke('handle_track_changed')`.
  This closes F7 and F8 and, more importantly, means a future `tray-icon` or
  `image-png` feature flag cannot silently re-arm `core:tray:*` or
  `core:image:from_path`.
- **`https://*.youtube.com/*` and `https://accounts.google.com/*` are gone.**
  The OAuth flow calls no Tauri command; it just needs to *navigate*, which
  `on_navigation` governs and the capability does not. `www.youtube.com` is a far
  larger attack surface than the player and has no reason to hold an IPC channel.
- **`request_notification_permission` is not listed** — it should be **removed
  from `generate_handler!`** instead. It is only ever called from Rust
  (`lib.rs:76` in `setup`), and no JavaScript in this repo invokes it. Note that
  once an app manifest exists this is not optional bookkeeping: any app command
  not listed in a capability becomes unreachable from JS, which is the desired
  outcome here, but it will *silently* be the outcome for any command someone adds
  later. Worth a comment in `lib.rs`.

**Verify after the change** that a `handle_track_changed` invoke from the player
still succeeds and that one from `www.youtube.com` is rejected. In debug builds the
rejection message names the missing permission (`webview/mod.rs:1806-1826`), so a
regression here is loud rather than silent.

### 3.3 Config and dependency hygiene (F10, F11, F4)

**`src-tauri/tauri.conf.json`** — set `"withGlobalTauri": false`. Nothing in this
repo uses `window.__TAURI__`. Per F10 this is not a security control; it is
removing an unused injected bundle.

Leave `"csp": null` alone, or delete the `security` block entirely. Per F9 it
cannot apply to a remote page. Do not let a scanner or a checklist talk anyone into
"adding a CSP" here — there is nowhere for it to attach, and pretending otherwise
creates false assurance.

**`src-tauri/Cargo.toml`** — make devtools debug-only:

```toml
[dependencies]
tauri = { version = "2", features = [] }

[target.'cfg(debug_assertions)'.dependencies]
tauri = { version = "2", features = ["devtools"] }
```

(Feature unification means this needs checking against how the release build is
actually produced; if it does not take, a plain `devtools` cargo feature on
`ytm-yagami` enabled only in the dev profile is the reliable form.)

**`src-tauri/src/lib.rs`** — reconsider `googleusercontent.com` in
`ALLOWED_DOMAINS` per F4. If media playback needs it, keep it for navigation but
ensure 3.2 is in place so navigating there does not confer IPC. If nothing breaks
without it, drop it.

---

## Summary

| # | Finding | Severity | Fixed by |
|---|---|---|---|
| F1 | `art` → `NSImage initWithContentsOfURL:` via souvlaki: no scheme check, no timeout, every invoke | Medium-High | 3.1 |
| F2 | App commands bypass the ACL entirely (no app manifest) | Medium | 3.2 |
| F3 | `art` → `reqwest`: no scheme or host allowlist | Medium-Low | 3.1 |
| F4 | Navigation allowlist includes user-content domains | Low-Medium | 3.2 + 3.3 |
| F5 | Notification text/image spoofing | Low | 3.1 |
| F6 | Unbounded thread spawn; notification flood | Low | 3.1 |
| F7 | `core:menu:*` app-menu-bar spoofing | Low | 3.2 |
| F8 | `core:path:resolve_directory` path disclosure | Low | 3.2 |
| F9 | `csp: null` | Informational — not fixable, not the lever | — |
| F10 | `withGlobalTauri: true` | Informational — not a control | 3.3 |
| F11 | devtools compiled into release builds | Low | 3.3 |
| — | Shared temp path symlink/TOCTOU | Not a finding on macOS | — |
| — | `*.youtube.com` wildcard depth | Not a finding on its own | 3.2 |

The single highest-value change is **3.1**: validating `art` closes both SSRF paths
at once and does not depend on any origin check being correct. **3.2** is the
structural fix — it makes the capability file actually govern the surface this app
exposes, rather than a surface it does not use.
