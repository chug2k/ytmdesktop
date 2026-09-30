//! The WebSocket through the tunnel server to the phone, the caBLE frames
//! inside it, and the one CTAP2 command this app sends: `getAssertion`.

use std::{
    io,
    net::{TcpStream, ToSocketAddrs},
    sync::{atomic::AtomicBool, atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use ring::rand::SystemRandom;
use tungstenite::{
    client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream, Connector,
    HandshakeError, Message, WebSocket,
};

use super::{
    cable::Psk,
    cbor::{self, Value},
    noise::{Crypter, Initiator, KeyPair},
    Failure,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Reads wake up this often to check for cancellation.
const POLL: Duration = Duration::from_millis(500);
/// A CTAP2 message is a few KB; this bounds what a peer can make us buffer.
const MAX_MESSAGE: usize = 64 * 1024;

const FRAME_SHUTDOWN: u8 = 0;
const FRAME_CTAP: u8 = 1;
const CTAP_GET_ASSERTION: u8 = 0x02;

/// What the phone returned for `getAssertion`, before base64 encoding.
pub struct AssertionBytes {
    pub credential_id: Vec<u8>,
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub user_handle: Option<Vec<u8>>,
}

/// The WebSocket before and after encryption, with cancellation.
struct Link<'a> {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    cancel: &'a AtomicBool,
    deadline: Instant,
}

pub struct Tunnel<'a> {
    link: Link<'a>,
    crypter: Crypter,
    /// The phone's `getInfo` says whether it can verify the user itself.
    supports_uv: bool,
}

fn tls_connector() -> Result<Connector, Failure> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| Failure::internal("TLS setup failed"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

fn tcp_connect(host: &str) -> Result<TcpStream, Failure> {
    let addrs = (host, 443)
        .to_socket_addrs()
        .map_err(|_| Failure::network("Could not look up the tunnel server"))?;
    for addr in addrs {
        if let Ok(stream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            return Ok(stream);
        }
    }
    Err(Failure::network("Could not reach the tunnel server"))
}

impl<'a> Tunnel<'a> {
    /// Connect to the phone's tunnel session and run the Noise handshake.
    pub fn open(
        url: &str,
        psk: &Psk,
        identity: KeyPair,
        rng: &SystemRandom,
        cancel: &'a AtomicBool,
        deadline: Instant,
    ) -> Result<Self, Failure> {
        let host = url
            .strip_prefix("wss://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| Failure::internal("bad tunnel URL"))?;
        let stream = tcp_connect(host)?;
        stream
            .set_read_timeout(Some(CONNECT_TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(CONNECT_TIMEOUT)))
            .map_err(|_| Failure::network("socket setup failed"))?;

        let mut request = url
            .into_client_request()
            .map_err(|_| Failure::internal("bad tunnel URL"))?;
        let headers = request.headers_mut();
        headers.insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("fido.cable"),
        );
        headers.insert(
            "Origin",
            HeaderValue::from_str(&format!("wss://{host}"))
                .map_err(|_| Failure::internal("bad tunnel host"))?,
        );

        let ws = match tungstenite::client_tls_with_config(
            request,
            stream,
            None,
            Some(tls_connector()?),
        ) {
            Ok((ws, _)) => ws,
            Err(HandshakeError::Failure(e)) => {
                eprintln!("[YTM Yagami] tunnel WebSocket failed: {e}");
                return Err(Failure::network("The tunnel server refused the connection"));
            }
            Err(HandshakeError::Interrupted(_)) => {
                return Err(Failure::network("The tunnel server did not answer"));
            }
        };
        if let MaybeTlsStream::Rustls(tls) = ws.get_ref() {
            let _ = tls.sock.set_read_timeout(Some(POLL));
        }

        let mut link = Link {
            ws,
            cancel,
            deadline,
        };
        let (initiator, hello) =
            Initiator::start(identity, psk, rng).map_err(|_| Failure::internal("key error"))?;
        link.send(hello)?;
        let reply = link.recv()?;
        let crypter = initiator
            .finish(&reply)
            .map_err(|_| Failure::protocol("The phone's handshake did not verify"))?;
        let mut tunnel = Self {
            link,
            crypter,
            supports_uv: false,
        };

        // The phone speaks first: a CBOR map whose key 1 is its getInfo.
        let hello = tunnel.recv()?;
        let info = cbor::decode(&hello)
            .as_ref()
            .and_then(|m| m.get_int(1))
            .and_then(Value::as_bytes)
            .and_then(cbor::decode)
            .ok_or_else(|| Failure::protocol("The phone sent no getInfo"))?;
        tunnel.supports_uv = info
            .get_int(4)
            .and_then(|options| options.get_text("uv"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(tunnel)
    }

    pub fn get_assertion(
        &mut self,
        rp_id: &str,
        client_data_hash: &[u8],
        allow_list: &[Vec<u8>],
    ) -> Result<AssertionBytes, Failure> {
        let request = get_assertion_request(rp_id, client_data_hash, allow_list, self.supports_uv);
        let mut frame = vec![FRAME_CTAP];
        frame.extend_from_slice(&request);
        self.send(&frame)?;

        let response = loop {
            let frame = self.recv()?;
            match frame.split_first() {
                Some((&FRAME_CTAP, body)) => break body.to_vec(),
                // Linking updates and other frames are not used here.
                Some((&FRAME_SHUTDOWN, _)) | None => {
                    return Err(Failure::protocol("The phone closed the session"))
                }
                Some(_) => continue,
            }
        };
        let result = parse_get_assertion_response(&response, allow_list);
        let _ = self.send(&[FRAME_SHUTDOWN]);
        let _ = self.link.ws.close(None);
        result
    }

    fn send(&mut self, plaintext: &[u8]) -> Result<(), Failure> {
        let ct = self
            .crypter
            .encrypt(plaintext)
            .map_err(|_| Failure::internal("encrypt failed"))?;
        self.link.send(ct)
    }

    fn recv(&mut self) -> Result<Vec<u8>, Failure> {
        let ct = self.link.recv()?;
        self.crypter
            .decrypt(&ct)
            .map_err(|_| Failure::protocol("A message from the phone did not decrypt"))
    }
}

impl Link<'_> {
    fn send(&mut self, data: Vec<u8>) -> Result<(), Failure> {
        self.ws
            .send(Message::Binary(data.into()))
            .map_err(|_| Failure::network("The tunnel connection dropped"))
    }

    fn recv(&mut self) -> Result<Vec<u8>, Failure> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Failure::cancelled());
            }
            if Instant::now() > self.deadline {
                return Err(Failure::timeout());
            }
            match self.ws.read() {
                Ok(Message::Binary(data)) if data.len() <= MAX_MESSAGE => return Ok(data.to_vec()),
                Ok(Message::Binary(_)) => return Err(Failure::protocol("oversized message")),
                Ok(Message::Close(_)) => {
                    return Err(Failure::protocol("The phone closed the session"))
                }
                // Pings are answered by tungstenite on the next write.
                Ok(_) => {
                    let _ = self.ws.flush();
                }
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return Err(Failure::network("The tunnel connection dropped")),
            }
        }
    }
}

/// CTAP2 `authenticatorGetAssertion`, command byte included. Keys are in
/// CTAP2 canonical order.
pub fn get_assertion_request(
    rp_id: &str,
    client_data_hash: &[u8],
    allow_list: &[Vec<u8>],
    uv: bool,
) -> Vec<u8> {
    let mut map = vec![
        (Value::Int(1), Value::text(rp_id)),
        (Value::Int(2), Value::Bytes(client_data_hash.to_vec())),
    ];
    if !allow_list.is_empty() {
        let descriptors = allow_list
            .iter()
            .map(|id| {
                Value::Map(vec![
                    (Value::text("id"), Value::Bytes(id.clone())),
                    (Value::text("type"), Value::text("public-key")),
                ])
            })
            .collect();
        map.push((Value::Int(3), Value::Array(descriptors)));
    }
    if uv {
        map.push((
            Value::Int(5),
            Value::Map(vec![(Value::text("uv"), Value::Bool(true))]),
        ));
    }
    let mut out = vec![CTAP_GET_ASSERTION];
    out.extend_from_slice(&cbor::encode(&Value::Map(map)));
    out
}

/// `body` is the CTAP2 status byte followed by the CBOR response.
pub fn parse_get_assertion_response(
    body: &[u8],
    allow_list: &[Vec<u8>],
) -> Result<AssertionBytes, Failure> {
    let (&status, rest) = body
        .split_first()
        .ok_or_else(|| Failure::protocol("empty response"))?;
    match status {
        0x00 => {}
        // CTAP2_ERR_OPERATION_DENIED, CTAP2_ERR_KEEPALIVE_CANCEL.
        0x27 | 0x2d => return Err(Failure::declined()),
        // CTAP2_ERR_NO_CREDENTIALS.
        0x2e => {
            return Err(Failure::not_allowed(
                "That phone has no passkey for this account",
            ))
        }
        other => {
            eprintln!("[YTM Yagami] phone returned CTAP status {other:#04x}");
            return Err(Failure::not_allowed("The phone could not sign in"));
        }
    }
    let map = cbor::decode(rest).ok_or_else(|| Failure::protocol("bad response"))?;
    let bytes = |key| {
        map.get_int(key)
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
    };

    // The phone may omit the credential when the request allowed exactly one.
    let credential_id = map
        .get_int(1)
        .and_then(|c| c.get_text("id"))
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .or_else(|| match allow_list {
            [only] => Some(only.clone()),
            _ => None,
        })
        .ok_or_else(|| Failure::protocol("response has no credential"))?;
    Ok(AssertionBytes {
        credential_id,
        authenticator_data: bytes(2).ok_or_else(|| Failure::protocol("no authenticatorData"))?,
        signature: bytes(3).ok_or_else(|| Failure::protocol("no signature"))?,
        user_handle: map
            .get_int(4)
            .and_then(|user| user.get_text("id"))
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_assertion_request_is_canonical_ctap2() {
        let request = get_assertion_request("google.com", &[0xab; 32], &[vec![1, 2, 3]], true);
        assert_eq!(request[0], CTAP_GET_ASSERTION);
        let map = cbor::decode(&request[1..]).unwrap();
        assert_eq!(map.get_int(1), Some(&Value::text("google.com")));
        assert_eq!(map.get_int(2), Some(&Value::Bytes(vec![0xab; 32])));
        let Value::Array(allow) = map.get_int(3).unwrap() else {
            panic!("allowList is not an array");
        };
        assert_eq!(allow[0].get_text("id"), Some(&Value::Bytes(vec![1, 2, 3])));
        assert_eq!(allow[0].get_text("type"), Some(&Value::text("public-key")));
        assert_eq!(
            map.get_int(5).unwrap().get_text("uv"),
            Some(&Value::Bool(true))
        );
        // Canonical: map keys ascending, "id" before "type".
        let Value::Map(entries) = map else { panic!() };
        let keys: Vec<_> = entries.iter().map(|(k, _)| k.as_int().unwrap()).collect();
        assert_eq!(keys, [1, 2, 3, 5]);
    }

    #[test]
    fn empty_allow_list_and_no_uv_are_omitted() {
        let request = get_assertion_request("google.com", &[0; 32], &[], false);
        let map = cbor::decode(&request[1..]).unwrap();
        assert!(map.get_int(3).is_none());
        assert!(map.get_int(5).is_none());
    }

    fn response(entries: Vec<(Value, Value)>) -> Vec<u8> {
        let mut body = vec![0x00];
        body.extend(cbor::encode(&Value::Map(entries)));
        body
    }

    #[test]
    fn parses_a_full_response() {
        let body = response(vec![
            (
                Value::Int(1),
                Value::Map(vec![
                    (Value::text("id"), Value::Bytes(vec![7; 16])),
                    (Value::text("type"), Value::text("public-key")),
                ]),
            ),
            (Value::Int(2), Value::Bytes(vec![1; 37])),
            (Value::Int(3), Value::Bytes(vec![2; 70])),
            (
                Value::Int(4),
                Value::Map(vec![(Value::text("id"), Value::Bytes(vec![3; 8]))]),
            ),
        ]);
        let a = parse_get_assertion_response(&body, &[]).unwrap();
        assert_eq!(a.credential_id, vec![7; 16]);
        assert_eq!(a.authenticator_data, vec![1; 37]);
        assert_eq!(a.signature, vec![2; 70]);
        assert_eq!(a.user_handle, Some(vec![3; 8]));
    }

    #[test]
    fn missing_credential_falls_back_to_a_single_allowed_id() {
        let body = response(vec![
            (Value::Int(2), Value::Bytes(vec![1; 37])),
            (Value::Int(3), Value::Bytes(vec![2; 70])),
        ]);
        let a = parse_get_assertion_response(&body, &[vec![9; 4]]).unwrap();
        assert_eq!(a.credential_id, vec![9; 4]);
        assert!(a.user_handle.is_none());
        assert!(parse_get_assertion_response(&body, &[vec![1], vec![2]]).is_err());
    }

    #[test]
    fn ctap_errors_become_failures() {
        assert_eq!(
            parse_get_assertion_response(&[0x27], &[])
                .err()
                .unwrap()
                .name,
            "NotAllowedError"
        );
        assert!(parse_get_assertion_response(&[0x2e], &[]).is_err());
        assert!(parse_get_assertion_response(&[], &[]).is_err());
        assert!(parse_get_assertion_response(&[0x00, 0xff], &[]).is_err());
    }
}
