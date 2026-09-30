use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::Manager;

/// The host the player lives on. Only this page gets `inject.js`.
const PLAYER_HOST: &str = "music.youtube.com";

/// Registrable domains allowed to load inside the app window. Everything else
/// is handed to the user's real browser.
const ALLOWED_DOMAINS: &[&str] = &[
    "youtube.com",
    "google.com",
    "googleapis.com",
    "gstatic.com",
    "googleusercontent.com",
    "ggpht.com",
    "ytimg.com",
];

mod media;
use media::MediaControlsWrapper;

#[cfg(target_os = "macos")]
mod macos_notifications;
#[cfg(target_os = "macos")]
mod passkey;
#[cfg(target_os = "macos")]
mod user_agent;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct TrackState {
    pub title: String,
    pub artist: String,
    pub art: String,
    #[serde(rename = "isPlaying")]
    pub is_playing: bool,
}

impl TrackState {
    /// Identity of a track for notification de-duplication. Title alone
    /// collides across covers, live versions, and same-named songs by
    /// different artists, silently swallowing those notifications.
    fn notification_key(&self) -> String {
        format!("{}\u{1f}{}", self.title, self.artist)
    }

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

/// Track text is attacker-influenced — it arrives from page script, which is not
/// necessarily Google's — and ends up in an OS notification outside the window.
const MAX_TEXT_CHARS: usize = 200;
/// Longer than any real artwork URL; a cheap bound before parsing.
const MAX_ART_URL_BYTES: usize = 2048;

/// Registrable domains Google serves YouTube Music artwork from.
///
/// `art` is not merely displayed. It is fetched by the *native* process twice
/// over: by `reqwest` for the notification attachment, and by
/// `NSImage initWithContentsOfURL:` inside souvlaki for the now-playing artwork.
/// Both run outside every restriction the webview enforces — CORS, Private
/// Network Access, mixed content, the page's CSP — so an unvalidated URL here is
/// a request-forgery primitive aimed at loopback and the LAN.
const ART_DOMAINS: &[&str] = &["googleusercontent.com", "ggpht.com", "ytimg.com"];

/// Strip control characters, which corrupt notification layout and cannot cross
/// the Objective-C string bridge, and cap the length.
fn sanitize_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Return the URL only if it is an HTTPS URL on a known Google artwork host.
/// Anything else becomes empty, which both consumers already treat as "no
/// artwork" — so a bad URL degrades to a text-only notification rather than
/// dropping the track update, the right failure mode for something decorative.
fn sanitize_art_url(value: &str) -> String {
    if value.len() > MAX_ART_URL_BYTES {
        return String::new();
    }
    let Ok(url) = url::Url::parse(value) else {
        return String::new();
    };
    // HTTPS only: plain http on a hostile network is attacker-controlled, and a
    // non-HTTP scheme is precisely what makes the NSImage path a file-read probe.
    if url.scheme() != "https" {
        return String::new();
    }
    // Credentials and explicit ports never appear on real artwork URLs, and are
    // how a lookalike host gets smuggled past a careless check.
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

pub struct AppState {
    pub last_notified: Mutex<String>,
    pub media_controls: MediaControlsWrapper,
    #[cfg(target_os = "macos")]
    pub passkeys: passkey::Passkeys,
}

/// True if `host` is, or is a subdomain of, one of `ALLOWED_DOMAINS`.
///
/// A substring test (`host.contains("youtube.com")`) would accept
/// `youtube.com.evil.com`, and a bare `ends_with("google.com")` would accept
/// `evilgoogle.com` — the dot matters.
pub fn is_allowed_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ALLOWED_DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        || is_regional_google_accounts(&host)
}

/// `accounts.google.<country>`, for example `accounts.google.com.vn`.
///
/// Sign-in ends with a redirect to the Google account host of the user's
/// country, which sets the session cookie. The suffix must be one two-letter
/// label, or `com`/`co` and one two-letter label, so `accounts.google.com.evil.com`
/// and `accounts.google.evil` do not match.
fn is_regional_google_accounts(host: &str) -> bool {
    let Some(suffix) = host.strip_prefix("accounts.google.") else {
        return false;
    };
    let is_cc = |label: &str| label.len() == 2 && label.bytes().all(|b| b.is_ascii_lowercase());
    match suffix.split('.').collect::<Vec<_>>().as_slice() {
        [cc] => is_cc(cc),
        [second, cc] => matches!(*second, "com" | "co") && is_cc(cc),
        _ => false,
    }
}

/// A `window.open` that is the sign-in handoff, not a link the user chose.
///
/// Google finishes passkey sign-in by opening `youtube.com/signin`, which then
/// redirects to the player. Those two addresses must load in this window.
/// Other new windows, including Help and Terms, still go to the system browser.
pub fn should_load_in_window(url: &url::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == PLAYER_HOST || host == "accounts.google.com" || host == "accounts.youtube.com" {
        return true;
    }
    let signin = url.path() == "/signin" || url.path().starts_with("/signin/");
    signin && (host == "youtube.com" || host.ends_with(".youtube.com"))
}

pub fn should_notify_track_change(new_track: &TrackState, last_notified: &mut String) -> bool {
    let key = new_track.notification_key();
    if *last_notified != key && new_track.is_playing {
        *last_notified = key;
        true
    } else {
        false
    }
}

#[tauri::command]
fn request_notification_permission() {
    #[cfg(target_os = "macos")]
    macos_notifications::request_authorization();
}

#[tauri::command(rename_all = "snake_case")]
fn handle_track_changed(payload: TrackState, state: tauri::State<'_, AppState>) {
    // Must be first: `media_controls.update` hands `art` straight to
    // NSImage initWithContentsOfURL: on every invoke, gated by nothing.
    let payload = payload.sanitized();

    state.media_controls.update(&payload);

    let mut last_notified = state.last_notified.lock().unwrap();
    if should_notify_track_change(&payload, &mut last_notified) {
        // Drop the lock before the (potentially slow) notification hand-off.
        drop(last_notified);

        #[cfg(target_os = "macos")]
        {
            let title = payload.title.clone();
            let artist = payload.artist.clone();
            let art = payload.art.clone();
            std::thread::spawn(move || {
                if let Err(e) = macos_notifications::show_track_notification(&title, &artist, &art)
                {
                    eprintln!("[YTM Yagami] Notification failed: {e}");
                }
            });
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            last_notified: Mutex::new(String::new()),
            media_controls: MediaControlsWrapper::new(),
            #[cfg(target_os = "macos")]
            passkeys: Default::default(),
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            macos_notifications::request_authorization();

            let handle = app.handle().clone();
            let builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(format!("https://{PLAYER_HOST}").parse().unwrap()),
            )
            .title("YouTube Music")
            .inner_size(1024.0, 768.0)
            .initialization_script(include_str!("../../src/ipc_transport.js"))
            // Only acts on accounts.google.com; see src/passkey/mod.rs.
            .initialization_script(include_str!("../../src/passkey_hybrid.js"))
            .on_page_load(|window, payload| {
                // The player bar only exists on music.youtube.com; injecting
                // anywhere else just installs a MutationObserver that never fires.
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
                    && payload.url().host_str() == Some(PLAYER_HOST)
                {
                    let _ = window.eval(include_str!("../../src/inject.js"));
                }
            })
            .on_navigation(|url| {
                // Non-HTTP schemes (about:blank, blob:) are internal to the
                // webview — OAuth popups need them — and must not be handed to
                // the OS as if they were links.
                if !matches!(url.scheme(), "http" | "https") {
                    return true;
                }
                if is_allowed_host(url.host_str().unwrap_or_default()) {
                    return true;
                }
                let _ = open::that(url.as_str());
                false
            })
            .on_new_window(move |url, _features| {
                // The app has one window. Without this handler, `window.open`
                // does nothing. The sign-in return is itself a `window.open`
                // of the player, so that address loads here. Any other http
                // address goes to the system browser.
                if should_load_in_window(&url) {
                    if let Some(window) = handle.get_webview_window("main") {
                        let _ = window.navigate(url);
                    }
                } else if matches!(url.scheme(), "http" | "https") {
                    let _ = open::that(url.as_str());
                }
                tauri::webview::NewWindowResponse::Deny
            });

            #[cfg(target_os = "macos")]
            let builder = builder.user_agent(&user_agent::safari());

            let window = builder.build()?;

            let app_state: tauri::State<AppState> = app.state();
            app_state.media_controls.init(&window);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            handle_track_changed,
            request_notification_permission,
            #[cfg(target_os = "macos")]
            passkey::passkey_start,
            #[cfg(target_os = "macos")]
            passkey::passkey_next,
            #[cfg(target_os = "macos")]
            passkey::passkey_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, is_playing: bool) -> TrackState {
        TrackState {
            title: title.to_string(),
            artist: artist.to_string(),
            art: "url".to_string(),
            is_playing,
        }
    }

    #[test]
    fn test_should_notify_on_new_playing_track() {
        let mut last = String::new();
        assert!(should_notify_track_change(
            &track("New Song", "Artist", true),
            &mut last
        ));
        assert_eq!(last, "New Song\u{1f}Artist");
    }

    #[test]
    fn test_should_not_notify_on_pause() {
        let mut last = String::from("New Song\u{1f}Artist");
        assert!(!should_notify_track_change(
            &track("New Song", "Artist", false),
            &mut last
        ));
    }

    #[test]
    fn test_should_not_notify_on_same_song_playing() {
        let mut last = String::from("New Song\u{1f}Artist");
        assert!(!should_notify_track_change(
            &track("New Song", "Artist", true),
            &mut last
        ));
    }

    #[test]
    fn test_should_not_notify_on_different_song_paused() {
        let mut last = String::from("Old Song\u{1f}Artist");
        assert!(!should_notify_track_change(
            &track("New Song", "Artist", false),
            &mut last
        ));
        assert_eq!(last, "Old Song\u{1f}Artist");
    }

    #[test]
    fn test_notifies_on_same_title_by_different_artist() {
        let mut last = String::new();
        assert!(should_notify_track_change(
            &track("Hallelujah", "Leonard Cohen", true),
            &mut last
        ));
        assert!(should_notify_track_change(
            &track("Hallelujah", "Jeff Buckley", true),
            &mut last
        ));
    }

    #[test]
    fn test_sequential_track_changes() {
        let mut last = String::new();
        let first = track("Song A", "Artist", true);
        assert!(should_notify_track_change(&first, &mut last));
        assert!(!should_notify_track_change(&first, &mut last));

        assert!(should_notify_track_change(
            &track("Song B", "Artist", true),
            &mut last
        ));
        assert_eq!(last, "Song B\u{1f}Artist");
    }

    #[test]
    fn test_track_state_serialization() {
        let track = track("Test", "Artist", true);
        let json = serde_json::to_string(&track).unwrap();
        assert!(json.contains("\"isPlaying\":true"));
        assert_eq!(serde_json::from_str::<TrackState>(&json).unwrap(), track);
    }

    #[test]
    fn allows_player_and_auth_hosts() {
        assert!(is_allowed_host("music.youtube.com"));
        assert!(is_allowed_host("accounts.google.com"));
        assert!(is_allowed_host("youtube.com"));
        assert!(is_allowed_host("lh3.googleusercontent.com"));
        assert!(is_allowed_host("MUSIC.YOUTUBE.COM"));
        assert!(is_allowed_host("accounts.google.com.vn"));
        assert!(is_allowed_host("accounts.google.co.uk"));
        assert!(is_allowed_host("accounts.google.de"));
    }

    #[test]
    fn keeps_real_artwork_urls() {
        let url = "https://lh3.googleusercontent.com/art=w544-h544-l90-rj";
        assert_eq!(sanitize_art_url(url), url);
        assert!(!sanitize_art_url("https://i.ytimg.com/vi/abc/hq.jpg").is_empty());
        assert!(!sanitize_art_url("https://yt3.ggpht.com/a/xyz").is_empty());
    }

    #[test]
    fn drops_art_urls_that_could_reach_the_local_network() {
        // Both native fetch paths bypass every webview-level protection, so
        // these are request-forgery vectors, not just bad artwork.
        assert_eq!(sanitize_art_url("http://127.0.0.1:11434/api/pull"), "");
        assert_eq!(sanitize_art_url("http://192.168.1.1/admin/reboot"), "");
        assert_eq!(
            sanitize_art_url("http://169.254.169.254/latest/meta-data/"),
            ""
        );
        // file:// is the NSImage path's local-read probe.
        assert_eq!(sanitize_art_url("file:///etc/passwd"), "");
        assert_eq!(sanitize_art_url("https://evil.com/x.jpg"), "");
        // Lookalike and smuggling shapes.
        assert_eq!(sanitize_art_url("https://ytimg.com.evil.com/x.jpg"), "");
        assert_eq!(
            sanitize_art_url("https://lh3.googleusercontent.com@evil.com/x"),
            ""
        );
        assert_eq!(
            sanitize_art_url("https://lh3.googleusercontent.com:8080/x"),
            ""
        );
        assert_eq!(sanitize_art_url("not a url"), "");
        assert_eq!(sanitize_art_url(""), "");
    }

    #[test]
    fn caps_and_cleans_notification_text() {
        assert_eq!(sanitize_text("  Song Title  "), "Song Title");
        // Control characters cannot cross the ObjC string bridge.
        assert_eq!(sanitize_text("Song\u{0}\u{7}Title"), "SongTitle");
        assert_eq!(
            sanitize_text(&"a".repeat(5000)).chars().count(),
            MAX_TEXT_CHARS
        );
        // Truncation is by char, not byte: slicing UTF-8 mid-codepoint panics.
        let emoji = "🎵".repeat(500);
        assert_eq!(sanitize_text(&emoji).chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn sanitizing_a_payload_preserves_playback_state() {
        let dirty = TrackState {
            title: "  Song\u{0}  ".to_string(),
            artist: "Artist".to_string(),
            art: "http://127.0.0.1/x".to_string(),
            is_playing: true,
        };
        let clean = dirty.sanitized();
        assert_eq!(clean.title, "Song");
        assert_eq!(clean.art, "");
        assert!(clean.is_playing);
    }

    #[test]
    fn sign_in_return_stays_in_the_window() {
        let stay = [
            "https://music.youtube.com/",
            "https://music.youtube.com/watch?v=abc",
            "https://www.youtube.com/signin?action_handle_signin=true&next=https://music.youtube.com/",
            "https://accounts.google.com/CheckCookie?continue=https://www.youtube.com/signin",
            "https://accounts.youtube.com/accounts/CheckConnection",
        ];
        for url in stay {
            assert!(should_load_in_window(&url.parse().unwrap()), "{url}");
        }
    }

    #[test]
    fn other_new_windows_leave_the_app() {
        let leave = [
            "https://www.youtube.com/watch?v=abc",
            "https://support.google.com/youtubemusic",
            "https://policies.google.com/privacy",
            "https://example.com/",
            "mailto:chug2k@gmail.com",
        ];
        for url in leave {
            assert!(!should_load_in_window(&url.parse().unwrap()), "{url}");
        }
    }

    #[test]
    fn rejects_lookalike_hosts() {
        // Suffix-confusion: the attacker owns the registrable domain.
        assert!(!is_allowed_host("youtube.com.evil.com"));
        assert!(!is_allowed_host("evilgoogle.com"));
        assert!(!is_allowed_host("accounts.google.com.evil.com"));
        assert!(!is_allowed_host("accounts.google.evil"));
        assert!(!is_allowed_host("accounts.google.net.vn"));
        assert!(!is_allowed_host("notyoutube.com"));
        assert!(!is_allowed_host("evil.com"));
        assert!(!is_allowed_host(""));
    }
}
