use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

mod media;
use media::MediaControlsWrapper;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct TrackState {
    pub title: String,
    pub artist: String,
    pub art: String,
    #[serde(rename = "isPlaying")]
    pub is_playing: bool,
}

// Store the last track title to prevent duplicate notifications
pub struct AppState {
    pub last_title: Mutex<String>,
    pub media_controls: MediaControlsWrapper,
}

// Function to handle the state update logic so it's easily testable without the Tauri App Handle
pub fn should_notify_track_change(new_track: &TrackState, last_title: &mut String) -> bool {
    if *last_title != new_track.title && new_track.is_playing {
        *last_title = new_track.title.clone();
        true
    } else {
        false
    }
}

// Ensure the command is private so rustc macro definition doesn't conflict during export
#[tauri::command(rename_all = "snake_case")]
fn handle_track_changed(
    payload: TrackState,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) {
    state.media_controls.update(&payload);

    let mut last_title = state.last_title.lock().unwrap();
    if should_notify_track_change(&payload, &mut last_title) {
        let _ = app
            .notification()
            .builder()
            .title(&payload.title)
            .body(&payload.artist)
            .show();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            last_title: Mutex::new(String::new()),
            media_controls: MediaControlsWrapper::new(),
        })
        .setup(move |app| {
            // Initialize media controls after setup
            let app_state: tauri::State<AppState> = app.state();
            app_state.media_controls.init(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![handle_track_changed])
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
}
