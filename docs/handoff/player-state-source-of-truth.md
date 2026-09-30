# Player state: replacing the DOM scrape with a real source of truth

**Status:** design investigation / handoff. No code in `src/` or `src-tauri/` has been changed by this document.

> ## ⚠️ Read this first — parts of this document are already stale
>
> This was written against the tree as of commit `2bfcaf4`, while `src/inject.js`,
> `tests/inject.test.js` and the Rust sources were being edited concurrently. Its
> line-number citations and its "current design" descriptions refer to code that
> has since changed. The **design recommendations remain valid**; the description
> of the starting point does not.
>
> **Already implemented — treat these as done, not proposed:**
> - `isPlaying` now derives from `!video.paused`, with the localized
>   `#play-pause-button` title kept only as a fallback when no `<video>` exists.
>   The locale bug this document opens with is fixed.
> - `extractTrackState(doc)` takes an optional document and no longer requires
>   `#play-pause-button`, so a missing button cannot null the whole extraction.
> - `src/inject.js` exposes `window.__ytmYagamiInternals = { extractTrackState,
>   extractIsPlaying, stop }`, and `tests/inject.test.js` now reads and executes
>   the real file instead of a duplicated copy. The duplicated-copy problem this
>   document's testing section describes is resolved.
> - The blind `.catch(function(){})` on the IPC invoke now logs via `console.error`.
> - `<video>` `play`/`pause`/`loadedmetadata` listeners are attached — but bound to
>   the element, not via capture on `document`, so unlike this document's proposal
>   they do **not** survive YTM swapping the video node.
>
> **Still open — this is the actual remaining value here:**
> - `navigator.mediaSession.metadata` as the primary metadata source (§2–§3).
> - `#movie_player.getVideoData()` as layer 2.
> - Capture-phase listeners on `document` replacing per-element listeners.
> - Eliminating the MutationObserver entirely; the 200 ms debounce and 5 s safety poll.
> - The in-app verification checklist in §5 — the highest-value part of this
>   document, and unaffected by any of the above.
>
> Any successor `inject.js` proposed here must preserve the
> `window.__ytmYagamiInternals` export surface (including a `stop()` that tears
> down whatever it installs) and keep `extractTrackState(doc)` /
> `extractIsPlaying(doc)` accepting an optional document, or it breaks the
> existing test suite.

**Question:** can `src/inject.js` stop inferring playback state from localized UI strings
(`title === 'Pause'`) and CSS-class selectors, and instead read a source of truth that is
locale-independent and resilient to Google reshuffling the player bar?

**Answer:** yes. Playback state should come from the `<video>` element (`video.paused` plus its
media events), and metadata should come from `navigator.mediaSession.metadata` — which YouTube
Music populates itself with structured, locale-free `{title, artist, album, artwork[]}` — with the
`#movie_player` public API as a second layer and the existing DOM scrape demoted to a last-resort
third layer. The MutationObserver can be eliminated entirely.

Throughout this doc, every claim is tagged:

- **[verified-from-source]** — read directly from this repo's code.
- **[spec-guaranteed]** — behavior mandated by the HTML/DOM specs, not YTM-specific.
- **[well-established, unverified here]** — widely relied-on behavior of YouTube/YTM or WebKit
  (userscripts, extensions, third-party clients depend on it) that I could **not** confirm in this
  app's WKWebView without running it. Section 5 gives the exact commands to confirm each one.

---

## 1. The current design and its confirmed defects

**[verified-from-source]** `src/inject.js` today:

- Extracts `{title, artist, art, isPlaying}` by scraping `yt-formatted-string.title.ytmusic-player-bar`,
  `yt-formatted-string.byline.ytmusic-player-bar`, `img.ytmusic-player-bar` (`src/inject.js:6-17`).
- Derives `isPlaying` from `playPauseButton.getAttribute('title') === 'Pause'` (`src/inject.js:18`).
  This compares against an **English, localized tooltip string**. On a German UI the attribute is
  "Pausieren", on Japanese "一時停止", etc., so `isPlaying` is permanently `false`. Downstream
  (`src-tauri/src/lib.rs:27-34`) `should_notify_track_change` requires `is_playing == true`, so
  **notifications never fire on non-English UIs**, and `src-tauri/src/media.rs:63-69` maps
  `is_playing == false` to `MediaPlayback::Paused`, so **OS Now Playing shows Paused forever**.
- Watches the whole `ytmusic-player-bar` subtree with a MutationObserver
  (`src/inject.js:51-58`, `{childList, subtree, attributes, characterData}`), which fires on every
  progress-bar tick and time-text update — many times per second — and coalesces with
  `requestAnimationFrame`. The rAF batching bounds the cost but the observer still wakes the JS
  engine every frame while a song plays.
- Every selector in the file is a Google-owned implementation detail. There is no auto-updater
  (**[verified-from-source]** — no updater plugin in `src-tauri`), so a player-bar redesign
  silently bricks every installed copy with no recovery path except a manual reinstall.

The Rust contract we must keep satisfying **[verified-from-source]**:

- `handle_track_changed` expects `payload: TrackState` = `{title: String, artist: String, art: String, isPlaying: bool}`
  (`src-tauri/src/lib.rs:13-20`, camelCase `isPlaying` via serde rename). All four fields are
  required; extra fields would be ignored by serde's default behavior but we won't send any.
- Rust dedupes notifications by title internally, but `media_controls.update()` runs on **every**
  invoke, so the JS side should keep deduping before invoking (as it does today with the
  JSON-stringify compare).
- `src-tauri/src/media.rs:31-34` already drives playback by calling
  `document.querySelector('video').play()/.pause()` from the souvlaki event handler — i.e. **the
  Rust side already trusts the `<video>` element as the control surface**. Reading state from the
  same element makes control and observation symmetric.

---

## 2. Candidate source of truth #1: the `<video>` element

YouTube Music plays everything — songs, videos, podcasts, and (on free accounts) ads — through a
single HTML5 media element, `video.html5-main-video`, inside the standard YouTube player at
`#movie_player`. **[well-established, unverified here]** — but note this repo's own
`media.rs` already assumes `document.querySelector('video')` finds it, and media keys reportedly
work, which is indirect evidence the selector is valid in this webview.

### What the element gives us

- **`video.paused`** — the authoritative playback boolean. **[spec-guaranteed]** semantics:
  `paused` is `true` iff the element is in the paused state (after `pause()` or before first
  `play()`). Crucially, **buffering does not set `paused`** — a stalled/`waiting` element still
  reports `paused === false`, which is exactly what we want (a buffering song should show
  "Playing" in Control Center, not flap to Paused).
- **Media events** — `play`, `pause`, `playing`, `ended`, `emptied`, `loadedmetadata`,
  `ratechange`, `waiting`. These are enum-like, locale-free, and fired by the engine, not by
  YTM's UI code.
- `duration` and `currentTime` — available for free if we ever want to report progress to
  souvlaki (see §9; requires a Rust-side change so it's out of scope here).

### The two classic gaps, and how to close them

1. **"Does YTM swap the `<video>` element between tracks?"** On youtube.com the element is
   long-lived and YTM swaps the MediaSource/`src` rather than the node
   **[well-established, unverified here]** — but we should not depend on that either way.
   The robust pattern: media events do not bubble, but **capture-phase listeners on `document`
   still see them** (the capture phase descends through ancestors to the target —
   **[spec-guaranteed]** DOM event dispatch). So:

   ```js
   document.addEventListener('play', handler, true);
   ```

   fires for *any* current or future media element in the page, with zero re-attachment logic.
   If YTM recreates the element tomorrow, nothing breaks. We then read
   `document.querySelector('video')` fresh at each send, never caching the node.

2. **The element carries no metadata.** `<video>` knows nothing about title/artist/artwork.
   It answers *"is something playing?"* perfectly and *"what is playing?"* not at all. That is
   why it is the playback-state layer, paired with a metadata layer below.

### Event-noise profile

Track transitions produce a short burst (`pause`/`emptied` → `loadedmetadata` → `play` →
`playing`, order not guaranteed across engines). A single trailing debounce (~200 ms) collapses
the burst into one IPC send with the settled state. Compare with today: dozens of observer
callbacks per second, forever. The event approach is quiescent while a song simply plays.

**Deliberately not listened to:** `timeupdate` (4 Hz forever, and we don't report progress) and
`waiting`/`stalled` (we don't want buffering to flap the state; `paused` already handles it).
`ratechange` is omitted because souvlaki's metadata (`src-tauri/src/media.rs:55-61`) has no rate
field — nothing downstream could consume it.

---

## 3. Candidate source of truth #2: `navigator.mediaSession` (metadata layer, primary)

### Why this is the right metadata source

YouTube Music populates the Media Session API itself — that is how Chrome's global media controls
and Android's notification show track info for music.youtube.com. When it does, the page sets:

```js
navigator.mediaSession.metadata = new MediaMetadata({
  title: 'Never Gonna Give You Up',
  artist: 'Rick Astley',
  album: 'Whenever You Need Somebody',
  artwork: [{ src: 'https://lh3.googleusercontent.com/...=w544-h544-...', sizes: '544x544', type: 'image/jpeg' }, ...]
});
```

**[well-established, unverified here]** for YTM-in-Chrome; whether YTM does this **inside this
app's WKWebView** is the single most important thing to verify (§5). Two conditions must hold:

1. WebKit must expose the API. Safari shipped Media Session (metadata + action handlers) in
   Safari 15 (2021), and WKWebView shares that WebKit **[well-established, unverified here]**.
   On any remotely modern macOS this should be present, but WKWebView feature flags do not always
   match Safari's, so it must be checked empirically.
2. YTM must feature-detect and populate it. YTM sees a Chrome UA (**[verified-from-source]**
   `CHROME_UA` in `lib.rs:5` plus `chrome_spoof.js`), so it will take its Chrome code path, which
   uses Media Session; but the spoof also means YTM may attempt Chrome-only calls. Sites almost
   universally guard with `if ('mediaSession' in navigator)`, so the plausible failure mode is
   "API absent → metadata never set → fall back", not a crash.

### What it gives us

Structured, locale-independent fields: `title`, `artist`, `album` (not currently used by Rust but
free to add later), and `artwork[]` — an array of `{src, sizes, type}` at multiple resolutions,
letting us **choose artwork resolution deliberately** instead of taking whatever `<img>` the
player bar happens to render (today's scrape gets the 40-60 px bar thumbnail
**[verified-from-source]** — the selectors target the player-bar image, and notifications get
that tiny image).

### Reading it, and observing writes

- `navigator.mediaSession.metadata` is a **readable attribute** — we can poll it any time.
- There is **no event** for metadata changes. Two complementary techniques:
  1. **Read-on-media-event:** every track change necessarily produces `<video>` events, so reading
     `mediaSession.metadata` inside the debounced handler from §2 catches nearly everything.
  2. **Patch the setter:** `inject.js` is eval'd in the page's main world
     (**[verified-from-source]** — `window.eval` in `lib.rs:94`, not an isolated content-script
     world), so we share YTM's JS realm and can wrap the prototype accessor:

     ```js
     var desc = Object.getOwnPropertyDescriptor(MediaSession.prototype, 'metadata');
     Object.defineProperty(MediaSession.prototype, 'metadata', {
       get: desc.get,
       set: function(v) { desc.set.call(this, v); scheduleSend(); },
       configurable: true, enumerable: desc.enumerable
     });
     ```

     WebIDL attributes are accessor properties with `configurable: true`
     **[spec-guaranteed]**, but the code must still guard (`if (desc && desc.set && desc.configurable)`)
     in case WebKit deviates. This fires at the exact moment YTM writes metadata — including
     writes that happen with no accompanying video event (e.g. metadata arriving late after
     `play`).
  3. **Ordering hazard:** inject.js runs on `PageLoadEvent::Finished` (`lib.rs:88`)
     **[verified-from-source]**, i.e. possibly *after* YTM already set metadata once. So the patch
     alone is insufficient — we must also read the current value at install time (the initial
     `sendIfChanged()` call does this).
  4. **Mutation hazard:** a site *can* mutate an existing `MediaMetadata`'s writable attributes
     instead of assigning a new object, which the setter patch would miss. The read-on-media-event
     path and the slow safety poll (§6) cover this. I consider assignment far more likely
     (it is the universal idiom) but the design doesn't bet on it.

### `mediaSession.playbackState` — deliberately not used

The spec allows pages to leave `playbackState` as `"none"` and let the UA infer state from the
media element; many sites never set it. Whether YTM sets it is unverified, and we don't need it:
`video.paused` is strictly more reliable. Ignore it.

---

## 4. Candidate source of truth #3: YTM internal objects (`#movie_player`, Polymer, store, ytcfg)

Ranked by stability:

### 4a. `#movie_player` public API — good; use as metadata fallback and state cross-check

The element `document.getElementById('movie_player')` is the standard YouTube HTML5 player. It
exposes the **same method surface as the publicly documented IFrame Player API**:
`getPlayerState()`, `getVideoData()`, `getPlayerResponse()`, `getDuration()`, `getCurrentTime()`,
`addEventListener('onStateChange', fn)`. Because these names are a published API contract that a
decade of third-party embeds depend on, they survive minification and have been stable for years
**[well-established, unverified here]** — though note Google only *documents* them for iframe
embeds; on music.youtube.com their presence is de facto, not contractual.

What it yields:

- `getPlayerState()` → **numeric enum**: `-1` unstarted, `0` ended, `1` playing, `2` paused,
  `3` buffering, `5` cued. Locale-independent playback state, useful as a fallback when no
  `<video>` node is findable, mapping `1` and `3` to playing.
- `getVideoData()` → `{video_id, title, author, ...}` — title and artist without any DOM parsing.
  `video_id` is also the ideal track-identity key (better than title strings), available if we
  later want smarter dedupe.
- `getPlayerResponse().videoDetails` → `title`, `author`, `videoId`, `lengthSeconds`,
  `thumbnail.thumbnails[]` (array of `{url, width, height}`) — artwork at selectable resolution.

Caveats: `#movie_player` may not exist until the first playback starts; `getPlayerResponse()` is a
large object and should be wrapped in try/catch; `videoDetails.author` for YTM is the artist
channel name, which for multi-artist tracks can differ slightly from the player-bar byline
(e.g. "Artist A" vs "Artist A, Artist B") **[well-established, unverified here]**.

### 4b. Polymer element properties (`ytmusic-player-bar.__data`, etc.) — rejected

Reachable from page scope, but property names are internal Polymer state, partially minified,
version-coupled, and undocumented. Strictly worse than 4a on every axis. Rejected.

### 4c. `ytmusic-app` store (`document.querySelector('ytmusic-app').store.getState()`) — rejected as primary

YTM ships a Redux-style store reachable from the app element; community userscripts use it
**[well-established, unverified here]**. It exposes queue and player state, but its shape is an
internal implementation detail with no compatibility promise, and it's exactly the kind of thing
a bundler rename destroys. Acceptable for exploratory debugging; wrong foundation for an app with
no auto-updater. Rejected.

### 4d. `window.ytcfg` — rejected

Configuration (API keys, experiment flags, locale), not playback state. Irrelevant here.

---

## 5. What I could NOT verify, and exactly how to verify it

I could not run the app or open a browser for this investigation. The following must be checked
empirically before merging. Run `cargo tauri dev`, open devtools against the webview
(`window.open_devtools()` from Rust, or right-click → Inspect if enabled), play a track, and
evaluate in the console:

| # | Claim to verify | Console check | Expected |
|---|---|---|---|
| 1 | WKWebView exposes Media Session | `'mediaSession' in navigator` | `true` |
| 2 | YTM populates it here | `navigator.mediaSession.metadata` (while a track plays) | `MediaMetadata {title, artist, album, artwork: [...]}` |
| 3 | Artwork array is populated | `navigator.mediaSession.metadata.artwork` | ≥1 entry with `src` and `sizes` |
| 4 | Metadata setter is patchable | `Object.getOwnPropertyDescriptor(MediaSession.prototype, 'metadata')` | `{get, set, configurable: true}` |
| 5 | Capture listeners see media events | `document.addEventListener('play', () => console.log('PLAY'), true)` then toggle playback | logs on each play |
| 6 | `#movie_player` API exists on YTM | `document.getElementById('movie_player').getPlayerState()` / `.getVideoData()` | `1` while playing; `{title, author, video_id}` |
| 7 | `video.paused` tracks the UI button | `document.querySelector('video').paused` after clicking pause/play | `true`/`false` matching UI |
| 8 | The whole point: locale independence | Switch YTM UI language (Settings → Language, or account language) to German/Japanese; play a track | notification fires; Control Center shows Playing |
| 9 | Ad behavior (free account only) | During an ad: `navigator.mediaSession.metadata` and `document.getElementById('movie_player').classList.contains('ad-showing')` | note whether metadata shows the ad; whether `ad-showing` is set |

If #1 or #2 fails, the design still works: layer 2 (`#movie_player`) or layer 3 (DOM scrape)
supplies metadata, and playback state never depended on mediaSession at all. If #5 fails
(it should not — it's spec behavior), fall back to attaching listeners directly to the video
element with a re-attach poll.

---

## 6. Recommended architecture

**Playback state (single source):** `document.querySelector('video')`, read fresh at send time —
`isPlaying = !video.paused && !video.ended`. Fallback if no video node: `#movie_player.getPlayerState() ∈ {1, 3}`.
Never derived from UI strings or CSS classes.

**Metadata (layered):**

1. `navigator.mediaSession.metadata` — structured, locale-free, artwork at chosen resolution.
2. `#movie_player.getVideoData()` + `getPlayerResponse().videoDetails.thumbnail` — locale-free,
   decade-stable method names.
3. The existing player-bar DOM scrape — kept verbatim as a last resort, minus any use for
   playback state. It is now only reachable when both structured sources are absent, and its
   failure mode is "no metadata" rather than "wrong playback state forever".

**Triggers (replacing the MutationObserver entirely):**

- Capture-phase `document` listeners for `play`, `pause`, `playing`, `ended`, `emptied`,
  `loadedmetadata` — survive video-element replacement, quiescent during steady playback.
- The `MediaSession.prototype.metadata` setter patch — catches metadata writes that arrive
  without a video event.
- A **5-second safety poll** calling the same `sendIfChanged()` — catches in-place metadata
  mutation, missed events, and the "already playing when injected" case. This is the only
  standing timer, versus today's per-frame observer wakeups. (Worst case it delays a missed
  track-change notification by 5 s; in practice the event paths fire first.)
- All triggers funnel through a 200 ms trailing debounce so a track-change burst becomes one IPC
  send, then through the existing JSON-stringify dedupe so unchanged state sends nothing.

**Rust contract:** unchanged. Same command name, same `{ payload: {title, artist, art, isPlaying} }`
shape, same `__TAURI_INTERNALS__.invoke` transport, same `__ytm_injected` re-entry guard
(inject.js is eval'd on every youtube.com page-load-finished, `lib.rs:93-95`). No capability
changes needed (`capabilities/default.json` already covers the remote URLs and `core:default`).

---

## 7. Proposed replacement `src/inject.js`

Same style as today: vanilla JS, non-module IIFE, eval'd as a raw script. **Do not apply this
while the other agent is editing `src/`** — it is the handoff artifact, not a live change.

```js
(function() {
  if (window.__ytm_injected) return;
  window.__ytm_injected = true;

  // Metadata sources, in order of preference:
  //   1. navigator.mediaSession.metadata — structured and locale-free,
  //      written by YouTube Music itself (title/artist/artwork).
  //   2. #movie_player public API — same method surface as the documented
  //      IFrame player API; stable for years.
  //   3. Player-bar DOM scrape — last resort only.
  // Playback state always comes from the <video> element (video.paused),
  // never from localized UI strings or CSS classes.

  function pickLargestArtwork(artwork) {
    if (!artwork || !artwork.length) return '';
    var best = artwork[0];
    var bestArea = 0;
    for (var i = 0; i < artwork.length; i++) {
      var m = /^(\d+)x(\d+)/.exec(artwork[i].sizes || '');
      var area = m ? parseInt(m[1], 10) * parseInt(m[2], 10) : 0;
      if (area >= bestArea) {
        bestArea = area;
        best = artwork[i];
      }
    }
    return best.src || '';
  }

  function metadataFromMediaSession() {
    var md = navigator.mediaSession && navigator.mediaSession.metadata;
    if (!md || !md.title) return null;
    return {
      title: md.title,
      artist: md.artist || '',
      art: pickLargestArtwork(md.artwork),
    };
  }

  function metadataFromMoviePlayer() {
    var player = document.getElementById('movie_player');
    if (!player || typeof player.getVideoData !== 'function') return null;
    var data;
    try {
      data = player.getVideoData();
    } catch (e) {
      return null;
    }
    if (!data || !data.title) return null;

    var art = '';
    if (typeof player.getPlayerResponse === 'function') {
      try {
        var response = player.getPlayerResponse();
        var thumbs = response && response.videoDetails
          && response.videoDetails.thumbnail
          && response.videoDetails.thumbnail.thumbnails;
        if (thumbs && thumbs.length) art = thumbs[thumbs.length - 1].url;
      } catch (e) { /* keep art empty */ }
    }
    return { title: data.title, artist: data.author || '', art: art };
  }

  function metadataFromDom() {
    var titleEl = document.querySelector('yt-formatted-string.title.ytmusic-player-bar');
    if (!titleEl || !titleEl.textContent) return null;

    var artist = '';
    var bylineEl = document.querySelector('yt-formatted-string.byline.ytmusic-player-bar');
    if (bylineEl) {
      var artistLink = bylineEl.querySelector('a');
      artist = artistLink ? artistLink.textContent : bylineEl.textContent.split(' • ')[0];
    }
    var thumbnailEl = document.querySelector('img.ytmusic-player-bar')
      || document.querySelector('.ytmusic-player-bar img')
      || document.querySelector('#song-image img');
    return {
      title: titleEl.textContent,
      artist: artist,
      art: thumbnailEl ? thumbnailEl.src : '',
    };
  }

  function isPlayingNow() {
    // video.paused is the engine's own state: buffering stays "playing",
    // and it is identical in every UI language.
    var video = document.querySelector('video');
    if (video) return !video.paused && !video.ended;

    var player = document.getElementById('movie_player');
    if (player && typeof player.getPlayerState === 'function') {
      try {
        var s = player.getPlayerState(); // 1 = playing, 3 = buffering
        return s === 1 || s === 3;
      } catch (e) { /* fall through */ }
    }
    return false;
  }

  function readTrackState() {
    var meta = metadataFromMediaSession() || metadataFromMoviePlayer() || metadataFromDom();
    if (!meta) return null;
    return {
      title: meta.title,
      artist: meta.artist,
      art: meta.art,
      isPlaying: isPlayingNow(),
    };
  }

  var lastStateJson = null;

  function sendIfChanged() {
    var state = readTrackState();
    if (!state) return;

    var json = JSON.stringify(state);
    if (json === lastStateJson) return;
    lastStateJson = json;

    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      window.__TAURI_INTERNALS__.invoke('handle_track_changed', { payload: state })
        .catch(function() {});
    }
  }

  // A track change arrives as a burst (pause/emptied → loadedmetadata →
  // metadata write → play); coalesce the burst into one send after it settles.
  var debounceId = null;
  function scheduleSend() {
    if (debounceId !== null) clearTimeout(debounceId);
    debounceId = setTimeout(function() {
      debounceId = null;
      sendIfChanged();
    }, 200);
  }

  // Playback state trigger: media events don't bubble, but capture-phase
  // listeners on document still see them — this survives YouTube Music
  // replacing the <video> element entirely.
  var MEDIA_EVENTS = ['play', 'pause', 'playing', 'ended', 'emptied', 'loadedmetadata'];
  for (var i = 0; i < MEDIA_EVENTS.length; i++) {
    document.addEventListener(MEDIA_EVENTS[i], scheduleSend, true);
  }

  // Metadata trigger: fire the moment YouTube Music assigns
  // navigator.mediaSession.metadata (we share the page's JS realm, so
  // patching the prototype accessor intercepts its writes).
  if (window.MediaSession && window.MediaSession.prototype) {
    var desc = Object.getOwnPropertyDescriptor(window.MediaSession.prototype, 'metadata');
    if (desc && desc.set && desc.configurable) {
      Object.defineProperty(window.MediaSession.prototype, 'metadata', {
        configurable: true,
        enumerable: desc.enumerable,
        get: desc.get,
        set: function(value) {
          desc.set.call(this, value);
          scheduleSend();
        },
      });
    }
  }

  // Safety net: a slow poll catches anything the triggers miss (metadata
  // mutated in place, events missed, track already playing at injection).
  // This is the only standing timer — no MutationObserver.
  setInterval(sendIfChanged, 5000);

  sendIfChanged();
})();
```

Notes on the code:

- The payload shape, command name, invoke transport, dedupe strategy, and re-entry guard are all
  byte-compatible with what Rust expects today; only the *sources* and *triggers* changed.
- `metadataFromDom()` is today's extractor with one change: the play/pause button is no longer
  consulted at all, so a missing/renamed `#play-pause-button` can no longer null out the whole
  extraction (today it does — `src/inject.js:9` returns null if the button is missing).
- The setter patch calls the original setter *first*, so if our handler ever throws, YTM's own
  Media Session behavior is unaffected.
- If the video element genuinely can't be found *and* `#movie_player` is absent, `isPlayingNow()`
  returns `false` — a metadata-only send with `isPlaying: false` updates souvlaki metadata but
  (correctly) doesn't notify.

---

## 8. Testing strategy

### 8a. Unit tests (vitest + jsdom, replacing `tests/inject.test.js`)

The current test file **duplicates** the extraction function instead of testing the shipped code
— it has already drifted (the test requires `img#thumbnail...` and returns null without it; the
real file has three fallback selectors and no such requirement) **[verified-from-source]**.
Because inject.js is a raw IIFE, the honest fix is to **eval the real file in jsdom**:

```js
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../src/inject.js', import.meta.url), 'utf8');

function boot({ mediaSession, tauri } = {}) {
  window.__ytm_injected = undefined;
  if (mediaSession !== undefined) {
    // jsdom has no MediaSession; install a minimal stand-in BEFORE eval.
    class MediaSession { get metadata() { return this._m; } set metadata(v) { this._m = v; } }
    window.MediaSession = MediaSession;
    Object.defineProperty(navigator, 'mediaSession', { value: new MediaSession(), configurable: true });
    navigator.mediaSession.metadata = mediaSession;
  }
  window.__TAURI_INTERNALS__ = tauri ?? { invoke: vi.fn().mockResolvedValue(undefined) };
  vi.useFakeTimers();          // owns the debounce, the 5 s poll, and cleanup
  window.eval(source);
}
```

Then cases along these axes (drive time with `vi.advanceTimersByTime`):

1. **Source priority:** with mediaSession metadata AND player-bar DOM present, the invoke payload
   uses the mediaSession values.
2. **Artwork selection:** artwork array `[{sizes:'96x96'},{sizes:'544x544'},{sizes:'256x256'}]`
   → payload `art` is the 544px `src`; missing/garbled `sizes` doesn't throw.
3. **movie_player fallback:** no mediaSession stand-in at all → a fake
   `<div id="movie_player">` with stubbed `getVideoData`/`getPlayerResponse`/`getPlayerState`
   supplies title/artist/art (jsdom lets you attach methods to the element object directly).
4. **DOM fallback:** neither structured source → today's fixtures (reuse the existing
   `document.body.innerHTML` blocks) still extract title/artist/art — *without* any
   `#play-pause-button` in the fixture.
5. **Locale independence (the regression that motivated all this):** fixture with
   `title="Pausieren"` on the button, a fake `<video>` with `paused` overridden to `false` via
   `Object.defineProperty` (jsdom's default is `true` and `play()` is unimplemented — always
   override the property, then dispatch `new Event('play')`) → payload has `isPlaying: true`.
6. **Buffering:** `paused=false` + dispatching `waiting` → still `isPlaying: true`, and no extra
   send (event not listened to).
7. **Capture + element swap:** remove the video element, insert a new one, dispatch `play` on it
   → handler still fires (validates the capture-listener strategy in jsdom's spec-conformant
   dispatch).
8. **Setter patch:** assign `navigator.mediaSession.metadata = new-metadata` after boot → exactly
   one invoke after the debounce window, with the new title.
9. **Debounce + dedupe:** dispatch `pause`,`emptied`,`loadedmetadata`,`play` within 50 ms → one
   invoke; repeat identical state → zero further invokes; the 5 s poll alone never re-sends
   unchanged state.
10. **No-Tauri guard:** delete `window.__TAURI_INTERNALS__` → no throw.

### 8b. Rust tests

No changes required — `should_notify_track_change` and serialization tests in `lib.rs` already
cover the contract, which is unchanged.

### 8c. Manual verification (must happen before release)

Run the checklist in §5 verbatim, on: (a) English UI, (b) at least one non-English UI, (c) if
available, a free (ad-supported) account for row 9. Additionally soak-test: play through ≥3
automatic track transitions and confirm exactly one notification each; pause/resume from both the
YTM UI and the OS media keys and confirm Control Center state follows (this also exercises the
existing `media.rs` eval path against the same `<video>` element).

---

## 9. Risk assessment (honest)

| Approach | Failure modes | Blast radius | Mitigation |
|---|---|---|---|
| `<video>` events + `video.paused` | WKWebView could throttle timers/events in occluded windows (events themselves are not throttled, timers can be — affects only the 5 s poll cadence). If YTM ever moved playback into an iframe from another origin, capture listeners on the top document would go blind — no sign of that today, and media keys via `querySelector('video')` would break identically, so we'd notice. | Playback state wrong → Now Playing stuck, notifications suppressed | `getPlayerState()` fallback; §5 row 5/7 checks; the safety poll re-reads truth every 5 s |
| Media Session metadata | (1) WKWebView doesn't expose the API or YTM doesn't populate it there — **the key unverified assumption**; (2) YTM mutates the MediaMetadata in place, bypassing the setter patch; (3) during ads (free tier) metadata may briefly show ad content → one spurious notification per ad | Metadata missing → automatic drop to layer 2/3; ads → cosmetic noise, English-market only today has the same issue via the DOM scrape | Layered fallback is not optional, it's the design; safety poll + read-on-media-event cover in-place mutation; if §5 row 9 shows `ad-showing` on `#movie_player`, gate sends on `!player.classList.contains('ad-showing')` as a follow-up |
| `#movie_player` API | Undocumented *on this property* (the contract is for iframe embeds); element absent before first playback; `author` may not match the byline for multi-artist tracks; `getPlayerResponse()` is big — call lazily (we do, and only on the debounced path) | Fallback metadata slightly off or absent | It's layer 2 of 3; try/catch everywhere; never called at high frequency |
| DOM scrape (retained fallback) | Everything that's wrong with it today — that's why it's last | Only reached when both structured layers fail; can no longer poison `isPlaying` | Kept selector-for-selector so it rots no faster than today |
| Setter patch on `MediaSession.prototype` | Another script (or a future YTM) also patching it; descriptor not configurable in some WebKit build | Loses the "instant metadata" trigger only | Guarded install; read-on-media-event and the poll still deliver metadata, just up to 5 s later |
| Removing the MutationObserver | If every event path failed simultaneously (no mediaSession, no video events, no movie_player), track changes would surface only via the 5 s poll reading the DOM scrape | Notifications delayed ≤5 s in a scenario that today would mean total breakage anyway | Acceptable; do not re-add the observer |

**Residual honest gap:** the whole recommendation leans on §5 rows 1-2. If WKWebView turns out
not to expose Media Session, the system degrades to `#movie_player` + DOM-scrape metadata with
`<video>`-based playback state — which *still fixes both confirmed bugs* (locale-dependent
`isPlaying` and the per-frame observer), because playback state never touches the DOM in any
layer. That is the core of the design: the two things that were actively wrong are fixed by
spec-guaranteed mechanisms; only the metadata *upgrade* depends on unverified platform behavior,
and it fails soft.

### Future enhancements (out of scope, noted for the Rust owner)

- `video.duration`/`currentTime` → souvlaki `MediaPlayback::Playing { progress }` and
  `MediaMetadata.duration` for a real scrubber in Control Center (needs `TrackState` fields).
- `album` from mediaSession metadata → souvlaki `album` (currently `None`, `media.rs:58`).
- `getVideoData().video_id` as the dedupe key in Rust instead of title strings (two tracks with
  identical titles currently suppress the second notification, `lib.rs:28`).
- Investigate whether WKWebView itself publishes a Now Playing entry for the page's media session,
  which could duplicate souvlaki's entry in Control Center — worth checking while doing §5, though
  today's app already has the same exposure.
