//! macOS notification bridge. The whole module is gated behind
//! `#[cfg(target_os = "macos")]` at the `mod` declaration in lib.rs.

use std::{
    error::Error,
    ffi::CString,
    fs,
    os::raw::c_char,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Album art is decorative. Give it a hard deadline so a hung connection can
/// never wedge the notification worker.
const ART_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ART_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Cover art is a few hundred KB; anything past this is not album art.
const MAX_ART_BYTES: u64 = 8 * 1024 * 1024;
/// Scratch files older than this are leftovers from a crashed run.
const ART_STALE_AFTER: Duration = Duration::from_secs(60);

unsafe extern "C" {
    fn ytm_notifications_request_authorization();
    fn ytm_notifications_show(
        identifier: *const c_char,
        title: *const c_char,
        subtitle: *const c_char,
        image_path: *const c_char,
    );
}

pub fn request_authorization() {
    unsafe {
        ytm_notifications_request_authorization();
    }
}

/// A process-unique notification id. A millisecond timestamp alone collides
/// when two tracks change inside the same tick.
fn next_notification_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("track-{millis}-{seq}")
}

/// Objective-C string bridging cannot carry interior NULs, and a track title is
/// attacker-influenced text from the page. Strip rather than fail the notification.
fn to_cstring(value: &str) -> CString {
    CString::new(value.replace('\0', "")).unwrap_or_default()
}

pub fn show_track_notification(
    title: &str,
    artist: &str,
    art_url: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let id = next_notification_id();

    // A failed art fetch must degrade to a text-only notification, never
    // suppress the notification entirely.
    let image_path = if art_url.trim().is_empty() {
        String::new()
    } else {
        match download_album_art(art_url, &id) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(e) => {
                eprintln!("[YTM Yagami] album art unavailable, sending without it: {e}");
                String::new()
            }
        }
    };

    let id = to_cstring(&id);
    let title = to_cstring(title);
    let artist = to_cstring(artist);
    let image_path = to_cstring(&image_path);

    unsafe {
        ytm_notifications_show(
            id.as_ptr(),
            title.as_ptr(),
            artist.as_ptr(),
            image_path.as_ptr(),
        );
    }

    Ok(())
}

fn http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(ART_CONNECT_TIMEOUT)
            .timeout(ART_TOTAL_TIMEOUT)
            .build()
            .expect("failed to build HTTP client")
    })
}

fn art_dir() -> PathBuf {
    std::env::temp_dir().join("com.charleslee.ytmyagami-notifications")
}

/// Delete leftovers from previous runs without touching files a concurrent
/// notification may still be about to attach.
fn prune_stale_art(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        if age.is_some_and(|age| age > ART_STALE_AFTER) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn download_album_art(url: &str, id: &str) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let response = http_client().get(url).send()?.error_for_status()?;

    if let Some(len) = response.content_length() {
        if len > MAX_ART_BYTES {
            return Err(format!("album art too large: {len} bytes").into());
        }
    }

    let bytes = response.bytes()?;
    if bytes.len() as u64 > MAX_ART_BYTES {
        return Err(format!("album art too large: {} bytes", bytes.len()).into());
    }

    let ext = infer::get(&bytes)
        .map(|kind| kind.extension())
        .unwrap_or("jpg");

    let dir = art_dir();
    fs::create_dir_all(&dir)?;
    prune_stale_art(&dir);

    // Unique per notification: UNNotificationAttachment moves the file into the
    // system attachment store, and two in-flight notifications sharing one path
    // race each other for it.
    let path = dir.join(format!("{id}.{ext}"));
    fs::write(&path, &bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_ids_are_unique_within_a_millisecond() {
        let a = next_notification_id();
        let b = next_notification_id();
        assert_ne!(a, b);
    }

    #[test]
    fn to_cstring_strips_interior_nuls_instead_of_failing() {
        let bridged = to_cstring("Song\0Title");
        assert_eq!(bridged.to_str().unwrap(), "SongTitle");
    }

    #[test]
    fn to_cstring_handles_empty_input() {
        assert_eq!(to_cstring("").to_str().unwrap(), "");
    }
}
