//! Passkey sign-in with a phone ("hybrid", caBLE v2) for Google's sign-in
//! page.
//!
//! WKWebView refuses WebAuthn for google.com: that needs an entitlement Apple
//! gives only to web browsers. So `src/passkey_hybrid.js` takes over
//! `navigator.credentials.get` on accounts.google.com and hands the request
//! here. This module does what Chrome does for "use a phone": it shows a QR
//! code, finds the phone's BLE advert, meets it on Google's or Apple's tunnel
//! server, and asks it for the assertion.
//!
//! This makes the app a WebAuthn client for one origin, so it must enforce
//! what a browser enforces: the origin comes from the webview, never from the
//! page, and the RP ID must be one that origin may claim.

mod ble;
mod cable;
mod cbor;
mod noise;
mod tunnel;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};

use ble::{BleEvent, BleState, Scanner};
use cable::{Discovery, QrPayload};
use noise::KeyPair;

/// The only page allowed to ask, and the RP IDs it may ask for. A browser
/// would accept any registrable suffix of the origin; this app has one use.
const ORIGIN_HOST: &str = "accounts.google.com";
const RP_IDS: &[&str] = &["google.com", "accounts.google.com"];

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MIN_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);
/// How long one `passkey_next` call waits before it reports "pending".
const LONG_POLL: Duration = Duration::from_secs(20);

/// Google's sign-in page puts several kilobytes in `challenge`. A browser
/// accepts that and hashes it into `clientDataJSON`. The cap only stops a
/// page from handing the app an unbounded buffer.
const MAX_CHALLENGE: usize = 64 * 1024;
const MAX_CREDENTIALS: usize = 64;
const MAX_CREDENTIAL_ID: usize = 1023;

/// A failure, carrying the `DOMException` name the page should see.
#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub name: &'static str,
    pub message: String,
}

impl Failure {
    fn new(name: &'static str, message: &str) -> Self {
        Self {
            name,
            message: message.to_string(),
        }
    }
    fn not_allowed(message: &str) -> Self {
        Self::new("NotAllowedError", message)
    }
    fn security(message: &str) -> Self {
        Self::new("SecurityError", message)
    }
    fn internal(message: &str) -> Self {
        Self::not_allowed(message)
    }
    fn network(message: &str) -> Self {
        Self::not_allowed(message)
    }
    fn protocol(message: &str) -> Self {
        Self::not_allowed(message)
    }
    fn cancelled() -> Self {
        Self::not_allowed("Cancelled")
    }
    fn timeout() -> Self {
        Self::not_allowed("Timed out")
    }
    fn declined() -> Self {
        Self::not_allowed("Declined on the phone")
    }
}

/// `PublicKeyCredentialRequestOptions`, as `passkey_hybrid.js` sends it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    challenge: String,
    rp_id: Option<String>,
    #[serde(default)]
    allow_credentials: Vec<String>,
    timeout: Option<u64>,
}

/// A request that passed every check, ready for the phone.
#[derive(Debug, PartialEq)]
struct Checked {
    rp_id: String,
    client_data_json: String,
    allow_list: Vec<Vec<u8>>,
    timeout: Duration,
}

fn b64_decode(value: &str) -> Option<Vec<u8>> {
    // Pages send base64url; tolerate padding.
    URL_SAFE_NO_PAD.decode(value.trim_end_matches('=')).ok()
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn check(page: &url::Url, request: &Request) -> Result<Checked, Failure> {
    if page.scheme() != "https" || page.host_str() != Some(ORIGIN_HOST) || page.port().is_some() {
        return Err(Failure::security(
            "Passkeys are only handled for Google sign-in",
        ));
    }
    let rp_id = request.rp_id.as_deref().unwrap_or(ORIGIN_HOST);
    if !RP_IDS.contains(&rp_id) {
        return Err(Failure::security("RP ID not valid for this origin"));
    }
    let challenge = b64_decode(&request.challenge)
        .filter(|c| (16..=MAX_CHALLENGE).contains(&c.len()))
        .ok_or_else(|| Failure::new("TypeError", "Bad challenge"))?;
    if request.allow_credentials.len() > MAX_CREDENTIALS {
        return Err(Failure::new("TypeError", "Too many credentials"));
    }
    let allow_list = request
        .allow_credentials
        .iter()
        .map(|id| b64_decode(id).filter(|id| !id.is_empty() && id.len() <= MAX_CREDENTIAL_ID))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| Failure::new("TypeError", "Bad credential ID"))?;
    let timeout = request
        .timeout
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
        .clamp(MIN_TIMEOUT, MAX_TIMEOUT);
    // The same serialisation, field for field, as Chrome and Safari.
    let client_data_json = format!(
        r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://{ORIGIN_HOST}","crossOrigin":false}}"#,
        b64(&challenge)
    );
    Ok(Checked {
        rp_id: rp_id.to_string(),
        client_data_json,
        allow_list,
        timeout,
    })
}

/// A plain SVG of the QR code. The page draws it as-is.
fn qr_svg(data: &str) -> Result<String, Failure> {
    let code = qrcode::QrCode::with_error_correction_level(data, qrcode::EcLevel::L)
        .map_err(|_| Failure::internal("QR code too large"))?;
    let width = code.width();
    let quiet = 4;
    let size = width + 2 * quiet;
    let mut path = String::new();
    for (i, color) in code.to_colors().iter().enumerate() {
        if *color == qrcode::Color::Dark {
            let (x, y) = (i % width + quiet, i / width + quiet);
            path.push_str(&format!("M{x} {y}h1v1h-1z"));
        }
    }
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" shape-rendering="crispEdges"><rect width="{size}" height="{size}" fill="#fff"/><path d="{path}" fill="#000"/></svg>"##
    ))
}

/// What the page's long-poll receives.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Event {
    /// Nothing new yet; ask again.
    Pending,
    /// The phone's advert arrived; the QR code is no longer needed.
    Connecting,
    /// The tunnel is up; the phone is asking the user.
    Confirm,
    Done {
        credential: Credential,
    },
    Failed {
        #[serde(flatten)]
        failure: Failure,
    },
}

/// The fields of a `PublicKeyCredential`, base64url-encoded.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    id: String,
    client_data_json: String,
    authenticator_data: String,
    signature: String,
    user_handle: Option<String>,
}

struct Session {
    id: u64,
    cancel: AtomicBool,
    events: Mutex<mpsc::Receiver<Event>>,
}

/// At most one sign-in runs at a time; a new one cancels the old.
#[derive(Default)]
pub struct Passkeys {
    current: Mutex<Option<Arc<Session>>>,
    next_id: AtomicU64,
}

impl Passkeys {
    fn session(&self, id: u64) -> Option<Arc<Session>> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .filter(|s| s.id == id)
            .cloned()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    session: u64,
    qr_svg: String,
}

#[tauri::command]
pub fn passkey_start(
    webview: tauri::Webview,
    state: tauri::State<'_, crate::AppState>,
    request: Request,
) -> Result<Started, Failure> {
    let page = webview
        .url()
        .map_err(|_| Failure::security("No page URL"))?;
    let checked = check(&page, &request)?;

    let rng = SystemRandom::new();
    let identity = KeyPair::generate(&rng).map_err(|_| Failure::internal("key error"))?;
    let mut secret = [0u8; 16];
    rng.fill(&mut secret)
        .map_err(|_| Failure::internal("random error"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let qr = QrPayload::get_assertion(identity.public_compressed(), secret, now).to_url();
    let qr_svg = qr_svg(&qr)?;

    let (tx, rx) = mpsc::channel();
    let passkeys = &state.passkeys;
    let session = Arc::new(Session {
        id: passkeys.next_id.fetch_add(1, Ordering::Relaxed),
        cancel: AtomicBool::new(false),
        events: Mutex::new(rx),
    });
    if let Some(old) = passkeys.current.lock().unwrap().replace(session.clone()) {
        old.cancel.store(true, Ordering::Relaxed);
    }

    let worker = session.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + checked.timeout;
        let event = match run(
            &checked,
            Discovery::new(secret),
            identity,
            &rng,
            &worker,
            deadline,
            &tx,
        ) {
            Ok(credential) => Event::Done { credential },
            Err(failure) => {
                eprintln!("[YTM Yagami] passkey sign-in failed: {}", failure.message);
                Event::Failed { failure }
            }
        };
        let _ = tx.send(event);
    });

    Ok(Started {
        session: session.id,
        qr_svg,
    })
}

#[tauri::command]
pub async fn passkey_next(
    state: tauri::State<'_, crate::AppState>,
    session: u64,
) -> Result<Event, Failure> {
    let session = state
        .passkeys
        .session(session)
        .ok_or_else(|| Failure::not_allowed("No such sign-in"))?;
    tauri::async_runtime::spawn_blocking(move || {
        match session.events.lock().unwrap().recv_timeout(LONG_POLL) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => Event::Pending,
            Err(mpsc::RecvTimeoutError::Disconnected) => Event::Failed {
                failure: Failure::internal("Sign-in ended"),
            },
        }
    })
    .await
    .map_err(|_| Failure::internal("worker failed"))
}

#[tauri::command]
pub fn passkey_cancel(state: tauri::State<'_, crate::AppState>, session: u64) {
    if let Some(session) = state.passkeys.session(session) {
        session.cancel.store(true, Ordering::Relaxed);
    }
}

fn run(
    request: &Checked,
    discovery: Discovery,
    identity: KeyPair,
    rng: &SystemRandom,
    session: &Session,
    deadline: Instant,
    events: &mpsc::Sender<Event>,
) -> Result<Credential, Failure> {
    let eid = {
        let (_scanner, adverts) = Scanner::start();
        loop {
            if session.cancel.load(Ordering::Relaxed) {
                return Err(Failure::cancelled());
            }
            if Instant::now() > deadline {
                return Err(Failure::timeout());
            }
            match adverts.recv_timeout(Duration::from_millis(250)) {
                Ok(BleEvent::Advert(advert)) => {
                    if let Some(eid) = discovery.decrypt_advert(&advert) {
                        break eid;
                    }
                }
                Ok(BleEvent::State(BleState::PoweredOff)) => {
                    return Err(Failure::not_allowed("Bluetooth is off"))
                }
                Ok(BleEvent::State(BleState::Unauthorized)) => {
                    return Err(Failure::not_allowed(
                        "YTM Yagami does not have Bluetooth permission",
                    ))
                }
                Ok(BleEvent::State(BleState::Unsupported)) => {
                    return Err(Failure::not_allowed("This Mac has no Bluetooth LE"))
                }
                Ok(BleEvent::State(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Failure::internal("Bluetooth scan stopped"))
                }
            }
        }
    };
    let _ = events.send(Event::Connecting);

    let url = eid
        .connect_url(&discovery.tunnel_id())
        .ok_or_else(|| Failure::protocol("Unknown tunnel server"))?;
    let mut tunnel = tunnel::Tunnel::open(
        &url,
        &discovery.psk(&eid),
        identity,
        rng,
        &session.cancel,
        deadline,
    )?;
    let _ = events.send(Event::Confirm);

    let client_data_hash = digest::digest(&digest::SHA256, request.client_data_json.as_bytes());
    let assertion = tunnel.get_assertion(
        &request.rp_id,
        client_data_hash.as_ref(),
        &request.allow_list,
    )?;
    Ok(Credential {
        id: b64(&assertion.credential_id),
        client_data_json: b64(request.client_data_json.as_bytes()),
        authenticator_data: b64(&assertion.authenticator_data),
        signature: b64(&assertion.signature),
        user_handle: assertion.user_handle.as_deref().map(b64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str) -> url::Url {
        url.parse().unwrap()
    }

    fn request(rp_id: Option<&str>) -> Request {
        Request {
            challenge: b64(&[7u8; 32]),
            rp_id: rp_id.map(str::to_string),
            allow_credentials: vec![b64(&[1, 2, 3])],
            timeout: Some(60_000),
        }
    }

    const SIGN_IN: &str = "https://accounts.google.com/v3/signin/challenge/pk?TL=abc";

    #[test]
    fn builds_the_client_data_a_browser_would() {
        let checked = check(&page(SIGN_IN), &request(Some("google.com"))).unwrap();
        assert_eq!(
            checked.client_data_json,
            format!(
                r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://accounts.google.com","crossOrigin":false}}"#,
                b64(&[7u8; 32])
            )
        );
        assert_eq!(checked.rp_id, "google.com");
        assert_eq!(checked.allow_list, vec![vec![1, 2, 3]]);
        assert_eq!(checked.timeout, Duration::from_secs(60));
    }

    #[test]
    fn rp_id_defaults_to_the_origin_host() {
        let checked = check(&page(SIGN_IN), &request(None)).unwrap();
        assert_eq!(checked.rp_id, "accounts.google.com");
    }

    #[test]
    fn refuses_other_pages() {
        for url in [
            "https://music.youtube.com/",
            "https://accounts.google.com.evil.com/",
            "http://accounts.google.com/",
            "https://accounts.google.com:8443/",
            "https://myaccount.google.com/",
        ] {
            let err = check(&page(url), &request(None)).unwrap_err();
            assert_eq!(err.name, "SecurityError", "{url}");
        }
    }

    #[test]
    fn refuses_rp_ids_the_origin_may_not_claim() {
        for rp_id in [
            "youtube.com",
            "com",
            "evil.com",
            "oogle.com",
            "mail.google.com",
            "",
        ] {
            let err = check(&page(SIGN_IN), &request(Some(rp_id))).unwrap_err();
            assert_eq!(err.name, "SecurityError", "{rp_id}");
        }
    }

    #[test]
    fn refuses_malformed_requests() {
        let mut short = request(None);
        short.challenge = b64(&[1; 8]);
        assert!(check(&page(SIGN_IN), &short).is_err());

        let mut big = request(None);
        big.challenge = b64(&[7; 6144]);
        assert!(check(&page(SIGN_IN), &big).is_ok());

        let mut huge = request(None);
        huge.challenge = b64(&[7; MAX_CHALLENGE + 1]);
        assert!(check(&page(SIGN_IN), &huge).is_err());

        let mut garbage = request(None);
        garbage.challenge = "not base64!".to_string();
        assert!(check(&page(SIGN_IN), &garbage).is_err());

        let mut many = request(None);
        many.allow_credentials = vec![b64(&[1]); MAX_CREDENTIALS + 1];
        assert!(check(&page(SIGN_IN), &many).is_err());

        let mut empty_id = request(None);
        empty_id.allow_credentials = vec![String::new()];
        assert!(check(&page(SIGN_IN), &empty_id).is_err());
    }

    #[test]
    fn timeout_is_clamped() {
        let mut quick = request(None);
        quick.timeout = Some(1);
        assert_eq!(check(&page(SIGN_IN), &quick).unwrap().timeout, MIN_TIMEOUT);
        let mut forever = request(None);
        forever.timeout = Some(u64::MAX);
        assert_eq!(
            check(&page(SIGN_IN), &forever).unwrap().timeout,
            MAX_TIMEOUT
        );
    }

    #[test]
    fn qr_svg_encodes_the_url() {
        let svg = qr_svg("FIDO:/1234567890").unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("h1v1h-1z"));
        // Only our own markup: nothing from the input reaches the SVG text.
        assert!(!svg.contains("FIDO"));
    }

    #[test]
    fn events_serialise_as_the_page_expects() {
        let json = serde_json::to_string(&Event::Failed {
            failure: Failure::not_allowed("Bluetooth is off"),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"state":"failed","name":"NotAllowedError","message":"Bluetooth is off"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::Pending).unwrap(),
            r#"{"state":"pending"}"#
        );
    }
}
