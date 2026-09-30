# Threading model audit — YTM Yagami

Audit of the concurrency model around `handle_track_changed`, souvlaki media
controls, and the macOS notification pipeline. Claims verified against source
are cited as `file:line` (crate sources read from the local cargo registry:
tauri 2.10.3, tauri-runtime-wry 2.10.1, wry 0.54.4, tao 0.34.8,
souvlaki 0.8.3). Reasoned inference and Apple-documentation claims are marked
as such; where only a run under a main-thread checker or TSan can settle a
point, that is said explicitly.

Sections:

1. [Q1 — What thread does a sync `#[tauri::command]` run on, and is souvlaki safe there?](#q1)
2. [Q2 — `setup()` thread, MPRemoteCommandCenter callback thread, and `window.eval` safety](#q2)
3. [Q3 — Is `MediaControlsWrapper: Send + Sync` sound?](#q3)
4. [Q4 — Notification ordering under rapid track skips, and the corrected pipeline](#q4)

<a name="q1"></a>
## Q1 — Sync `#[tauri::command]` runs on the **main thread** on macOS; souvlaki is safe as invoked here

### The dispatch chain, verified from source

A synchronous (non-`async`) command is executed inline on whatever thread
delivers the IPC message. There is no thread pool and no spawn anywhere on the
path:

1. **Command wrapper (macro).** For a non-async command, `#[tauri::command]`
   generates `body_blocking`: the user function is called inline —
   `let result = $path(...); kind.block(result, resolver);` — on the caller's
   thread. Only `async` commands go through `respond_async_serialized`, which
   spawns onto the tokio runtime.
   — `tauri-macros-2.5.5/src/command/wrapper.rs:359-390` (blocking),
   `:333-350` (async).
2. **Invoke plumbing.** The generated handler is called synchronously:
   `Webview::on_message` (`tauri-2.10.3/src/webview/mod.rs:1724`) →
   `manager.run_invoke_handler(invoke)` (`src/webview/mod.rs:1888`) →
   `(self.webview.invoke_handler)(invoke)` (`src/manager/mod.rs:469-471`).
   No spawn, no channel.
3. **IPC entry points.** `on_message` is invoked inline from both IPC
   transports: the `ipc://` custom-protocol handler
   (`tauri-2.10.3/src/ipc/protocol.rs:75`, registered at
   `src/manager/webview.rs:274-279`) and the `postMessage` script-message
   handler (`src/ipc/protocol.rs:311`). Again inline, no thread hop.
4. **Which thread delivers those on macOS.** Both wry entry points are
   main-thread:
   - The script-message path: `WryWebViewDelegate` is declared
     `#[thread_kind = MainThreadOnly]` and its
     `userContentController:didReceiveScriptMessage:` calls the tauri handler
     inline — `wry-0.54.4/src/wkwebview/class/wry_web_view_delegate.rs:26-40`.
   - The custom-protocol path: `start_task`
     (`wry-0.54.4/src/wkwebview/class/url_scheme_handler.rs:57`) calls the
     registered protocol closure inline (`url_scheme_handler.rs:322`). WebKit
     invokes `WKURLSchemeHandler` methods on the main thread (Apple
     documentation claim, not verifiable from this source tree — but the wry
     class is only ever registered from main-thread webview construction, and
     WebKit's contract is well established).

**Conclusion (verified): `handle_track_changed` runs on the macOS main
thread — the tao event-loop/UI thread.** Everything it does synchronously —
both mutex locks, souvlaki `set_metadata`/`set_playback`, the JSON
deserialization of the payload — happens on the UI thread. The analysis below
does not invert.

### Is souvlaki safe as invoked here?

souvlaki's macOS backend does **not** dispatch to the main queue internally.
`set_playback` and `set_metadata` are raw `msg_send!` calls to
`[MPNowPlayingInfoCenter defaultCenter]` executed on the **calling** thread:

- `set_playback_status` — `souvlaki-0.8.3/src/platform/macos/mod.rs:94-111`
  (`setPlaybackState:` via `msg_send!`).
- `set_playback_metadata` — `mod.rs:115-141` (`setNowPlayingInfo:` via
  `msg_send!`).

The one internal thread hop souvlaki does make goes the *other* way: when
`cover_url` is set (which `media.rs:90-101` always does for non-empty art),
`set_playback_metadata` dispatches the artwork fetch to a **global background
GCD queue** (`mod.rs:134-139`), where `load_image_from_url` does a fully
synchronous `[NSImage initWithContentsOfURL:]` network fetch (`mod.rs:322-329`)
and then calls `setNowPlayingInfo:` *from that background queue*
(`mod.rs:151-159`).

Given Q1's answer, the app's own calls land on the main thread, which is the
safest possible thread for MediaPlayer.framework. Apple's header documentation
for `MPNowPlayingInfoCenter.nowPlayingInfo` additionally states now-playing
info may be set from any thread (documentation claim), so even souvlaki's
background-queue artwork write is within contract for *that* property.
`setPlaybackState:` (macOS-only) has no documented thread contract; here it is
only ever called from the main thread, so nothing is violated.

**Failure mode had this been off-main:** for `nowPlayingInfo` specifically,
none crisply defined — it is documented thread-tolerant. The realistic hazards
are (a) `setPlaybackState:` off-main being outside any documented contract
(symptom, if any: Now Playing widget showing stale play/pause state or console
`Main Thread Checker` diagnostics under Xcode), and (b) souvlaki's own
check-then-act artwork race, below. Only a run under the Main Thread Checker
or TSan can positively demonstrate either.

### Residual issues on this path (reasoned inference, source-cited)

1. **Main-thread blocking, not a safety bug.** The sync command executes JSON
   deserialization + two mutex locks + four ObjC calls on the UI thread on
   every player state tick. Cheap today; but any future blocking work added to
   `update()` stalls rendering and event handling. The album-art download was
   correctly pushed off-thread (`lib.rs:93`).
2. **Duplicate art download.** Every genuine track change downloads the same
   art twice: once by souvlaki's internal background fetch (for the Now
   Playing widget, `mod.rs:134-139`) and once by
   `macos_notifications::download_album_art` (for the notification,
   `macos_notifications.rs:131`). Harmless but wasteful; if art URLs are ever
   made larger this doubles.
3. **souvlaki's internal artwork race (upstream bug, worth knowing).**
   `set_playback_metadata` bumps `GLOBAL_METADATA_COUNTER` and the background
   artwork task only applies its artwork if the counter is still current
   (`mod.rs:143-149`). That check-then-act is not atomic with the
   `setNowPlayingInfo:` write, and `set_playback_artwork`/
   `set_playback_progress` both do non-atomic read-merge-write of the
   now-playing dictionary (`mod.rs:151-169`) from different threads. Under
   rapid skips a stale cover can land on the Now Playing widget, and a
   concurrent `set_playback` progress write can be lost. This is inside
   souvlaki; the app cannot fix it without pinning artwork updates itself.

<a name="q2"></a>
## Q2 — `setup()` is main-thread; MPRemoteCommandCenter blocks fire on the main thread; `window.eval` is thread-safe from anywhere

**Which thread is `setup()` on?** The setup hook runs inside the event-loop
callback on the `Ready` event: `RuntimeRunEvent::Ready => setup(&mut self)`
(`tauri-2.10.3/src/app.rs:1296-1300`), i.e. on the thread that called
`App::run`. On macOS that is necessarily the main thread — tao's event loop
must run there, and Tauri does not even expose the `any_thread` escape hatch on
macOS (`src/app.rs:1498-1502`). So `MediaControlsWrapper::init`, and therefore
souvlaki's `attach` → `addTargetWithHandler:` registrations
(`souvlaki-0.8.3/src/platform/macos/mod.rs:172-256`), all execute on the main
thread.

**Which thread invokes the command-center blocks?** MPRemoteCommandCenter
delivers remote-command events on the main thread (Apple documentation /
long-standing framework behavior — not verifiable from this source tree; a
Main Thread Checker run would confirm). souvlaki wraps the app's closure in a
`ConcreteBlock` and hands it straight to `addTargetWithHandler:`
(`mod.rs:175-186` and siblings), so the closure in `media.rs:58-74` runs on
the main thread whenever the user presses a media key or uses the Now Playing
widget.

**Is `get_webview_window` + `window.eval` safe from that thread?** Yes — from
any thread, and *particularly* from the main thread:

- `Webview::eval` forwards to the runtime dispatcher
  (`tauri-2.10.3/src/webview/mod.rs:1896-1901`).
- `WryWebviewDispatcher::eval_script` calls `send_user_message` with
  `WebviewMessage::EvaluateScript` (`tauri-runtime-wry-2.10.1/src/lib.rs:1774-1783`).
- `send_user_message` (`tauri-runtime-wry-2.10.1/src/lib.rs:234-254`) checks
  the current thread: **on the main thread it handles the message inline**
  (calling WKWebView `evaluateJavaScript` synchronously, which is required to
  be main-thread anyway); from any other thread it posts through the tao
  `EventLoopProxy`, which is `Send` and explicitly designed for cross-thread
  wakeups.

So the media-key path (main thread → inline eval) is correct, and even if some
other souvlaki backend or a future refactor invoked the handler off-main, the
eval would be marshalled through the event-loop proxy rather than touching
WKWebView from the wrong thread. One caveat, verified from the same function:
with the `tracing` feature enabled, `eval_script` uses the blocking `getter!`
variant (`lib.rs:1759-1771`) which waits on a channel; called from the main
thread it still completes (inline handling sends before the wait), but it is
the kind of code where a deadlock would appear if the inline-on-main-thread
branch of `send_user_message` ever changed. The default build does not enable
`tracing`.

**One genuine ordering caveat:** `init` is called at the end of `setup()`
(`lib.rs:156-157`) while `attach` registrations happen before
`*self.controls.lock() = Some(controls)` (`media.rs:58-81`). A media-key event
arriving in that window would find the window already registered, so the eval
path works; there is no observable race. Had `attach` been able to fire before
`app_handle.get_webview_window("main")` could resolve, the handler's
`if let Some(window)` guard (`media.rs:71`) degrades to a silent no-op —
correct behavior.

<a name="q3"></a>
## Q3 — `manage(AppState)` is sound: souvlaki's macOS `MediaControls` is a **unit struct**, not a pointer-holder

`tauri::Manager::manage` requires `Send + Sync + 'static`. The premise that
souvlaki's macOS `MediaControls` holds raw ObjC `id` pointers is **false for
souvlaki 0.8.3**: the macOS backend's `MediaControls` is a zero-sized unit
struct — `pub struct MediaControls;`
(`souvlaki-0.8.3/src/platform/macos/mod.rs:40`). It stores nothing; every
operation is a fresh `msg_send!` to the process-global singletons
`[MPNowPlayingInfoCenter defaultCenter]` and
`[MPRemoteCommandCenter sharedCommandCenter]`. There is no
`unsafe impl Send`/`Sync` anywhere in the macOS backend; the auto-derived
`Send + Sync` for a fieldless struct is what makes
`Arc<Mutex<Option<MediaControls>>>` (and hence `MediaControlsWrapper` and
`AppState`) satisfy `manage`'s bounds. **No unsoundness here.**

Two honest qualifications:

1. **The type-level story understates the aliasing.** Because the real state
   is ObjC-global, the `Mutex` around `Option<MediaControls>` serializes the
   *app's* calls but cannot serialize souvlaki's own background-queue artwork
   writes (Q1, residual issue 3) against them. The mutex provides mutual
   exclusion of a proxy object, not of the underlying `nowPlayingInfo`
   dictionary. That is a souvlaki design wart, not a Rust soundness hole —
   all the racy accesses are behind `unsafe` in souvlaki, and the property
   itself is documented thread-tolerant.
2. **This is platform-conditional.** On Windows, souvlaki's `MediaControls`
   *does* hold COM interface pointers, and on Linux a DBus service handle.
   Those backends carry their own `Send` stories; this audit verified only
   the macOS backend, which is what ships. If the Windows build is ever
   exercised, the `Arc<Mutex<...>>`-makes-it-`Send` question must be re-asked
   against `souvlaki/src/platform/windows`.
3. **`attach`'s closure requires `Send + 'static`** (`mod.rs:49-52`), and the
   app passes an `AppHandle` clone (`media.rs:57-58`), which is `Send + Sync`
   by Tauri's design. The `ConcreteBlock` copy leaks into
   MPRemoteCommandCenter and is never deallocated until `detach` — the app
   never detaches, which is fine for a process-lifetime singleton.

<a name="q4"></a>
## Q4 — Notification ordering under rapid skips: what breaks, and the serialized latest-wins pipeline that fixes it

### The current pipeline, restated precisely

Per genuine track change, `handle_track_changed` (main thread, per Q1) spawns
a detached `std::thread` (`lib.rs:93`) → blocking art download with 5 s
connect / 10 s total timeouts (`macos_notifications.rs:19-20,131`) → writes a
**unique per-notification** temp file (`macos_notifications.rs:156`, the
shared-path race is already fixed) → `ytm_notifications_show` →
`dispatch_async` onto the main queue (`macos_notifications.m:74`) →
`removeAllDeliveredNotifications` (`macos_notifications.m:78`) → schedule a
request with a **0.1 s time-interval trigger** (`macos_notifications.m:100-106`)
and a fresh unique identifier.

Two structural facts drive everything below:

- **Fetch completion order is arbitrary.** Five fast "next" presses spawn five
  threads racing five different art URLs. Enqueue order onto the main queue is
  fetch-completion order, not track order. The main queue then executes the
  blocks serially in that (wrong) order.
- **A 0.1 s trigger means every notification spends ~0.1 s *pending*, not
  *delivered*.** `removeAllDeliveredNotifications` does not touch pending
  requests — nothing in the current code ever cancels a scheduled-but-undelivered
  notification.

One useful fact in the current code's favor, a direct consequence of Q1: since
every `handle_track_changed` runs on the main thread, the `last_notified`
dedupe and the spawn order are strictly serialized in true track order. The
disorder is introduced entirely *after* the spawn.

### Concrete interleavings (tracks T1…T5 skipped rapidly; thread tᵢ fetches art for Tᵢ)

1. **Stale track wins.** T5's art is small/cached, T2's server is slow.
   t5 completes at +0.3 s → banner "T5" (correct, user is on T5). t2 completes
   at +6 s → its main-queue block clears the delivered T5 banner and posts
   "T2". End state, seconds after the user stopped skipping: **the only
   visible notification names a track the user skipped past**, and the correct
   one was actively removed to make room for it. This is the headline failure.
2. **Out-of-order barrage.** Fetches complete 3,1,4,2,5 within a second: up to
   five banners flash in scrambled order, each clearing all previous delivered
   ones. Even the "good" ending (5 last) shows the user a nonsense sequence.
3. **Stale and fresh banners stack (the trigger-window race).** t5's block
   runs: removeAllDelivered (clears old), schedules N5 — *pending* for 0.1 s.
   Within that window t2's block runs: `removeAllDeliveredNotifications`
   removes **nothing** (N5 is pending, not delivered), schedules N2. Both then
   deliver: **two banners, the stale T2 as the newest/topmost.** Whenever two
   fetches complete within ~0.1 s of each other — exactly the rapid-skip
   regime — the clear-previous intent silently fails.
4. **Unbounded concurrent work, no cancellation.** n presses = n detached
   threads, n sockets, n downloads, each up to 10 s. Nothing tells t2 that its
   track is obsolete; it runs to completion and then actively damages the UI
   (case 1). Threads are also unjoinable — at quit, in-flight fetches are
   killed wherever they happen to be (harmless today, but nothing guarantees
   it stays that way).
5. **Collateral of `removeAllDeliveredNotifications`:** it clears *every*
   delivered notification of the app, not just the previous track's — any
   future non-track notification would be swept away by the next song change.

### `UNNotificationAttachment` move semantics — confirmed, with implications

Apple's documentation for `UNNotificationAttachment` states that once the
system validates an attachment, the file is **moved** into the system-managed
attachment data store (documentation claim; not verifiable from this source
tree, but it is also what the in-repo comment at
`macos_notifications.rs:153-155` records, and matches observed behavior — the
temp file vanishes on `addNotificationRequest`). Implications:

- Per-notification unique paths are **mandatory**, not just tidy — a second
  notification referencing the same path finds it already gone. (Already
  fixed; noted for the record.)
- Successfully attached files need no cleanup by the app: the system owns them
  and deletes them when the notification is removed. The 60 s prune
  (`macos_notifications.rs:115-129`) only ever catches orphans from failed
  attach or a crash — it is correctly scoped.
- In the redesign below, a superseded worker deleting its own abandoned temp
  file can never delete a live notification's image: a live notification's
  file has already been moved out of the temp dir.

### The fix: one serialized worker, one-slot latest-wins mailbox, chunked-cancel fetch, fixed notification identifier

Design invariants:

- **One worker thread** owns the entire notification pipeline. At most **one
  fetch in flight** ever (the concurrency bound), and posts to ObjC happen in
  submission order by construction.
- **One-slot mailbox, latest wins.** `submit` overwrites the slot; a job the
  worker never picked up is dropped, not queued. Under five fast presses the
  worker typically fetches T1's art (already in flight), abandons it at the
  next chunk boundary, and fetches T5 — T2–T4 never touch the network.
- **Generation counter as cancellation token.** Each submit bumps it; the
  in-flight download compares its own generation at every chunk boundary and
  after the fetch, abandoning superseded work (blocking reqwest cannot be
  interrupted mid-`read`, so cancellation latency is one chunk read bounded by
  the existing 5 s/10 s timeouts — stated plainly).
- **Fixed notification identifier + per-identifier removal + nil trigger** on
  the ObjC side: cancel the previous track's notification whether pending or
  delivered (closing interleaving 3), deliver immediately (no pending window
  at all), and stop clobbering unrelated notifications (closing 5).

#### `src-tauri/src/macos_notifications.rs` — replace `show_track_notification` and `download_album_art` with:

```rust
use std::{
    io::Read,
    sync::{Arc, Condvar},
};

/// Cancellation granularity for an in-flight art download: the superseded
/// check runs once per chunk, so worst-case abandon latency is one chunk
/// read (further bounded by ART_CONNECT_TIMEOUT / ART_TOTAL_TIMEOUT).
const ART_CHUNK: usize = 16 * 1024;

/// One queued track notification. Only the newest submission matters.
struct NotifyJob {
    title: String,
    artist: String,
    art_url: String,
    generation: u64,
}

struct Mailbox {
    /// One-slot latest-wins queue: `submit` overwrites, the worker takes.
    slot: Mutex<Option<NotifyJob>>,
    available: Condvar,
    /// Monotonic id of the newest submitted job. A job whose generation no
    /// longer matches has been superseded and must abandon its work.
    generation: AtomicU64,
}

/// Serialized notification pipeline. Exactly one worker thread; at most one
/// art fetch in flight; posts reach Notification Center in submission order.
pub struct NotificationPipeline {
    mailbox: Arc<Mailbox>,
}

impl NotificationPipeline {
    pub fn new() -> Self {
        let mailbox = Arc::new(Mailbox {
            slot: Mutex::new(None),
            available: Condvar::new(),
            generation: AtomicU64::new(0),
        });
        let worker_mailbox = Arc::clone(&mailbox);
        std::thread::Builder::new()
            .name("ytm-notification-worker".into())
            .spawn(move || worker_loop(&worker_mailbox))
            .expect("failed to spawn notification worker");
        Self { mailbox }
    }

    /// Latest-wins hand-off. Replaces any job the worker has not yet picked
    /// up (the superseded job is dropped, not queued) and signals an
    /// in-flight fetch to abandon at its next chunk boundary.
    pub fn submit(&self, title: String, artist: String, art_url: String) {
        let generation = self.mailbox.generation.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut slot = self.mailbox.slot.lock().unwrap();
            *slot = Some(NotifyJob {
                title,
                artist,
                art_url,
                generation,
            });
        }
        self.mailbox.available.notify_one();
    }
}

fn worker_loop(mailbox: &Mailbox) {
    loop {
        let job = {
            let mut slot = mailbox.slot.lock().unwrap();
            loop {
                match slot.take() {
                    Some(job) => break job,
                    None => slot = mailbox.available.wait(slot).unwrap(),
                }
            }
        };

        let id = next_notification_id();

        // A failed art fetch must degrade to a text-only notification, never
        // suppress the notification entirely. A *superseded* fetch, by
        // contrast, skips the whole notification: a newer job is coming.
        let image_path = if job.art_url.trim().is_empty() {
            String::new()
        } else {
            match download_album_art(&job.art_url, &id, mailbox, job.generation) {
                Ok(Some(path)) => path.to_string_lossy().into_owned(),
                Ok(None) => continue, // superseded mid-download
                Err(e) => {
                    eprintln!("[YTM Yagami] album art unavailable, sending without it: {e}");
                    String::new()
                }
            }
        };

        // The fetch finished, but a newer track may have been submitted while
        // the file was being written. Posting it now would resurrect exactly
        // the stale-track-wins interleaving this worker exists to prevent.
        if mailbox.generation.load(Ordering::SeqCst) != job.generation {
            if !image_path.is_empty() {
                let _ = fs::remove_file(&image_path);
            }
            continue;
        }

        let id = to_cstring(&id);
        let title = to_cstring(&job.title);
        let artist = to_cstring(&job.artist);
        let image_path = to_cstring(&image_path);
        unsafe {
            ytm_notifications_show(
                id.as_ptr(),
                title.as_ptr(),
                artist.as_ptr(),
                image_path.as_ptr(),
            );
        }
    }
}

/// Download `url` to a unique temp file, abandoning at the next chunk
/// boundary if the job is superseded. `Ok(None)` means "superseded, nothing
/// written"; errors mean "send the notification without art".
fn download_album_art(
    url: &str,
    id: &str,
    mailbox: &Mailbox,
    my_generation: u64,
) -> Result<Option<PathBuf>, Box<dyn Error + Send + Sync>> {
    let superseded = || mailbox.generation.load(Ordering::SeqCst) != my_generation;

    let mut response = http_client().get(url).send()?.error_for_status()?;

    if let Some(len) = response.content_length() {
        if len > MAX_ART_BYTES {
            return Err(format!("album art too large: {len} bytes").into());
        }
    }

    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = [0u8; ART_CHUNK];
    loop {
        if superseded() {
            return Ok(None);
        }
        let n = response.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() as u64 > MAX_ART_BYTES {
            return Err(format!("album art too large: {} bytes", bytes.len()).into());
        }
    }

    let ext = infer::get(&bytes)
        .map(|kind| kind.extension())
        .unwrap_or("jpg");

    let dir = art_dir();
    fs::create_dir_all(&dir)?;
    prune_stale_art(&dir);

    // Unique per notification: UNNotificationAttachment moves the file into
    // the system attachment store, and two in-flight notifications sharing
    // one path race each other for it.
    let path = dir.join(format!("{id}.{ext}"));
    fs::write(&path, &bytes)?;
    Ok(Some(path))
}
```

(`std::io::Read` on `reqwest::blocking::Response` is the documented way to
stream a blocking response body; the existing `http_client()`, `art_dir()`,
`prune_stale_art()`, `next_notification_id()`, `to_cstring()` and the
constants are unchanged. The old free function `show_track_notification` is
subsumed by the worker body and deleted along with its `Result` plumbing —
`NotificationPipeline::submit` is the new entry point.)

#### `src-tauri/src/lib.rs` — wire the pipeline through `AppState`:

```rust
pub struct AppState {
    pub last_notified: Mutex<String>,
    pub media_controls: MediaControlsWrapper,
    #[cfg(target_os = "macos")]
    pub notifications: macos_notifications::NotificationPipeline,
}
```

```rust
        .manage(AppState {
            last_notified: Mutex::new(String::new()),
            media_controls: MediaControlsWrapper::new(),
            #[cfg(target_os = "macos")]
            notifications: macos_notifications::NotificationPipeline::new(),
        })
```

```rust
#[tauri::command(rename_all = "snake_case")]
fn handle_track_changed(payload: TrackState, state: tauri::State<'_, AppState>) {
    state.media_controls.update(&payload);

    let mut last_notified = state.last_notified.lock().unwrap();
    if should_notify_track_change(&payload, &mut last_notified) {
        // Drop the lock before the (potentially slow) notification hand-off.
        drop(last_notified);

        #[cfg(target_os = "macos")]
        state.notifications.submit(
            payload.title.clone(),
            payload.artist.clone(),
            payload.art.clone(),
        );
    }
}
```

`submit` only takes one short mutex + a condvar signal, so the main-thread
cost of the command drops (no `thread::spawn` per track change). Because all
submits happen on the main thread (Q1), generation order is exactly the track
order the user produced — the worker's "latest" is well defined.

#### `src-tauri/src/macos_notifications.m` — close the trigger-window race:

```objc
static NSString *const kNowPlayingIdentifier = @"ytm-now-playing";
```

Inside the `dispatch_async` block of `ytm_notifications_show`, replace the
`removeAllDeliveredNotifications` call and the request construction with:

```objc
        // Cancel the previous track's notification whether it is still
        // pending (scheduled, not yet presented) or already delivered.
        // removeAllDeliveredNotifications alone leaves pending requests
        // alive — that gap is how a stale banner could outlive a fresh one
        // under rapid skips — and it also swept away every other
        // notification this app ever posts.
        [center removePendingNotificationRequestsWithIdentifiers:@[kNowPlayingIdentifier]];
        [center removeDeliveredNotificationsWithIdentifiers:@[kNowPlayingIdentifier]];
```

```objc
        // nil trigger: deliver immediately. There is no pending window at
        // all, so no interleaving can slip a stale notification past the
        // removal above.
        UNNotificationRequest *request =
            [UNNotificationRequest requestWithIdentifier:kNowPlayingIdentifier
                                                 content:content
                                                 trigger:nil];
```

The Rust-side unique `identifier` argument is still passed (it names the art
file) but the *request* identifier becomes the fixed `kNowPlayingIdentifier`.
Remove-then-add with a fixed identifier is deliberate rather than relying on
same-identifier replacement alone: replacing a *delivered* notification by
reusing its identifier updates Notification Center but is not guaranteed to
present a fresh banner (documentation is ambiguous across macOS versions —
flagging this as the one behavior only manual testing can pin down; the
remove-then-add sequence sidesteps it).

#### Why every enumerated interleaving is now closed

1. *Stale track wins* — impossible: the worker never posts a job whose
   generation is stale (checked after fetch, before ObjC), and only one job
   can be fetching at a time.
2. *Out-of-order barrage* — impossible: posts leave a single thread in
   generation order; intermediate tracks whose fetch never started are
   dropped from the one-slot mailbox, not delivered late.
3. *Stale-clears-fresh / stacking* — impossible: nil trigger removes the
   pending window; per-identifier removePending+removeDelivered makes the
   remove-then-add atomic enough on the serially-executing main queue.
4. *Unbounded work* — bounded to one thread, one socket, one in-flight fetch;
   superseded fetches abandon within one chunk read.
5. *Collateral clearing* — per-identifier removal leaves unrelated
   notifications alone.

Residual, stated honestly: cancellation latency is one `read` call (worst
case ~5–10 s against a hung server, per the existing timeouts) during which
the newest job waits in the mailbox — visible as a slightly late notification,
never as a wrong or out-of-order one. And the worker thread is detached for
process lifetime; at quit it dies mid-fetch at worst, same as today but with
exactly one thread instead of n.
