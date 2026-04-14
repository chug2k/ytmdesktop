use crate::TrackState;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, PlatformConfig};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri::Manager;

pub struct MediaControlsWrapper {
    controls: Arc<Mutex<Option<MediaControls>>>,
}

impl MediaControlsWrapper {
    pub fn new() -> Self {
        Self {
            controls: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init(&self, app_handle: AppHandle) {
        let hwnd = None; // souvlaki handles missing hwnd in newer versions by default

        let config = PlatformConfig {
            dbus_name: "com.ytmdesktop.tauri",
            display_name: "YouTube Music",
            hwnd,
        };

        match MediaControls::new(config) {
            Ok(mut controls) => {
                let app_handle_clone = app_handle.clone();
                controls
                    .attach(move |event| {
                        let js = match event {
                            MediaControlEvent::Play => "document.querySelector('video').play();",
                            MediaControlEvent::Pause => "document.querySelector('video').pause();",
                            MediaControlEvent::Next => "document.querySelector('.next-button').click();",
                            MediaControlEvent::Previous => "document.querySelector('.previous-button').click();",
                            _ => return,
                        };
                        if let Some(window) = app_handle_clone.get_webview_window("main") {
                            let _ = window.eval(js);
                        }
                    })
                    .expect("Failed to attach media controls");

                let mut guard = self.controls.lock().unwrap();
                *guard = Some(controls);
            }
            Err(e) => {
                eprintln!("Failed to initialize media controls: {}", e);
            }
        }
    }

    pub fn update(&self, state: &TrackState) {
        let mut guard = self.controls.lock().unwrap();
        if let Some(controls) = guard.as_mut() {
            let _ = controls.set_metadata(MediaMetadata {
                title: Some(&state.title),
                artist: Some(&state.artist),
                album: None,
                cover_url: Some(&state.art),
                duration: None,
            });

            let playback_status = if state.is_playing {
                souvlaki::MediaPlayback::Playing { progress: None }
            } else {
                souvlaki::MediaPlayback::Paused { progress: None }
            };

            let _ = controls.set_playback(playback_status);
        }
    }
}
