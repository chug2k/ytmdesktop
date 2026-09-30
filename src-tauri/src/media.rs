use crate::TrackState;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, PlatformConfig};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tauri::WebviewWindow;

/// Player commands are dispatched by evaluating JS in the page. Every snippet
/// must tolerate a missing element: the player bar is absent while YTM boots,
/// and a `null` deref here throws inside `eval` where nothing can observe it.
const JS_PLAY: &str = "(function(){var v=document.querySelector('video');if(v)v.play();})()";
const JS_PAUSE: &str = "(function(){var v=document.querySelector('video');if(v)v.pause();})()";
const JS_TOGGLE: &str = "(function(){var v=document.querySelector('video');if(!v)return;if(v.paused)v.play();else v.pause();})()";
const JS_NEXT: &str = "(function(){var b=document.querySelector('ytmusic-player-bar .next-button')||document.querySelector('.next-button');if(b)b.click();})()";
const JS_PREVIOUS: &str = "(function(){var b=document.querySelector('ytmusic-player-bar .previous-button')||document.querySelector('.previous-button');if(b)b.click();})()";

#[derive(Default)]
pub struct MediaControlsWrapper {
    controls: Arc<Mutex<Option<MediaControls>>>,
}

impl MediaControlsWrapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach OS media controls to `window`.
    ///
    /// Takes the window rather than an `AppHandle` because the Windows backend
    /// requires a real HWND — souvlaki `.expect()`s on `hwnd: None`, so passing
    /// `None` there panics the app at startup rather than degrading.
    pub fn init(&self, window: &WebviewWindow) {
        #[cfg(target_os = "windows")]
        let hwnd = match window.hwnd() {
            Ok(handle) => Some(handle.0 as *mut std::ffi::c_void),
            Err(e) => {
                eprintln!("[YTM Yagami] no HWND, skipping media controls: {e}");
                return;
            }
        };
        #[cfg(not(target_os = "windows"))]
        let hwnd = None;

        let config = PlatformConfig {
            dbus_name: "com.charleslee.ytmyagami",
            display_name: "YouTube Music",
            hwnd,
        };

        let mut controls = match MediaControls::new(config) {
            Ok(controls) => controls,
            Err(e) => {
                eprintln!("[YTM Yagami] failed to initialize media controls: {e}");
                return;
            }
        };

        let app_handle = window.app_handle().clone();
        let attached = controls.attach(move |event| {
            let js = match event {
                MediaControlEvent::Play => JS_PLAY,
                // Stop has no distinct meaning for a streaming player bar.
                MediaControlEvent::Pause | MediaControlEvent::Stop => JS_PAUSE,
                // The keyboard play/pause key routes through MPRemoteCommandCenter's
                // togglePlayPauseCommand, which souvlaki surfaces as Toggle. Dropping
                // this arm makes the hardware key a no-op.
                MediaControlEvent::Toggle => JS_TOGGLE,
                MediaControlEvent::Next => JS_NEXT,
                MediaControlEvent::Previous => JS_PREVIOUS,
                _ => return,
            };
            if let Some(window) = app_handle.get_webview_window("main") {
                let _ = window.eval(js);
            }
        });

        if let Err(e) = attached {
            eprintln!("[YTM Yagami] failed to attach media controls: {e}");
            return;
        }

        *self.controls.lock().unwrap() = Some(controls);
    }

    pub fn update(&self, state: &TrackState) {
        let mut guard = self.controls.lock().unwrap();
        let Some(controls) = guard.as_mut() else {
            return;
        };

        let cover_url = if state.art.is_empty() {
            None
        } else {
            Some(state.art.as_str())
        };

        let _ = controls.set_metadata(MediaMetadata {
            title: Some(&state.title),
            artist: Some(&state.artist),
            album: None,
            cover_url,
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
