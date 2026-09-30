# IPC transport hack & Chrome spoof — investigation handoff

Status: COMPLETE (Q1–Q5). Investigator: Fable (subagent), 2026-08-10.

Update 2026-09-30: `chrome_spoof.js` is gone. macOS now sends a Safari UA built from the installed Safari's version (`src-tauri/src/user_agent.rs`), and other platforms keep their engine's own UA, so Q4's version-drift problem does not apply. The fetch guard is now `src/ipc_transport.js`, as Q5-iii recommends. Step 0 has not been run yet.

Scope: `src/chrome_spoof.js` does two conflated jobs: (A) Chrome-environment spoofing so Google OAuth works, and (B) a `window.fetch` monkeypatch that rejects `ipc://` URLs to force Tauri's postMessage IPC transport. This document pins down how Tauri 2.10.3 actually negotiates IPC transport, whether the hack is necessary, what collateral damage it causes, and how to make the whole thing maintainable.

Verification notes: "VERIFIED" = read directly from source (this repo, or the vendored crate at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-2.10.3/`). "INFERRED" = reasoned from source but not observed at runtime; the app was not launched during this investigation.

## Contents

1. [Q1 — How Tauri 2.10.3 chooses its IPC transport](#q1)
2. [Q2 — Is there a supported mechanism that replaces the hack?](#q2)
3. [Q3 — Collateral damage of the fetch monkeypatch](#q3)
4. [Q4 — Durability of the UA spoof](#q4)
5. [Q5 — Recommended fix](#q5)

<a name="q1"></a>
## Q1 — How Tauri 2.10.3 chooses its IPC transport

All paths below are relative to `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-2.10.3/` unless prefixed. Versions pinned from `src-tauri/Cargo.lock`: tauri 2.10.3 (line 3695-3696), wry 0.54.4 (line 5261-5262), tauri-build 2.5.6.

### The invoke pipeline (VERIFIED, read from bundled scripts)

`__TAURI_INTERNALS__.invoke` — the entry point `src/inject.js:50` uses — is defined in `scripts/core.js:81-113`. It registers a success and an error callback, then calls `window.__TAURI_INTERNALS__.ipc({cmd, callback, error, payload, options})`.

`__TAURI_INTERNALS__.ipc` is defined in `scripts/ipc.js:142-180`. For the default **brownfield** pattern (this app configures no `app.security.pattern` in `src-tauri/tauri.conf.json`, so brownfield applies) it immediately delegates to `window.__TAURI_INTERNALS__.postMessage(message)` (`scripts/ipc.js:145-148`). Despite the name, this is *not* the transport decision — it is `sendIpcMessage`, installed by `scripts/ipc-protocol.js:88-90`.

**`sendIpcMessage` (`scripts/ipc-protocol.js:22-86`) is the entire transport negotiation.** Verbatim logic:

1. State: `let customProtocolIpcFailed = false` (`ipc-protocol.js:17`) — a closure variable in an init-script IIFE, so it **resets on every page navigation** and is shared by all invokes on that page.
2. `const canUseCustomProtocol = osName !== 'android'` (`ipc-protocol.js:20`).
3. If `!customProtocolIpcFailed && (canUseCustomProtocol || cmd === fetchChannelDataCommand)`: it POSTs `fetch(window.__TAURI_INTERNALS__.convertFileSrc(cmd, 'ipc'), {method: 'POST', headers: {Tauri-Callback, Tauri-Error, Tauri-Invoke-Key, Content-Type}, body})` (`ipc-protocol.js:26-41`).
4. `convertFileSrc` (`scripts/core.js:13-20`): on macOS/Linux/iOS the URL is `ipc://localhost/<cmd>`; on Windows/Android it is `http(s)://ipc.localhost/<cmd>`.
5. **The fallback**: if that fetch's promise **rejects**, the rejection handler (`ipc-protocol.js:59-68`) does `console.warn('IPC custom protocol failed, Tauri will now use the postMessage interface instead', e)`, sets `customProtocolIpcFailed = true`, and **re-sends the same message** — no invoke is lost. The comment in the crate names the exact scenario: *"failed to use the custom protocol IPC (either the webview blocked a custom protocol or it was a CSP error) so we need to fallback to the postMessage interface"*.
6. The postMessage branch (`ipc-protocol.js:70-85`) serializes the whole message (adding `options.customProtocolIpcBlocked: true`) and calls `window.ipc.postMessage(data)`. `window.ipc` is installed by wry's own init script on every page of the webview — `wry-0.54.4/src/wkwebview/mod.rs:637-641`: `window.ipc = Object.freeze({postMessage: s => window.webkit.messageHandlers.ipc.postMessage(s)})`.

So: **transport choice is runtime trial-and-error with a per-page-load latch, not configuration.** Custom-protocol fetch is tried first on every fresh page; the first rejection permanently (for that page) switches all subsequent invokes to `postMessage`. There is no config knob in 2.10.3 to pre-select the transport.

### The Rust side (VERIFIED)

- The `ipc://` custom protocol is registered unconditionally for every webview: `src/manager/webview.rs:274-279`.
- Custom-protocol requests land in `src/ipc/protocol.rs:38-183`. Every response gets `Access-Control-Allow-Origin: *` (`protocol.rs:48-57`) — remote origins are *deliberately served*. `parse_invoke_request` (`protocol.rs:435-556`) requires the `Origin` header (`protocol.rs:492-500`) and uses it as the ACL origin URL, plus the `Tauri-Invoke-Key` header.
- postMessage requests land in `handle_ipc_message` (`protocol.rs:185+`); here the origin URL is the webview's current URL passed by wry (`protocol.rs:302`).
- Both funnel into `Webview::on_message` (`src/webview/mod.rs:1724`), which first verifies the invoke key (`webview/mod.rs:1727-1745`; the key is injected into the page as a main-frame init script, so remote pages in this webview have it), then classifies the origin: `is_local_url` (`webview/mod.rs:1680-1720`) — `https://music.youtube.com` is **Remote**.
- **ACL twist (VERIFIED, important):** `webview/mod.rs:1801-1805` — *"we only check ACL on plugin commands or if the app defined its ACL manifest."* This app has no app ACL manifest: `tauri_build::build()` with the default `AppManifest` (no commands, no `src-tauri/permissions/` dir — verified in `src-tauri/build.rs`) produces none (`tauri-build-2.5.6/src/acl.rs:404-410` inserts the app manifest only when it has permissions). Therefore **`handle_track_changed` and `request_notification_permission` bypass the ACL entirely — from any origin, on either transport.** The `remote` field in `capabilities/default.json` gates only *plugin* commands (`core:default`), none of which this app's JS currently calls.

### On macOS, does the `ipc://` fetch reject naturally on a remote https page? (INFERRED)

wry registers `ipc` via `WKWebViewConfiguration.setURLSchemeHandler` with no CORS/fetch enablement (`wry-0.54.4/src/wkwebview/mod.rs:249-292` — there is no WKWebView API for that). WebKit does not treat app-handled custom schemes as CORS-fetchable from an http(s) document, so a `fetch('ipc://localhost/...')` from `https://music.youtube.com` is blocked by the webview as a network/access-control error → the promise **rejects** → Tauri's own fallback fires. Three independent signals support this: (a) Tauri's fallback comment names exactly this case ("the webview blocked a custom protocol"); (b) the hack's own comment and commit `81ef66e` record it empirically ("ipc:// custom protocol is blocked on HTTPS pages"); (c) on Linux wry registers the scheme as *secure* but **not** CORS-enabled (`wry-0.54.4/src/webkitgtk/web_context.rs:136-142` calls only `register_uri_scheme_as_secure`), so the same block applies under webkitgtk. Not observed at runtime in this investigation.

### Verdict: is the monkeypatch necessary in 2.10.3?

**Mechanically, no.** The monkeypatch produces a rejected promise for `ipc://` fetches — which is *byte-for-byte the same code path* (`ipc-protocol.js:59-68`) that the natural WebKit block produces. If WebKit blocks the fetch (the expected case), the fallback fires without the hack. If WebKit somehow allowed the fetch, the Rust handler serves remote origins anyway (ACAO:*, Origin-header ACL) and the invoke would succeed over the custom protocol. **Either way, `invoke` works without the hack.**

What the hack *actually* changes:

1. The rejection is synchronous and deterministic — the `ipc://` request never enters WebKit's fetch stack.
2. Consequently **no CSP-violation event or report is generated** on Google's pages, and no `console.warn` noise per navigation.
3. It forecloses any WebKit edge behavior (hangs, policy-delegate weirdness) for custom-scheme subresource loads.

Commit `81ef66e` (2026-04-17) claims *"block ipc:// fetches on remote pages (breaks Google OAuth otherwise)"*. **The causal mechanism for OAuth breakage is not reproducible from source**: no invoke is initiated on `accounts.google.com` pages at all (`inject.js` is injected only on `music.youtube.com`, `lib.rs:137-139`; `chrome_spoof.js` performs no invokes; Tauri's init scripts perform no page-load invokes — verified against `scripts/init.js` and `src/event/mod.rs:224-240`). The most plausible mechanism: Google pages run CSP with reporting endpoints; a blocked `ipc://localhost` fetch on `music.youtube.com` (where the first track-change invoke can fire mid-login-redirect) generates a `securitypolicyviolation` + report that feeds Google's bot heuristics. That is a guess. Note the same commit also removed `opener:default` from capabilities, so the observed "OAuth fixed" may have been multi-causal. **Treat "hack is load-bearing for OAuth" as an empirical claim needing one runtime experiment** (see Q5).

### The Tauri-bump hazard, precisely

The coupling is to three undocumented implementation details, all in tauri's bundled `scripts/ipc-protocol.js`:
- invoke tries `fetch()` first with a URL that is a **string** starting with `ipc://` (the hack's guard is `typeof url === 'string' && url.startsWith('ipc://')` — if a Tauri bump switches to `new Request(...)`, a `URL` object, or a different scheme, the hack silently stops matching);
- a **rejection** of that fetch triggers a fallback (if a bump removes the fallback or makes rejection fatal, all IPC dies);
- the fallback is `window.ipc.postMessage` (wry-provided, also unversioned).

Note the hack is **already a no-op on Windows/Android builds**: there `convertFileSrc` produces `http(s)://ipc.localhost/<cmd>`, which the `ipc://` prefix guard does not match (`scripts/core.js:16-18`). Those platforms already run on the un-hacked, natural negotiation path today.

<a name="q2"></a>
## Q2 — Is there a supported mechanism that makes the hack unnecessary?

**Yes — it is the fallback itself.** The automatic custom-protocol → postMessage fallback in `scripts/ipc-protocol.js:59-68` *is* Tauri 2's supported mechanism for running IPC on pages where the custom protocol is unreachable (remote URLs, CSP blocks). It exists precisely for this app's situation, retries the failed message, and latches for the page's lifetime. Nothing needs to be forced for `invoke` to work from `https://music.youtube.com`.

The supported configuration surface, item by item:

- **`remote` in capabilities (`src-tauri/capabilities/default.json`)** — already present. This is the v2 replacement for v1's `app.security.dangerousRemoteDomainIpcAccess`; it declares which remote URLs may execute the capability's *permissions*. In this app it currently authorizes `core:default` plugin commands for the listed Google/YouTube origins. It does **not** affect transport, and — per the Q1 ACL twist — it is *not* what authorizes `handle_track_changed` (app commands bypass ACL because no app ACL manifest exists). Keep it: it is required the moment the page calls any plugin command (e.g. the event API), and it documents intent.
- **`app.withGlobalTauri`** — irrelevant. It only additionally injects the bundled `window.__TAURI__` API (`scripts/bundle.global.js`); `__TAURI_INTERNALS__.invoke` and the whole transport stack are injected regardless, main-frame-only, on every page of the webview including remote ones. This app has it `false` and `inject.js` correctly uses `__TAURI_INTERNALS__` directly.
- **A supported "force postMessage" switch** — does not exist in 2.10.3. Grepping the crate for the negotiation shows no config reaching `sendIpcMessage` other than the OS name and the runtime latch. (Ironically Android is hardcoded to postMessage-only — `ipc-protocol.js:19-20` — so postMessage-only operation is a first-class, permanently-supported mode of the protocol, not a deprecated remnant.)
- **App ACL for own commands (recommended hardening, orthogonal)** — defining `tauri_build::Attributes::app_manifest(AppManifest::new().commands(&["handle_track_changed", "request_notification_permission"]))` plus capability entries would put the app's own commands under the `remote` gate instead of the current allow-everything bypass. See teammate fable-security's thread for the security angle.

**Headline recommendation: delete the fetch monkeypatch.** Tauri's own negotiation already lands on postMessage on remote origins; the hack's only defensible residual value is suppressing the one doomed `ipc://` fetch per page load (console noise + possible CSP-report telemetry to Google). That residual concern is empirical and cheap to test (Q5), and even if it turns out real, the right shape is a tiny, separately-documented `ipc_transport.js` — not a line hidden inside a fingerprint-spoofing file.

<a name="q3"></a>
## Q3 — Collateral damage of the fetch monkeypatch

The patch (`src/chrome_spoof.js:10-16`): `window.fetch = function(url) { if (typeof url === 'string' && url.startsWith('ipc://')) return Promise.reject(new TypeError(...)); return originalFetch.apply(this, arguments); }`.

### Functional correctness — mostly fine (VERIFIED by reading the patch)

- **`fetch(new Request(...))` is NOT broken.** The `typeof url === 'string'` guard means non-string inputs (Request, URL objects) skip the block and are forwarded verbatim via `originalFetch.apply(this, arguments)`, which preserves both arguments and `this`. The only consequence is that an `ipc://` URL wrapped in a Request object would *bypass* the block — irrelevant today because Tauri 2.10.3 always calls fetch with a string (`ipc-protocol.js:37` with `convertFileSrc`'s string return), but it is exactly the kind of thing a Tauri bump could change silently (see Q1 hazard list).
- **`fetch.length`** — actually *matches* native. Per WebIDL, optional arguments don't count toward `Function.length`, so native `fetch.length === 1`; the patched arity-1 function is also 1. Not a tell.
- **`fetch.name`** — a tell. Assignment to a member expression (`window.fetch = function(url){...}`) confers no inferred name, so `fetch.name === ''` vs native `'fetch'`.
- **`fetch.toString()`** — the loudest tell: returns the patch's source instead of `function fetch() { [native code] }`. `Function.prototype.toString.call(fetch)` cannot be fooled without also patching `toString`. Google's anti-abuse scripts (botguard) are known to fingerprint native functions this way.

### The irony (VERIFIED)

The same file installs `navigator.webdriver → false` (`chrome_spoof.js:83-85`) and fake plugins to *evade* fingerprinting, while the fetch patch is trivially detectable. And the spoofs themselves carry tells of the same class:
- Real Chrome defines `webdriver` as a getter on `Navigator.prototype`; the spoof defines an **own property on the `navigator` instance** — `Object.getOwnPropertyDescriptor(navigator, 'webdriver')` returns a descriptor in the spoofed environment and `undefined` in real Chrome.
- `navigator.plugins` returns a plain `Array` of object literals — `navigator.plugins instanceof PluginArray` is `false`, and `item()`/`namedItem()` are missing.
- WKWebView also never sends `Sec-CH-UA*` request headers the way a real Chrome 125 would, so the server side can distinguish regardless of what JS says.

Conclusion: the spoof's goal can only be "pass Google's *current* pragmatic checks", not "be indistinguishable from Chrome". That argues for keeping it minimal and legible, not for growing it.

### Does the patch survive same-origin iframes? (VERIFIED)

No — and neither does anything else here, symmetrically:
- `chrome_spoof.js` is installed via `WebviewWindowBuilder::initialization_script` (`src-tauri/src/lib.rs:122`), which is **main-frame-only** (tauri 2.10.3 `src/webview/webview_window.rs:946-948`; the all-frames variant is a separate method, documented at `webview_window.rs:951-958`). The `on_page_load` re-injection (`lib.rs:123-140`) uses `window.eval`, which also targets the main frame.
- An iframe (Google sign-in widget, YT embeds) therefore has: native `fetch`, **unspoofed** `navigator.webdriver`/`plugins`/`userAgentData`, and no `__TAURI_INTERNALS__`/`window.ipc` either (Tauri's internal scripts are also main-frame-only, `src/manager/webview.rs:160-218`; wry's `window.ipc` script is injected with main-frame-only `true`, `wry-0.54.4/src/wkwebview/mod.rs:637-641`).
- Consequence for IPC: none — no frame that lacks the patch has IPC internals to mispatch. Consequence for spoofing: a frame/top-window **inconsistency** (top window says `webdriver:false` + fake plugins; iframe says defaults) that a fingerprinting script comparing realms can see. The UA *string* is consistent everywhere because it is set natively on the webview (`lib.rs:121`).

One forward-looking fragility worth recording: the patch works only because `ipc-protocol.js:37` calls the **global** `fetch` at invoke time. If a future Tauri hardens its init script by capturing `const f = window.fetch` before user scripts run — a common anti-tamper move — the hack silently stops intercepting. (Today the order is internal-scripts-first, user-scripts-second — `src/manager/webview.rs:216-218` — but interception still works because the call is late-bound.)

<a name="q4"></a>
## Q4 — Durability of the UA spoof half

### Current drift points (VERIFIED)

Chrome "125" is pinned in **two files with three encodings**:
1. `src-tauri/src/lib.rs:5` — `CHROME_UA` = `"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) ... Chrome/125.0.0.0 Safari/537.36"` — applied natively to the webview (`lib.rs:121`), so it governs **every HTTP request** on **every platform**: Windows and Linux builds send a *Macintosh* UA.
2. `src/chrome_spoof.js:7-8` — `CHROME_VERSION = '125'`, `CHROME_FULL_VERSION = '125.0.6422.142'` — governs `navigator.userAgentData` brands and high-entropy values.

Nothing ties them together; bumping one and not the other yields a major-version mismatch between the UA string and `userAgentData.brands` — a fingerprint stronger than an old version.

Hardcoded platform lies (`chrome_spoof.js:52,55,61-62`): `platform: 'macOS'`, `architecture: 'arm'`, `platformVersion: '15.0.0'`. Two observations:
- On **macOS**, the combination is actually coherent with real Chrome behavior: Chrome's frozen (reduced) UA on Mac reports `Intel Mac OS X 10_15_7` even on Apple Silicon, while `getHighEntropyValues` reports `architecture: 'arm'`. So the Mac story is fine.
- On **Windows/Linux builds**, both halves claim to be a Mac. That is *internally* consistent but externally absurd (an "arm Mac" whose TCP stack, fonts, codecs, and rendering all scream Windows), and `platformVersion: '15.0.0'` pins a specific macOS that will age.

Version age: Chrome 125 is May-2024 stable; as of this writing it is ~2 years stale. Google already gates OAuth on browser heuristics ("This browser or app may not be secure"); a UA whose major trails stable by dozens of releases is a standing risk of exactly the failure this spoof exists to prevent.

### Proposed maintainable design

**Single source of truth in Rust, injected into JS at compile time.**

1. Keep one pair of consts in `lib.rs` (or a tiny `chrome_version.rs`):
   `const CHROME_MAJOR: &str = "125"; const CHROME_FULL: &str = "125.0.6422.142";`
2. Derive `CHROME_UA` from them with a platform-selected frozen-UA token (these exact tokens are what real reduced-UA Chrome sends):
   - macOS: `Macintosh; Intel Mac OS X 10_15_7`
   - Windows: `Windows NT 10.0; Win64; x64`
   - Linux: `X11; Linux x86_64`
   selected with `cfg!(target_os = ...)`, and `Chrome/{CHROME_MAJOR}.0.0.0`.
3. Turn `chrome_spoof.js` (post-split: `ua_spoof.js`) into a template with placeholders — `__CHROME_MAJOR__`, `__CHROME_FULL__`, `__UA_PLATFORM__` (`'macOS'`/`'Windows'`/`'Linux'`), `__UA_ARCH__` (`'arm'` on macOS-aarch64, `'x86'` elsewhere), `__UA_PLATFORM_VERSION__` — and have `lib.rs` do `include_str!(...).replace("__CHROME_MAJOR__", CHROME_MAJOR)...` once at startup (or in `build.rs` for zero runtime cost). The script is ~2 KB; runtime `.replace` at window-build time is negligible.
4. `platformVersion`: either a per-OS constant reviewed at bump time, or read the real OS version at startup (macOS: already have ObjC glue available) — a real value is strictly more plausible.

**Version-drift strategy.** A scheduled CI job (e.g. monthly GitHub Actions cron):
- fetch current stable from Chrome's version endpoints — `https://versionhistory.googleapis.com/v1/chrome/platforms/mac/channels/stable/versions` or `https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions.json`;
- compare the major against `CHROME_MAJOR` (grep it out of the one file that owns it — another reason for a single source);
- fail / open an issue (or an automated PR editing the two consts) when the pin trails stable by ≥ N majors (N=2–3 is a sane threshold; matching exactly is unnecessary — real users run slightly-stale Chromes, and every bump is a small re-verification of OAuth).
Bumping then touches exactly one Rust const pair, and the UA string, brands list, and full-version list can never disagree again.

<a name="q5"></a>
## Q5 — The fix

### Step 0: the one runtime experiment this document could not run

Build with the fetch monkeypatch deleted (leave everything else untouched). Verify, signed out, on macOS:
1. Full OAuth sign-in completes.
2. On `music.youtube.com`, the devtools console shows exactly one `IPC custom protocol failed, Tauri will now use the postMessage interface instead` warning per page load (that is Tauri's fallback firing naturally — expected, not a bug).
3. Track-change notifications and media keys work (i.e. `handle_track_changed` invokes arrive).

If OAuth still works → the hack was redundant, delete it permanently. If OAuth demonstrably breaks → keep the patch but as its own file (see iii), with a comment linking commit `81ef66e` and this document, and note that it is a *telemetry suppressor*, not a transport requirement.

### (i) Loud failure: a startup IPC self-test

Whether or not the hack survives Step 0, IPC death must stop being silent. `src/inject.js:50-56` already `console.error`s failed invokes — necessary but invisible unless devtools is open. Build on it with a **two-sided** self-test, because the failure mode being defended against (a Tauri bump changing transport negotiation) can kill JS→Rust delivery entirely, and only the Rust side can be trusted to notice *absence*:

- **JS side** (in `inject.js`, or a 10-line `ipc_selftest.js` injected alongside it on the player host): immediately invoke a trivial new command, with a deadline:
  ```js
  var ping = window.__TAURI_INTERNALS__.invoke('ipc_ping');
  var timeout = new Promise(function(_, rej) { setTimeout(rej, 5000, new Error('timeout')); });
  Promise.race([ping, timeout]).catch(function(err) {
    console.error('[YTM Yagami] IPC self-test failed:', err);
    // visible banner — notifications/media keys are dead, say so in-app
    var el = document.createElement('div');
    el.textContent = 'YTM Yagami: desktop integration broken (IPC failure) — notifications and media keys are disabled.';
    el.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:99999;background:#b00020;color:#fff;padding:8px 16px;font:13px sans-serif;text-align:center';
    (document.body || document.documentElement).appendChild(el);
  });
  ```
- **Rust side**: add `#[tauri::command] fn ipc_ping(state: tauri::State<AppState>) { state.ipc_alive.store(true, Ordering::Relaxed); }` and register it. In `on_page_load` (`lib.rs:123`), when the Finished event is for `PLAYER_HOST`, reset the flag and spawn a thread that sleeps ~10 s and then, if the flag is still false, fires a native notification through the already-existing `macos_notifications` path (plus `eprintln!`). This catches the total-death case where `inject.js` itself never ran or no invoke can be delivered at all — armed by a Rust-side event, so it cannot be suppressed by the JS failure it is detecting.

The round-trip is meaningful: an `ipc_ping` reply exercises fetch-attempt → rejection → postMessage → invoke-key check → (skipped) ACL → command dispatch → response via `webview.eval(runCallback(...))` (`tauri-2.10.3/src/ipc/protocol.rs:323-340`) — the full path `handle_track_changed` depends on.

### (ii) Regression test pinning transport behaviour (no app launch required)

The insight: the transport contract lives in **JS files shipped inside the tauri crate**, and those files are present in the local cargo registry on any machine that can build the app. CI can therefore assert against them directly and fail a Tauri bump *at test time* instead of shipping silently broken IPC.

Two tiers, both cheap:

1. **Checksum tripwire.** Store `sha256` of the crate's `scripts/ipc-protocol.js` and `scripts/ipc.js` in the repo (e.g. `src-tauri/tests/tauri-ipc-scripts.lock`). A `#[test]` (or CI step) locates the crate via `cargo metadata` — `packages[] | select(.name=="tauri") | .manifest_path` — and compares. Any Tauri upgrade that touches IPC scripts turns CI red until a human re-reads the diff and re-blesses the hashes. This catches *everything*, including changes the semantic tier below doesn't anticipate.
2. **Semantic contract asserts** (same test, better error messages — they tell you *whether the load-bearing parts moved* rather than just "something changed"). Against `ipc-protocol.js`, assert it still contains:
   - `convertFileSrc(cmd, 'ipc')` — invoke still tries a custom-protocol **fetch first**, with a **string** URL (the guard condition the hack, if retained, depends on);
   - `customProtocolIpcFailed = true` inside a fetch rejection handler — the **fallback latch still exists**;
   - `window.ipc.postMessage(` — the fallback still lands on wry's postMessage bridge.

   Sketch:
   ```rust
   // src-tauri/tests/ipc_transport_contract.rs
   fn tauri_crate_dir() -> PathBuf { /* cargo metadata → tauri manifest_path parent */ }

   #[test]
   fn tauri_ipc_transport_contract() {
       let s = fs::read_to_string(tauri_crate_dir().join("scripts/ipc-protocol.js")).unwrap();
       assert!(s.contains("convertFileSrc(cmd, 'ipc')"), "invoke no longer fetch()es a string ipc URL — transport negotiation changed; re-audit docs/handoff/ipc-transport-and-spoof.md");
       assert!(s.contains("customProtocolIpcFailed = true"), "custom-protocol→postMessage fallback latch is gone");
       assert!(s.contains("window.ipc.postMessage("), "postMessage fallback no longer targets window.ipc");
   }
   ```
3. *(Optional, strongest)* a Node-based behavioral test: substitute the `__TEMPLATE_*__` placeholders in `ipc-protocol.js`, eval it in a sandbox whose `fetch` rejects, and assert the message arrives at a stubbed `window.ipc.postMessage`. This actually *executes* the negotiation instead of grepping it, at the cost of a small JS test harness. Worth it only if the repo grows a JS test setup anyway (note: `inject.js` already exposes `window.__ytmYagamiInternals` for exactly this style of testing).

If the hack is retained after Step 0, add a JS unit test beside it: patched fetch rejects `'ipc://x'`, forwards `new Request('ipc://x')` and ordinary URLs untouched — pinning the guard semantics that tier-2's "string URL" assert keeps honest from the Tauri side.

### (iii) Splitting `chrome_spoof.js`

Two files, two reasons to change:
- `src/ua_spoof.js` — `window.chrome`, `navigator.userAgentData`, `navigator.plugins`, `navigator.webdriver` (+ version placeholders per Q4). Changes when Google's checks or Chrome's release train move.
- `src/ipc_transport.js` — the fetch guard, **only if Step 0 proves it necessary**; header comment stating: what it does, that Tauri's own fallback makes it mechanically redundant, the OAuth-telemetry rationale, commit `81ef66e`, and a pointer to this document. Changes when Tauri's transport changes (and the Q5-ii test is what notices that).

Wiring in `lib.rs`: two `.initialization_script(...)` calls (they run in order); the `on_page_load` re-eval (`lib.rs:132-134`) needs only `ua_spoof.js` — an init script already ran on every navigation, the re-eval exists for the allowed-hosts belt-and-braces, and the IPC guard has no business on non-player pages (nothing invokes there). Keep separate `window.__ytm_*_applied` guards per file.

## Summary for the impatient

1. In Tauri 2.10.3 the monkeypatch is **mechanically redundant**: invoke tries `fetch(ipc://...)` and on rejection automatically and losslessly falls back to `window.ipc.postMessage` (`scripts/ipc-protocol.js:22-86`) — the exact path the hack forces. On a remote https origin the fetch is expected to reject on its own (WKWebView blocks non-CORS custom-scheme fetches; INFERRED, one runtime experiment to confirm).
2. The supported remote-IPC mechanism (capability `remote` field) is already in place — and, surprisingly, this app's own commands bypass the ACL entirely because it defines no app ACL manifest (`tauri-2.10.3/src/webview/mod.rs:1801-1805`).
3. The hack's real (unproven) value is suppressing a doomed `ipc://` fetch that may feed Google CSP-violation telemetry during OAuth. Test that once; delete the hack if OAuth survives.
4. Whatever happens: split the file, add the two-sided IPC self-test so transport death is loud, and add the crate-script contract test so a Tauri bump fails CI instead of shipping a silently dead IPC.



