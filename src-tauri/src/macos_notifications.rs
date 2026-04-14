#[cfg(target_os = "macos")]
use std::{
    error::Error,
    ffi::CString,
    fs,
    os::raw::c_char,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn ytm_notifications_request_authorization();
    fn ytm_notifications_show(
        identifier: *const c_char,
        title: *const c_char,
        subtitle: *const c_char,
        image_path: *const c_char,
    );
}

#[cfg(target_os = "macos")]
pub fn request_authorization() {
    unsafe {
        ytm_notifications_request_authorization();
    }
}

#[cfg(target_os = "macos")]
pub fn show_track_notification(
    title: &str,
    artist: &str,
    art_url: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let image_path = if art_url.trim().is_empty() {
        String::new()
    } else {
        download_album_art(art_url)?.to_string_lossy().into_owned()
    };

    let id = format!(
        "track-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    );

    let id = CString::new(id)?;
    let title = CString::new(title)?;
    let artist = CString::new(artist)?;
    let image_path = CString::new(image_path)?;

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

#[cfg(target_os = "macos")]
fn download_album_art(url: &str) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let response = reqwest::blocking::get(url)?.error_for_status()?;
    let bytes = response.bytes()?;

    let ext = infer::get(&bytes)
        .map(|kind| kind.extension())
        .unwrap_or("jpg");

    let dir = std::env::temp_dir().join("com.ytmyagami.desktop-notifications");
    fs::create_dir_all(&dir)?;

    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let path = dir.join(format!("album-art-{ts}.{ext}"));

    fs::write(&path, &bytes)?;
    Ok(path)
}
