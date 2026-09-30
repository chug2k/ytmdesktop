// SPDX-License-Identifier: MPL-2.0
//! caBLE's variant of the Noise `KNpsk0` handshake, and the encrypted channel
//! it produces. The tunnel server relays these messages but cannot read them.
//!
//! Ported from kanidm's webauthn-authenticator-rs 0.5.5 (`src/cable/noise.rs`,
//! MPL-2.0), with OpenSSL replaced by `ring`. Only the initiator (desktop)
//! side is here; the tests carry a responder to check it against.

use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    agreement::{self, EphemeralPrivateKey, UnparsedPublicKey},
    digest,
    rand::SystemRandom,
};

use super::cable::{hkdf_sha256, Psk};

const KN_PROTOCOL: &[u8; 32] = b"Noise_KNpsk0_P256_AESGCM_SHA256\0";
const PADDING: usize = 32;
const P256_POINT_LEN: usize = 65;
const TAG_LEN: usize = 16;

#[derive(Debug)]
pub enum NoiseError {
    Crypto,
    BadMessage,
}

impl From<ring::error::Unspecified> for NoiseError {
    fn from(_: ring::error::Unspecified) -> Self {
        NoiseError::Crypto
    }
}

/// caBLE uses three nonce layouts. `Old` and `New` are the two transport
/// versions; `Handshake` is only used inside the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NonceType {
    /// Little-endian counter in the first four bytes, AAD of `[2]`.
    Old,
    /// Noise-standard big-endian counter in the last four bytes, no AAD.
    New,
    /// Big-endian counter in the first four bytes.
    Handshake,
}

struct CipherState {
    key: Option<[u8; 32]>,
    n: u32,
    nonce_type: NonceType,
    padding: bool,
}

impl CipherState {
    fn new(nonce_type: NonceType, padding: bool) -> Self {
        Self {
            key: None,
            n: 0,
            nonce_type,
            padding,
        }
    }

    fn init_key(&mut self, key: [u8; 32]) {
        self.key = Some(key);
        self.n = 0;
    }

    fn nonce(&self) -> Nonce {
        let mut nonce = [0u8; 12];
        match self.nonce_type {
            NonceType::Old => nonce[..4].copy_from_slice(&self.n.to_le_bytes()),
            NonceType::New => nonce[8..].copy_from_slice(&self.n.to_be_bytes()),
            NonceType::Handshake => nonce[..4].copy_from_slice(&self.n.to_be_bytes()),
        }
        Nonce::assume_unique_for_key(nonce)
    }

    fn default_aad(&self) -> &'static [u8] {
        if self.nonce_type == NonceType::Old {
            &[2]
        } else {
            &[]
        }
    }

    fn aead(key: &[u8; 32]) -> Result<LessSafeKey, NoiseError> {
        Ok(LessSafeKey::new(UnboundKey::new(&aead::AES_256_GCM, key)?))
    }

    fn encrypt(&mut self, plaintext: &[u8], aad: Option<&[u8]>) -> Result<Vec<u8>, NoiseError> {
        let Some(key) = self.key else {
            return Ok(plaintext.to_vec());
        };
        let n = self.n.checked_add(1).ok_or(NoiseError::Crypto)?;
        let mut buf = if self.padding {
            pad(plaintext)
        } else {
            plaintext.to_vec()
        };
        let aad = aad.unwrap_or(self.default_aad());
        Self::aead(&key)?.seal_in_place_append_tag(self.nonce(), Aad::from(aad), &mut buf)?;
        self.n = n;
        Ok(buf)
    }

    fn decrypt(&mut self, ciphertext: &[u8], aad: Option<&[u8]>) -> Result<Vec<u8>, NoiseError> {
        let Some(key) = self.key else {
            return Ok(ciphertext.to_vec());
        };
        let n = self.n.checked_add(1).ok_or(NoiseError::Crypto)?;
        let mut buf = ciphertext.to_vec();
        let opened = Self::aead(&key)?
            .open_in_place(
                self.nonce(),
                Aad::from(aad.unwrap_or(self.default_aad())),
                &mut buf,
            )
            .map(|pt| pt.len());
        let len = match opened {
            Ok(len) => len,
            // The first transport message tells us which version the phone
            // speaks: if the old layout fails, try the new one once.
            Err(_) if self.nonce_type == NonceType::Old && self.n == 0 && aad.is_none() => {
                self.nonce_type = NonceType::New;
                return self.decrypt(ciphertext, None);
            }
            Err(e) => return Err(e.into()),
        };
        buf.truncate(len);
        self.n = n;
        if self.padding {
            unpad(&mut buf)?;
        }
        Ok(buf)
    }
}

/// Pad to a multiple of 32 bytes. The last byte counts the zero bytes
/// before it.
fn pad(msg: &[u8]) -> Vec<u8> {
    let padded_len = (msg.len() + PADDING) & !(PADDING - 1);
    let mut out = vec![0u8; padded_len];
    out[..msg.len()].copy_from_slice(msg);
    out[padded_len - 1] = (padded_len - msg.len() - 1) as u8;
    out
}

fn unpad(msg: &mut Vec<u8>) -> Result<(), NoiseError> {
    let padding = usize::from(*msg.last().ok_or(NoiseError::BadMessage)?) + 1;
    if padding > msg.len() {
        return Err(NoiseError::BadMessage);
    }
    msg.truncate(msg.len() - padding);
    Ok(())
}

/// The encrypted channel after the handshake: one key for each direction.
pub struct Crypter {
    reader: CipherState,
    writer: CipherState,
}

impl Crypter {
    fn new(read_key: [u8; 32], write_key: [u8; 32]) -> Self {
        let mut reader = CipherState::new(NonceType::Old, true);
        reader.init_key(read_key);
        let mut writer = CipherState::new(NonceType::Old, true);
        writer.init_key(write_key);
        Self { reader, writer }
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        self.writer.encrypt(plaintext, None)
    }

    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let plaintext = self.reader.decrypt(ciphertext, None)?;
        self.writer.nonce_type = self.reader.nonce_type;
        Ok(plaintext)
    }
}

/// Noise `SymmetricState`.
struct Symmetric {
    ck: [u8; 32],
    h: [u8; 32],
    cipher: CipherState,
}

impl Symmetric {
    fn new() -> Self {
        Self {
            ck: *KN_PROTOCOL,
            h: *KN_PROTOCOL,
            cipher: CipherState::new(NonceType::Handshake, false),
        }
    }

    fn mix_hash(&mut self, data: &[u8]) {
        let mut ctx = digest::Context::new(&digest::SHA256);
        ctx.update(&self.h);
        ctx.update(data);
        self.h.copy_from_slice(ctx.finish().as_ref());
    }

    fn mix_key(&mut self, ikm: &[u8]) {
        let mut out = [0u8; 64];
        hkdf_sha256(&self.ck, ikm, &[], &mut out);
        self.ck.copy_from_slice(&out[..32]);
        self.cipher
            .init_key(out[32..].try_into().expect("32-byte half"));
    }

    fn mix_key_and_hash(&mut self, ikm: &[u8]) {
        let mut out = [0u8; 96];
        hkdf_sha256(&self.ck, ikm, &[], &mut out);
        self.ck.copy_from_slice(&out[..32]);
        self.mix_hash(&out[32..64]);
        self.cipher
            .init_key(out[64..].try_into().expect("32-byte third"));
    }

    fn encrypt_and_hash(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let h = self.h;
        let ct = self.cipher.encrypt(plaintext, Some(&h))?;
        self.mix_hash(&ct);
        Ok(ct)
    }

    fn decrypt_and_hash(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let h = self.h;
        let pt = self.cipher.decrypt(ciphertext, Some(&h))?;
        self.mix_hash(ciphertext);
        Ok(pt)
    }

    /// Noise `Split`: the first key encrypts initiator messages, the second
    /// encrypts responder messages.
    fn split(&self) -> ([u8; 32], [u8; 32]) {
        let mut out = [0u8; 64];
        hkdf_sha256(&self.ck, &[], &[], &mut out);
        (
            out[..32].try_into().expect("32-byte half"),
            out[32..].try_into().expect("32-byte half"),
        )
    }
}

fn ecdh(private: EphemeralPrivateKey, peer: &[u8]) -> Result<[u8; 32], NoiseError> {
    let peer = UnparsedPublicKey::new(&agreement::ECDH_P256, peer);
    agreement::agree_ephemeral(private, &peer, |shared| {
        let mut out = [0u8; 32];
        out.copy_from_slice(shared);
        out
    })
    .map_err(|_| NoiseError::BadMessage)
}

/// A P-256 key pair. `ring` lets each private key do exactly one ECDH, which
/// is exactly how often caBLE uses each key.
pub struct KeyPair {
    private: EphemeralPrivateKey,
    public: [u8; P256_POINT_LEN],
}

impl KeyPair {
    pub fn generate(rng: &SystemRandom) -> Result<Self, NoiseError> {
        let private = EphemeralPrivateKey::generate(&agreement::ECDH_P256, rng)?;
        let public = private
            .compute_public_key()?
            .as_ref()
            .try_into()
            .map_err(|_| NoiseError::Crypto)?;
        Ok(Self { private, public })
    }

    /// SEC1 compressed form, as the QR code carries it.
    pub fn public_compressed(&self) -> [u8; 33] {
        let mut out = [0u8; 33];
        out[0] = 2 | (self.public[64] & 1);
        out[1..].copy_from_slice(&self.public[1..33]);
        out
    }
}

/// The desktop side of the handshake, between sending the first message and
/// reading the phone's reply.
pub struct Initiator {
    state: Symmetric,
    ephemeral: EphemeralPrivateKey,
    identity: EphemeralPrivateKey,
}

impl Initiator {
    /// `KNpsk0` with the identity key the QR code published. Returns the
    /// state and the first handshake message (ephemeral point || tag).
    pub fn start(
        identity: KeyPair,
        psk: &Psk,
        rng: &SystemRandom,
    ) -> Result<(Self, Vec<u8>), NoiseError> {
        let mut state = Symmetric::new();
        state.mix_hash(&[1]);
        state.mix_hash(&identity.public);
        state.mix_key_and_hash(psk);

        let ephemeral = KeyPair::generate(rng)?;
        state.mix_hash(&ephemeral.public);
        state.mix_key(&ephemeral.public);
        let tag = state.encrypt_and_hash(&[])?;

        let mut message = ephemeral.public.to_vec();
        message.extend_from_slice(&tag);
        Ok((
            Self {
                state,
                ephemeral: ephemeral.private,
                identity: identity.private,
            },
            message,
        ))
    }

    pub fn finish(mut self, response: &[u8]) -> Result<Crypter, NoiseError> {
        if response.len() != P256_POINT_LEN + TAG_LEN {
            return Err(NoiseError::BadMessage);
        }
        let (peer, tag) = response.split_at(P256_POINT_LEN);
        let ee = ecdh(self.ephemeral, peer)?;
        self.state.mix_hash(peer);
        self.state.mix_key(peer);
        self.state.mix_key(&ee);
        let se = ecdh(self.identity, peer)?;
        self.state.mix_key(&se);
        if !self.state.decrypt_and_hash(tag)?.is_empty() {
            return Err(NoiseError::BadMessage);
        }
        let (write_key, read_key) = self.state.split();
        Ok(Crypter::new(read_key, write_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phone's side of `KNpsk0`. It uses `p256` because the responder
    /// does two ECDH operations with one ephemeral key, which `ring` forbids.
    fn respond(identity_public: &[u8; 65], psk: &Psk, message: &[u8]) -> (Crypter, Vec<u8>) {
        use p256::{ecdh::diffie_hellman, elliptic_curve::sec1::ToEncodedPoint, PublicKey};

        let (peer, tag) = message.split_at(P256_POINT_LEN);
        let mut state = Symmetric::new();
        state.mix_hash(&[1]);
        state.mix_hash(identity_public);
        state.mix_key_and_hash(psk);
        state.mix_hash(peer);
        state.mix_key(peer);
        assert!(state.decrypt_and_hash(tag).unwrap().is_empty());

        let ephemeral = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let public = ephemeral.public_key().to_encoded_point(false);
        let dh = |point: &[u8]| {
            let point = PublicKey::from_sec1_bytes(point).unwrap();
            let shared = diffie_hellman(ephemeral.to_nonzero_scalar(), point.as_affine());
            shared.raw_secret_bytes().to_vec()
        };
        state.mix_hash(public.as_bytes());
        state.mix_key(public.as_bytes());
        state.mix_key(&dh(peer));
        state.mix_key(&dh(identity_public));
        let tag = state.encrypt_and_hash(&[]).unwrap();

        let mut response = public.as_bytes().to_vec();
        response.extend_from_slice(&tag);
        let (initiator_key, responder_key) = state.split();
        (Crypter::new(initiator_key, responder_key), response)
    }

    #[test]
    fn handshake_agrees_on_keys_both_ways() {
        let rng = SystemRandom::new();
        let psk = [9u8; 32];
        let identity = KeyPair::generate(&rng).unwrap();
        let identity_public = identity.public;
        let (initiator, message) = Initiator::start(identity, &psk, &rng).unwrap();
        assert_eq!(message.len(), 81);

        let (mut phone, response) = respond(&identity_public, &psk, &message);
        let mut desktop = initiator.finish(&response).unwrap();

        let ct = phone.encrypt(b"hello from the phone").unwrap();
        assert_eq!(desktop.decrypt(&ct).unwrap(), b"hello from the phone");
        let ct = desktop.encrypt(b"getAssertion").unwrap();
        assert_eq!(phone.decrypt(&ct).unwrap(), b"getAssertion");
    }

    #[test]
    fn handshake_fails_with_the_wrong_psk() {
        let rng = SystemRandom::new();
        let identity = KeyPair::generate(&rng).unwrap();
        let identity_public = identity.public;
        let (initiator, message) = Initiator::start(identity, &[1; 32], &rng).unwrap();
        let result = std::panic::catch_unwind(|| respond(&identity_public, &[2; 32], &message));
        assert!(
            result.is_err(),
            "phone accepted a handshake with the wrong psk"
        );
        // And a response the phone did not make is refused.
        let mut forged = message.clone();
        forged.truncate(81);
        assert!(initiator.finish(&forged).is_err());
    }

    #[test]
    fn compressed_point_matches_p256() {
        use p256::{elliptic_curve::sec1::ToEncodedPoint, PublicKey};
        let pair = KeyPair::generate(&SystemRandom::new()).unwrap();
        let expected = PublicKey::from_sec1_bytes(&pair.public)
            .unwrap()
            .to_encoded_point(true);
        assert_eq!(pair.public_compressed().as_slice(), expected.as_bytes());
    }

    #[test]
    fn padding_round_trips() {
        for len in 0..100 {
            let msg = vec![0xab; len];
            let mut padded = pad(&msg);
            assert_eq!(padded.len() % PADDING, 0);
            assert!(padded.len() > len);
            unpad(&mut padded).unwrap();
            assert_eq!(padded, msg);
        }
        assert!(unpad(&mut vec![]).is_err());
        assert!(unpad(&mut vec![5]).is_err());
    }

    #[test]
    fn crypter_detects_new_construction_from_first_message() {
        let mut phone = Crypter::new([1; 32], [2; 32]);
        phone.writer.nonce_type = NonceType::New;
        phone.reader.nonce_type = NonceType::New;
        let mut desktop = Crypter::new([2; 32], [1; 32]);

        let ct = phone.encrypt(b"post-handshake").unwrap();
        assert_eq!(desktop.decrypt(&ct).unwrap(), b"post-handshake");
        assert_eq!(desktop.writer.nonce_type, NonceType::New);
        // A replay fails: the counter moved on.
        assert!(desktop.decrypt(&ct).is_err());

        let reply = desktop.encrypt(b"request").unwrap();
        assert_eq!(phone.decrypt(&reply).unwrap(), b"request");
    }

    #[test]
    fn crypter_old_construction_round_trips_and_rejects_tampering() {
        let mut alice = Crypter::new([123; 32], [231; 32]);
        let mut bob = Crypter::new([231; 32], [123; 32]);
        for len in 0..80 {
            let msg = vec![0xff; len];
            let ct = alice.encrypt(&msg).unwrap();
            let mut tampered = ct.clone();
            tampered[len % ct.len()] ^= 1;
            let mut shadow = Crypter::new([231; 32], [123; 32]);
            shadow.reader.n = bob.reader.n;
            assert!(shadow.decrypt(&tampered).is_err());
            assert_eq!(bob.decrypt(&ct).unwrap(), msg);
            assert_eq!(bob.reader.nonce_type, NonceType::Old);
        }
    }

    /// Keys and ciphertext leaked from a patched Chromium, via kanidm.
    #[test]
    fn crypter_matches_chromium_new_construction() {
        let write_key = [
            0x1f, 0xba, 0x3c, 0xce, 0x17, 0x62, 0x2c, 0x68, 0x26, 0x8d, 0x9f, 0x75, 0xb5, 0xa8,
            0xa3, 0x35, 0x1b, 0x51, 0x7f, 0x9, 0x6f, 0xb5, 0xe2, 0x94, 0x94, 0x1a, 0xf7, 0xe3,
            0xa6, 0xa8, 0xd6, 0xe1,
        ];
        let read_key = [
            0xe3, 0x4f, 0x1a, 0xa3, 0x74, 0x72, 0x38, 0xc0, 0x4d, 0x3b, 0xd2, 0x5e, 0x7, 0xef,
            0x1b, 0x35, 0xfe, 0xf3, 0x59, 0x0, 0xd, 0x75, 0x56, 0x15, 0xcd, 0x85, 0xbe, 0x27, 0xcf,
            0xc8, 0x7, 0xd1,
        ];
        let plaintext = CHROMIUM_FRAME;
        let mut crypter = Crypter::new(read_key, write_key);
        crypter.writer.nonce_type = NonceType::New;
        crypter.reader.nonce_type = NonceType::New;
        let ct = crypter.encrypt(plaintext).unwrap();
        assert_eq!(ct, CHROMIUM_CIPHERTEXT);

        let mut peer = Crypter::new(write_key, read_key);
        assert_eq!(peer.decrypt(&ct).unwrap(), plaintext);
    }

    const CHROMIUM_FRAME: &[u8] = &[
        0x1, 0x1, 0xa6, 0x1, 0x58, 0x20, 0x38, 0x89, 0x28, 0x5c, 0x8c, 0x63, 0x23, 0x95, 0xc, 0xed,
        0x7, 0x49, 0x84, 0xf9, 0xf9, 0x46, 0x3b, 0xc1, 0x73, 0x9b, 0xb6, 0x21, 0xa9, 0xe5, 0xf1,
        0xee, 0x8d, 0xd9, 0x39, 0x3b, 0xa2, 0x80, 0x2, 0xa2, 0x62, 0x69, 0x64, 0x78, 0x18, 0x77,
        0x65, 0x62, 0x61, 0x75, 0x74, 0x68, 0x6e, 0x2e, 0x66, 0x69, 0x72, 0x73, 0x74, 0x79, 0x65,
        0x61, 0x72, 0x2e, 0x69, 0x64, 0x2e, 0x61, 0x75, 0x64, 0x6e, 0x61, 0x6d, 0x65, 0x78, 0x18,
        0x77, 0x65, 0x62, 0x61, 0x75, 0x74, 0x68, 0x6e, 0x2e, 0x66, 0x69, 0x72, 0x73, 0x74, 0x79,
        0x65, 0x61, 0x72, 0x2e, 0x69, 0x64, 0x2e, 0x61, 0x75, 0x3, 0xa3, 0x62, 0x69, 0x64, 0x50,
        0xd6, 0xd7, 0xaa, 0x29, 0x8f, 0xe8, 0x4a, 0x6, 0xaa, 0xde, 0xd7, 0xe4, 0x9d, 0x90, 0xa,
        0x62, 0x64, 0x6e, 0x61, 0x6d, 0x65, 0x61, 0x61, 0x6b, 0x64, 0x69, 0x73, 0x70, 0x6c, 0x61,
        0x79, 0x4e, 0x61, 0x6d, 0x65, 0x61, 0x61, 0x4, 0x82, 0xa2, 0x63, 0x61, 0x6c, 0x67, 0x26,
        0x64, 0x74, 0x79, 0x70, 0x65, 0x6a, 0x70, 0x75, 0x62, 0x6c, 0x69, 0x63, 0x2d, 0x6b, 0x65,
        0x79, 0xa2, 0x63, 0x61, 0x6c, 0x67, 0x39, 0x1, 0x0, 0x64, 0x74, 0x79, 0x70, 0x65, 0x6a,
        0x70, 0x75, 0x62, 0x6c, 0x69, 0x63, 0x2d, 0x6b, 0x65, 0x79, 0x5, 0x81, 0xa2, 0x62, 0x69,
        0x64, 0x44, 0x0, 0x1, 0x2, 0x3, 0x64, 0x74, 0x79, 0x70, 0x65, 0x6a, 0x70, 0x75, 0x62, 0x6c,
        0x69, 0x63, 0x2d, 0x6b, 0x65, 0x79, 0x7, 0xa1, 0x62, 0x75, 0x76, 0xf5,
    ];

    const CHROMIUM_CIPHERTEXT: &[u8] = &[
        0x50, 0x62, 0xcf, 0x34, 0x57, 0x1e, 0x8, 0x27, 0xa7, 0xc0, 0x20, 0x7f, 0x7c, 0x0, 0x18,
        0x45, 0x67, 0xd6, 0x13, 0xea, 0xb0, 0xda, 0x8, 0xa, 0xd0, 0x42, 0xd8, 0x6, 0x64, 0xc5,
        0x9d, 0xf7, 0xb0, 0x1a, 0x13, 0xdb, 0x17, 0xfd, 0x27, 0x75, 0x75, 0xcc, 0xff, 0x53, 0xb6,
        0xa3, 0x4f, 0xdb, 0x4f, 0xbc, 0xf8, 0x32, 0xf3, 0xd3, 0x60, 0xf8, 0xe5, 0xa7, 0xda, 0xee,
        0x7f, 0x26, 0x5a, 0x92, 0x53, 0xa9, 0x4, 0xd6, 0xeb, 0xff, 0x2f, 0x93, 0x70, 0xd3, 0x55,
        0x36, 0xd8, 0xbf, 0x5, 0x48, 0x30, 0xaa, 0xad, 0xff, 0xb9, 0x96, 0xb4, 0x20, 0xb2, 0xb3,
        0x17, 0xa, 0xc8, 0xa, 0x83, 0x79, 0x68, 0x23, 0xed, 0x3c, 0x28, 0x4b, 0x17, 0x7c, 0x23,
        0x40, 0xc, 0xa0, 0x12, 0x4d, 0x6a, 0x68, 0x26, 0x3d, 0x39, 0x78, 0x3c, 0xfe, 0xf0, 0x27,
        0x3f, 0xdf, 0x3b, 0xfc, 0xfa, 0xa, 0x6c, 0x33, 0xdf, 0x31, 0x9b, 0x12, 0x6f, 0x6e, 0x82,
        0x90, 0xd2, 0x2c, 0x4c, 0xd3, 0x2a, 0x7a, 0x97, 0x88, 0x56, 0xba, 0x22, 0x73, 0xd1, 0xbe,
        0x1c, 0xa, 0x29, 0x1e, 0x5e, 0xe1, 0x97, 0x41, 0x6a, 0xa0, 0xf7, 0xa1, 0x4, 0xe4, 0xd0,
        0xac, 0x58, 0x2b, 0x70, 0x84, 0x82, 0x32, 0x6d, 0x5f, 0xf0, 0xf1, 0x76, 0x8c, 0x14, 0x16,
        0xd0, 0x16, 0xb1, 0xf8, 0x92, 0x42, 0xe7, 0xe, 0x80, 0x31, 0x2f, 0xe6, 0xb6, 0xd4, 0x2,
        0x9a, 0x40, 0xad, 0xa3, 0x74, 0xb3, 0x1e, 0x7d, 0x66, 0xfa, 0xc3, 0xba, 0x72, 0x83, 0x94,
        0x4b, 0x9b, 0x60, 0xda, 0x4b, 0x98, 0xf6, 0x78, 0x4, 0x9, 0x5f, 0xd3, 0x9c, 0xd1, 0xd4,
        0x5d, 0x75, 0xc9, 0x3d, 0x2d, 0x86, 0xcb, 0xfc, 0x21, 0x61, 0x6f, 0x9f, 0x1a, 0x57, 0x6c,
        0xcf, 0x8c, 0x86, 0x2e, 0xe1, 0x85, 0x12, 0x5f, 0xc1, 0xed, 0x7e, 0xd2, 0x48, 0x6e, 0x2c,
        0x5f, 0xbf, 0xc3, 0x9c, 0x91, 0x95, 0x97, 0xdd, 0x86, 0xc3, 0x38, 0xe7, 0xdf, 0x55, 0x3d,
        0x51, 0xe8,
    ];
}
