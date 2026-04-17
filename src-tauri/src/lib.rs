use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::Manager;

const CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

mod media;
use media::MediaControlsWrapper;

#[cfg(target_os = "macos")]
mod macos_notifications;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct TrackState {
    pub title: String,
    pub artist: String,
    pub art: String,
    #[serde(rename = "isPlaying")]
    pub is_playing: bool,
}

pub struct AppState {
    pub last_title: Mutex<String>,
    pub media_controls: MediaControlsWrapper,
}

pub fn should_notify_track_change(new_track: &TrackState, last_title: &mut String) -> bool {
    if *last_title != new_track.title && new_track.is_playing {
        *last_title = new_track.title.clone();
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
fn handle_track_changed(
    payload: TrackState,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) {
    state.media_controls.update(&payload);

    let mut last_title = state.last_title.lock().unwrap();
    if should_notify_track_change(&payload, &mut last_title) {
        let title = payload.title.clone();
        let artist = payload.artist.clone();
        let art = payload.art.clone();

        std::thread::spawn(move || {
            #[cfg(target_os = "macos")]
            {
                if let Err(e) = macos_notifications::show_track_notification(&title, &artist, &art) {
                    eprintln!("[YTM Yagami] Notification failed: {e}");
                }
            }
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            last_title: Mutex::new(String::new()),
            media_controls: MediaControlsWrapper::new(),
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            macos_notifications::request_authorization();

            let _window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External("https://music.youtube.com".parse().unwrap()),
            )
            .title("YouTube Music")
            .inner_size(1024.0, 768.0)
            .user_agent(CHROME_UA)
            .initialization_script(include_str!("../../src/chrome_spoof.js"))
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
            .on_navigation(|url| {
                let url_str = url.as_str();
                if url_str.contains("youtube.com")
                    || url_str.contains("google.com")
                    || url_str.contains("googleapis.com")
                    || url_str.contains("gstatic.com")
                    || url_str.contains("accounts.google")
                {
                    true
                } else {
                    let _ = open::that(url_str);
                    false
                }
            })
            .build()?;

            let app_state: tauri::State<AppState> = app.state();
            app_state.media_controls.init(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![handle_track_changed, request_notification_permission])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_notify_on_new_playing_track() {
        let mut last_title = String::new();
        let track = TrackState {
            title: "New Song".to_string(),
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: true,
        };

        let notify = should_notify_track_change(&track, &mut last_title);
        assert!(notify);
        assert_eq!(last_title, "New Song");
    }

    #[test]
    fn test_should_not_notify_on_pause() {
        let mut last_title = String::from("New Song");
        let track = TrackState {
            title: "New Song".to_string(), // Same song
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: false, // Paused
        };

        let notify = should_notify_track_change(&track, &mut last_title);
        assert!(!notify);
    }

    #[test]
    fn test_should_not_notify_on_same_song_playing() {
        let mut last_title = String::from("New Song");
        let track = TrackState {
            title: "New Song".to_string(),
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: true,
        };

        let notify = should_notify_track_change(&track, &mut last_title);
        assert!(!notify);
    }

    #[test]
    fn test_should_not_notify_on_different_song_paused() {
        let mut last_title = String::from("Old Song");
        let track = TrackState {
            title: "New Song".to_string(),
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: false,
        };

        let notify = should_notify_track_change(&track, &mut last_title);
        assert!(!notify);
        assert_eq!(last_title, "Old Song"); // should not update last_title
    }

    #[test]
    fn test_sequential_track_changes() {
        let mut last_title = String::new();

        let track1 = TrackState {
            title: "Song A".to_string(),
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: true,
        };
        assert!(should_notify_track_change(&track1, &mut last_title));

        // Same song again — no notification
        assert!(!should_notify_track_change(&track1, &mut last_title));

        let track2 = TrackState {
            title: "Song B".to_string(),
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: true,
        };
        assert!(should_notify_track_change(&track2, &mut last_title));
        assert_eq!(last_title, "Song B");
    }

    #[test]
    fn test_track_state_serialization() {
        let track = TrackState {
            title: "Test".to_string(),
            artist: "Artist".to_string(),
            art: "url".to_string(),
            is_playing: true,
        };
        let json = serde_json::to_string(&track).unwrap();
        assert!(json.contains("\"isPlaying\":true"));

        let deserialized: TrackState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, track);
    }
}
