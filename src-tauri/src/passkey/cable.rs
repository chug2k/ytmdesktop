// SPDX-License-Identifier: MPL-2.0
//! caBLE v2 discovery: the QR code, the tunnel server names, the derived keys,
//! and the BLE advert the phone sends back.
//!
//! Ported from kanidm's webauthn-authenticator-rs 0.5.5 (`src/cable/`,
//! MPL-2.0), with OpenSSL replaced by `ring` and `aes`. That code is itself a
//! port of Chromium's `device/fido/cable/v2_handshake.cc`. The test vectors
//! are kanidm's, captured from Chrome and iOS.

use std::fmt::Write as _;

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use ring::{digest, hkdf, hmac};

use super::cbor::{self, Value};

pub type QrSecret = [u8; 16];
pub type TunnelId = [u8; 16];
pub type Psk = [u8; 32];
type EidKey = [u8; 64];

const QR_PREFIX: &str = "FIDO:/";

/// Tunnel servers with fixed IDs. IDs from 256 up are hashed into a name.
const ASSIGNED_DOMAINS: [&str; 2] = ["cable.ua5v.com", "cable.auth.com"];
const TUNNEL_SERVER_SALT: &[u8] = b"caBLEv2 tunnel server domain\0\0\0";
const TUNNEL_SERVER_TLDS: [&str; 4] = [".com", ".org", ".net", ".info"];
const BASE32_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

/// HKDF `info` values for the keys derived from the QR secret.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Derived {
    EidKey = 1,
    TunnelId = 2,
    Psk = 3,
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-SHA-256. An empty salt is the same as a salt of zeros, as RFC 5869
/// requires.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) {
    let info = [info];
    hkdf::Salt::new(hkdf::HKDF_SHA256, salt)
        .extract(ikm)
        .expand(&info, Len(out.len()))
        .and_then(|okm| okm.fill(out))
        .expect("HKDF output length is far below the 255 * 32 byte limit");
}

fn derive(kind: Derived, secret: &QrSecret, salt: &[u8], out: &mut [u8]) {
    hkdf_sha256(salt, secret, &(kind as u32).to_le_bytes(), out);
}

/// The tunnel server a phone chose, by the ID from its advert.
pub fn tunnel_domain(id: u16) -> Option<String> {
    if id < 256 {
        return ASSIGNED_DOMAINS.get(usize::from(id)).map(|d| d.to_string());
    }
    let mut input = TUNNEL_SERVER_SALT.to_vec();
    let offset = input.len() - 3;
    input[offset..offset + 2].copy_from_slice(&id.to_le_bytes());
    let hash = digest::digest(&digest::SHA256, &input);
    let mut bits = u64::from_le_bytes(hash.as_ref()[..8].try_into().ok()?);
    let tld = TUNNEL_SERVER_TLDS[(bits & 3) as usize];
    bits >>= 2;
    let mut name = String::from("cable.");
    while bits != 0 {
        name.push(char::from(BASE32_CHARS[(bits & 31) as usize]));
        bits >>= 5;
    }
    name.push_str(tld);
    Some(name)
}

/// Chromium's `BytesToDigits`: each 7-byte chunk becomes 17 decimal digits,
/// so the QR code can use its dense numeric mode.
pub fn base10_encode(input: &[u8]) -> String {
    input.chunks(7).fold(String::new(), |mut out, chunk| {
        let width = [0, 3, 5, 8, 10, 13, 15, 17][chunk.len()];
        let mut padded = [0u8; 8];
        padded[..chunk.len()].copy_from_slice(chunk);
        let _ = write!(out, "{:0width$}", u64::from_le_bytes(padded));
        out
    })
}

/// Chromium's `DigitsToBytes`. `None` for anything that `base10_encode`
/// could not have produced. Only the tests read QR codes.
#[cfg(test)]
pub fn base10_decode(input: &str) -> Option<Vec<u8>> {
    if !input.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut out = Vec::new();
    for chunk in input.as_bytes().chunks(17) {
        let bytes = match chunk.len() {
            17 => 7,
            15 => 6,
            13 => 5,
            10 => 4,
            8 => 3,
            5 => 2,
            3 => 1,
            _ => return None,
        };
        let value: u64 = std::str::from_utf8(chunk).ok()?.parse().ok()?;
        if value >> (bytes * 8) != 0 {
            return None;
        }
        out.extend_from_slice(&value.to_le_bytes()[..bytes]);
    }
    Some(out)
}

/// The contents of the `FIDO:/` QR code that the phone scans.
#[derive(Debug, Clone, PartialEq)]
pub struct QrPayload {
    /// Compressed P-256 public key of this session's identity key.
    pub identity_public: [u8; 33],
    pub secret: QrSecret,
    pub known_domains: i64,
    pub timestamp: i64,
    /// Present means caBLE v2.1. Safari always sends it; Chrome omits it
    /// when false.
    pub supports_linking: Option<bool>,
    pub request_type: String,
}

impl QrPayload {
    pub fn get_assertion(identity_public: [u8; 33], secret: QrSecret, timestamp: i64) -> Self {
        Self {
            identity_public,
            secret,
            known_domains: ASSIGNED_DOMAINS.len() as i64,
            timestamp,
            supports_linking: Some(false),
            request_type: "ga".to_string(),
        }
    }

    pub fn to_url(&self) -> String {
        let mut map = vec![
            (Value::Int(0), Value::Bytes(self.identity_public.to_vec())),
            (Value::Int(1), Value::Bytes(self.secret.to_vec())),
            (Value::Int(2), Value::Int(self.known_domains)),
            (Value::Int(3), Value::Int(self.timestamp)),
        ];
        if let Some(linking) = self.supports_linking {
            map.push((Value::Int(4), Value::Bool(linking)));
        }
        map.push((Value::Int(5), Value::text(&self.request_type)));
        format!(
            "{QR_PREFIX}{}",
            base10_encode(&cbor::encode(&Value::Map(map)))
        )
    }

    #[cfg(test)]
    fn from_url(url: &str) -> Option<Self> {
        let payload = base10_decode(url.strip_prefix(QR_PREFIX)?)?;
        let map = cbor::decode(&payload)?;
        Some(Self {
            identity_public: map.get_int(0)?.as_bytes()?.try_into().ok()?,
            secret: map.get_int(1)?.as_bytes()?.try_into().ok()?,
            known_domains: map.get_int(2)?.as_int()?,
            timestamp: map.get_int(3)?.as_int()?,
            supports_linking: map.get_int(4).and_then(Value::as_bool),
            request_type: match map.get_int(5)? {
                Value::Text(t) => t.clone(),
                _ => return None,
            },
        })
    }
}

/// What the phone put in its BLE advert: where to find it on the tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eid {
    pub tunnel_server_id: u16,
    pub routing_id: [u8; 3],
    pub nonce: [u8; 10],
}

impl Eid {
    fn to_bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[1..11].copy_from_slice(&self.nonce);
        out[11..14].copy_from_slice(&self.routing_id);
        out[14..16].copy_from_slice(&self.tunnel_server_id.to_le_bytes());
        out
    }

    fn from_bytes(bytes: &[u8; 16]) -> Option<Self> {
        // The first byte is reserved and always zero.
        if bytes[0] != 0 {
            return None;
        }
        let eid = Self {
            nonce: bytes[1..11].try_into().ok()?,
            routing_id: bytes[11..14].try_into().ok()?,
            tunnel_server_id: u16::from_le_bytes(bytes[14..16].try_into().ok()?),
        };
        // An unknown tunnel server is a parse failure.
        tunnel_domain(eid.tunnel_server_id).map(|_| eid)
    }

    pub fn connect_url(&self, tunnel_id: &TunnelId) -> Option<String> {
        let domain = tunnel_domain(self.tunnel_server_id)?;
        Some(format!(
            "wss://{domain}/cable/connect/{}/{}",
            hex_upper(&self.routing_id),
            hex_upper(tunnel_id)
        ))
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02X}");
        out
    })
}

/// The keys one QR code session derives from its secret.
pub struct Discovery {
    secret: QrSecret,
    eid_key: EidKey,
}

impl Discovery {
    pub fn new(secret: QrSecret) -> Self {
        let mut eid_key = [0u8; 64];
        derive(Derived::EidKey, &secret, &[], &mut eid_key);
        Self { secret, eid_key }
    }

    pub fn tunnel_id(&self) -> TunnelId {
        let mut id = [0u8; 16];
        derive(Derived::TunnelId, &self.secret, &[], &mut id);
        id
    }

    pub fn psk(&self, eid: &Eid) -> Psk {
        let mut psk = [0u8; 32];
        derive(Derived::Psk, &self.secret, &eid.to_bytes(), &mut psk);
        psk
    }

    /// Decrypt a caBLE service-data advert. `None` for adverts from other
    /// sessions, other devices, or anything malformed: the scanner sees every
    /// caBLE advert in radio range.
    pub fn decrypt_advert(&self, advert: &[u8]) -> Option<Eid> {
        let advert: &[u8; 20] = advert.try_into().ok()?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, &self.eid_key[32..]);
        let tag = hmac::sign(&key, &advert[..16]);
        let mismatch = tag.as_ref()[..4]
            .iter()
            .zip(&advert[16..])
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if mismatch != 0 {
            return None;
        }
        // One AES-256 block. Chromium calls this CBC with a zero IV, which is
        // the same thing for a single block.
        let cipher = aes::Aes256::new(GenericArray::from_slice(&self.eid_key[..32]));
        let mut block = GenericArray::clone_from_slice(&advert[..16]);
        cipher.decrypt_block(&mut block);
        Eid::from_bytes(block.as_slice().try_into().ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base10_matches_chromium() {
        assert_eq!(base10_encode(&[0x61, 0x62, 0xff]), "16736865");
        assert_eq!(base10_decode("16736865").unwrap(), [0x61, 0x62, 0xff]);
        for (bytes, digits) in [(0, 0), (1, 3), (2, 5), (3, 8), (6, 15), (7, 17), (8, 20)] {
            assert_eq!(base10_encode(&vec![0; bytes]), "0".repeat(digits));
        }
        let all: Vec<u8> = (0..=255).collect();
        for len in 0..all.len() {
            assert_eq!(
                base10_decode(&base10_encode(&all[..len])).unwrap(),
                &all[..len]
            );
        }
    }

    #[test]
    fn base10_rejects_what_it_cannot_produce() {
        for bad in ["abc", "\u{ff11}\u{ff12}\u{ff13}", "1", "12", "1234", "999"] {
            assert_eq!(base10_decode(bad), None, "{bad:?}");
        }
        assert_eq!(base10_decode("99999999999999999"), None);
    }

    #[test]
    fn qr_urls_from_chrome_and_ios_round_trip() {
        let chrome = "FIDO:/162870791865632382552704231438327900152302540348097243854039966655366469794954476199158014113179232779520163209900691930075274801398564434658077048963842109321447142660";
        let ios = "FIDO:/089962132878132862898875319509818655951233947060166026934941652203853844930597225184066237811614893181300344014421205790072080843938838513707157859599106109321447142404";
        for url in [chrome, ios] {
            let payload = QrPayload::from_url(url).unwrap();
            assert_eq!(payload.to_url(), url);
        }
    }

    #[test]
    fn our_qr_payload_is_v2_1_get_assertion() {
        let payload = QrPayload::get_assertion([2; 33], [7; 16], 1_700_000_000);
        let parsed = QrPayload::from_url(&payload.to_url()).unwrap();
        assert_eq!(parsed, payload);
        assert_eq!(parsed.request_type, "ga");
        assert_eq!(parsed.supports_linking, Some(false));
        assert_eq!(parsed.known_domains, 2);
    }

    #[test]
    fn tunnel_domains() {
        assert_eq!(tunnel_domain(0).unwrap(), "cable.ua5v.com");
        assert_eq!(tunnel_domain(1).unwrap(), "cable.auth.com");
        assert_eq!(tunnel_domain(2), None);
        assert_eq!(tunnel_domain(255), None);
        assert_eq!(tunnel_domain(266).unwrap(), "cable.wufkweyy3uaxb.com");
        assert_eq!(tunnel_domain(0xf980).unwrap(), "cable.my4kstlhndi4c.net");
    }

    #[test]
    fn derives_the_tunnel_id_chromium_derives() {
        let discovery = Discovery::new([0; 16]);
        assert_eq!(
            hex_upper(&discovery.tunnel_id()),
            "3EEF97097986413B059EAA2A30D653D4"
        );
    }

    #[test]
    fn decrypts_a_captured_advert() {
        let discovery = Discovery::new([
            1, 254, 166, 247, 196, 128, 116, 147, 220, 37, 111, 158, 172, 247, 86, 201,
        ]);
        let advert = [
            2, 125, 132, 237, 96, 118, 181, 94, 36, 124, 131, 15, 130, 149, 94, 77, 18, 110, 127,
            67,
        ];
        let eid = discovery.decrypt_advert(&advert).unwrap();
        assert_eq!(
            eid,
            Eid {
                tunnel_server_id: 0,
                routing_id: [2, 101, 85],
                nonce: [139, 181, 197, 201, 164, 77, 145, 58, 94, 178],
            }
        );
        assert_eq!(
            eid.connect_url(&discovery.tunnel_id()).unwrap(),
            "wss://cable.ua5v.com/cable/connect/026555/367CBBF5F5085DF4098476AFE4B9B1D2"
        );
    }

    #[test]
    fn ignores_adverts_it_cannot_authenticate() {
        let discovery = Discovery::new([
            1, 254, 166, 247, 196, 128, 116, 147, 220, 37, 111, 158, 172, 247, 86, 201,
        ]);
        let advert = [
            2, 125, 132, 237, 96, 118, 181, 94, 36, 124, 131, 15, 130, 149, 94, 77, 18, 110, 127,
            67,
        ];
        let mut body = advert;
        body[0] ^= 1;
        assert_eq!(discovery.decrypt_advert(&body), None);
        let mut tag = advert;
        tag[16] ^= 1;
        assert_eq!(discovery.decrypt_advert(&tag), None);
        for len in 0..advert.len() {
            assert_eq!(discovery.decrypt_advert(&advert[..len]), None);
        }
        assert_eq!(Discovery::new([0; 16]).decrypt_advert(&advert), None);
    }

    #[test]
    fn eid_parsing() {
        let bytes = [
            0, 9, 139, 115, 107, 54, 169, 140, 185, 164, 47, 9, 10, 11, 255, 1,
        ];
        let eid = Eid::from_bytes(&bytes).unwrap();
        assert_eq!(eid.tunnel_server_id, 0x01ff);
        assert_eq!(eid.routing_id, [9, 10, 11]);
        assert_eq!(eid.to_bytes(), bytes);
        let mut reserved = bytes;
        reserved[0] = 1;
        assert_eq!(Eid::from_bytes(&reserved), None);
        let mut unknown_server = bytes;
        unknown_server[15] = 0;
        assert_eq!(Eid::from_bytes(&unknown_server), None);
    }
}
