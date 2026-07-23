use alloy::hex;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use getrandom::fill;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use super::{Result, WalletConnectError};

const WALLETCONNECT_ENVELOPE_TYPE_0: u8 = 0;
const WALLETCONNECT_NONCE_LEN: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalletConnectEnvelope {
    pub envelope_type: u8,
    pub nonce: [u8; WALLETCONNECT_NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

impl WalletConnectEnvelope {
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + WALLETCONNECT_NONCE_LEN + self.ciphertext.len());
        bytes.push(self.envelope_type);
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.ciphertext);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() <= 1 + WALLETCONNECT_NONCE_LEN {
            return Err(WalletConnectError::Crypto);
        }
        if bytes[0] != WALLETCONNECT_ENVELOPE_TYPE_0 {
            return Err(WalletConnectError::Crypto);
        }
        let mut nonce = [0u8; WALLETCONNECT_NONCE_LEN];
        nonce.copy_from_slice(&bytes[1..=WALLETCONNECT_NONCE_LEN]);
        Ok(Self {
            envelope_type: bytes[0],
            nonce,
            ciphertext: bytes[1 + WALLETCONNECT_NONCE_LEN..].to_vec(),
        })
    }

    #[must_use]
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.to_bytes())
    }

    pub fn from_base64(value: &str) -> Result<Self> {
        let bytes = STANDARD
            .decode(value)
            .map_err(|_| WalletConnectError::Crypto)?;
        Self::from_bytes(&bytes)
    }
}

pub fn encode_walletconnect_message(
    sym_key: &[u8; 32],
    plaintext: &[u8],
) -> Result<WalletConnectEnvelope> {
    let mut nonce = [0u8; WALLETCONNECT_NONCE_LEN];
    fill(&mut nonce).map_err(|_| WalletConnectError::Crypto)?;
    encode_walletconnect_message_with_nonce(sym_key, plaintext, nonce)
}

pub(crate) fn encode_walletconnect_message_with_nonce(
    sym_key: &[u8; 32],
    plaintext: &[u8],
    nonce: [u8; WALLETCONNECT_NONCE_LEN],
) -> Result<WalletConnectEnvelope> {
    let cipher = ChaCha20Poly1305::new(&Key::from(*sym_key));
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &[],
            },
        )
        .map_err(|_| WalletConnectError::Crypto)?;
    Ok(WalletConnectEnvelope {
        envelope_type: WALLETCONNECT_ENVELOPE_TYPE_0,
        nonce,
        ciphertext,
    })
}

pub fn decode_walletconnect_message(
    sym_key: &[u8; 32],
    envelope: &WalletConnectEnvelope,
) -> Result<Vec<u8>> {
    if envelope.envelope_type != WALLETCONNECT_ENVELOPE_TYPE_0 {
        return Err(WalletConnectError::Crypto);
    }
    let cipher = ChaCha20Poly1305::new(&Key::from(*sym_key));
    cipher
        .decrypt(
            &Nonce::from(envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &[],
            },
        )
        .map_err(|_| WalletConnectError::Crypto)
}

pub fn generate_walletconnect_key_pair() -> Result<([u8; 32], [u8; 32])> {
    let mut private_key = [0u8; 32];
    fill(&mut private_key).map_err(|_| WalletConnectError::Crypto)?;
    let public_key = PublicKey::from(&StaticSecret::from(private_key)).to_bytes();
    Ok((private_key, public_key))
}

pub fn derive_walletconnect_session_sym_key(
    private_key: &[u8; 32],
    peer_public_key: &[u8; 32],
) -> Result<[u8; 32]> {
    let private = StaticSecret::from(*private_key);
    let peer = PublicKey::from(*peer_public_key);
    let shared = private.diffie_hellman(&peer);
    if shared.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(WalletConnectError::Crypto);
    }
    let hkdf = Hkdf::<Sha256>::new(None, shared.as_bytes());
    let mut output = [0u8; 32];
    hkdf.expand(&[], &mut output)
        .map_err(|_| WalletConnectError::Crypto)?;
    Ok(output)
}

#[must_use]
pub fn derive_walletconnect_session_topic(sym_key: &[u8; 32]) -> String {
    hash_walletconnect_key(sym_key)
}

#[must_use]
pub fn hash_walletconnect_key(key: &[u8; 32]) -> String {
    hex::encode(Sha256::digest(key))
}
