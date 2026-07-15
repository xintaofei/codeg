use std::sync::atomic::{AtomicU64, Ordering};

use aes_gcm::{
    aead::{consts::U12, Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use anyhow::{bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use p256::{
    ecdh::EphemeralSecret,
    elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint},
    PublicKey,
};
use sha2::Sha256;

use crate::protocol::{
    PairEnvelope, RelayFrame, DESKTOP_TO_MOBILE_NONCE_TAG, MOBILE_TO_DESKTOP_NONCE_TAG,
    PROTOCOL_VERSION,
};

type HmacSha256 = Hmac<Sha256>;

pub struct SessionCrypto {
    pub device_id: String,
    pub connection_id: String,
    mobile_to_desktop: Aes256Gcm,
    desktop_to_mobile: Aes256Gcm,
    received_seq: AtomicU64,
    send_seq: AtomicU64,
}

impl SessionCrypto {
    pub fn from_mobile_hello(
        desktop_id: &str,
        pairing_root: &[u8; 32],
        hello: &PairEnvelope,
    ) -> anyhow::Result<(Self, PairEnvelope)> {
        if hello.v != PROTOCOL_VERSION
            || hello.phase != "mobile_hello"
            || hello.desktop_id != desktop_id
        {
            bail!("invalid mobile handshake metadata");
        }
        verify_handshake_proof(
            pairing_root,
            "mobile",
            desktop_id,
            &hello.device_id,
            &hello.connection_id,
            &hello.public_key,
            &hello.proof,
        )?;

        let mobile_bytes = URL_SAFE_NO_PAD
            .decode(&hello.public_key)
            .context("invalid mobile public key encoding")?;
        let mobile_public =
            PublicKey::from_sec1_bytes(&mobile_bytes).context("invalid mobile P-256 public key")?;
        let desktop_secret = EphemeralSecret::random(&mut OsRng);
        let desktop_public = PublicKey::from(&desktop_secret);
        let desktop_public_encoded =
            URL_SAFE_NO_PAD.encode(desktop_public.to_encoded_point(false).as_bytes());
        let shared = desktop_secret.diffie_hellman(&mobile_public);
        let (mobile_to_desktop, desktop_to_mobile) = derive_keys(
            shared.raw_secret_bytes(),
            pairing_root,
            &hello.connection_id,
        )?;
        let proof = handshake_proof(
            pairing_root,
            "desktop",
            desktop_id,
            &hello.device_id,
            &hello.connection_id,
            &desktop_public_encoded,
        );
        let response = PairEnvelope {
            v: PROTOCOL_VERSION,
            phase: "desktop_hello".into(),
            desktop_id: desktop_id.into(),
            device_id: hello.device_id.clone(),
            connection_id: hello.connection_id.clone(),
            public_key: desktop_public_encoded,
            proof,
        };
        Ok((
            Self {
                device_id: hello.device_id.clone(),
                connection_id: hello.connection_id.clone(),
                mobile_to_desktop,
                desktop_to_mobile,
                received_seq: AtomicU64::new(0),
                send_seq: AtomicU64::new(0),
            },
            response,
        ))
    }

    pub fn open_mobile_frame(&self, frame: &RelayFrame) -> anyhow::Result<Vec<u8>> {
        if frame.v != PROTOCOL_VERSION
            || frame.message_type != "frame"
            || frame.device_id != self.device_id
            || frame.connection_id != self.connection_id
        {
            bail!("frame does not match the authenticated session");
        }
        let expected = self.received_seq.load(Ordering::Acquire) + 1;
        if frame.seq != expected {
            bail!("unexpected or replayed mobile sequence");
        }
        let expected_nonce = relay_nonce(MOBILE_TO_DESKTOP_NONCE_TAG, frame.seq)?;
        let supplied_nonce = URL_SAFE_NO_PAD
            .decode(&frame.nonce)
            .context("invalid frame nonce encoding")?;
        if supplied_nonce != expected_nonce {
            bail!("frame nonce does not match its sequence");
        }
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&frame.ciphertext)
            .context("invalid frame ciphertext encoding")?;
        let nonce = Nonce::<U12>::from(expected_nonce);
        let plaintext = self
            .mobile_to_desktop
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad: &frame.aad(),
                },
            )
            .map_err(|_| anyhow::anyhow!("frame authentication failed"))?;
        self.received_seq.store(frame.seq, Ordering::Release);
        Ok(plaintext)
    }

    pub fn seal_desktop_payload(
        &self,
        desktop_id: &str,
        plaintext: &[u8],
    ) -> anyhow::Result<RelayFrame> {
        let seq = self.send_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let ack = self.received_seq.load(Ordering::Acquire);
        let nonce = relay_nonce(DESKTOP_TO_MOBILE_NONCE_TAG, seq)?;
        let mut frame = RelayFrame {
            v: PROTOCOL_VERSION,
            message_type: "frame".into(),
            desktop_id: desktop_id.into(),
            device_id: self.device_id.clone(),
            connection_id: self.connection_id.clone(),
            frame_id: format!("f_{}", uuid::Uuid::new_v4().simple()),
            seq,
            ack,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: String::new(),
        };
        let aes_nonce = Nonce::<U12>::from(nonce);
        let ciphertext = self
            .desktop_to_mobile
            .encrypt(
                &aes_nonce,
                Payload {
                    msg: plaintext,
                    aad: &frame.aad(),
                },
            )
            .map_err(|_| anyhow::anyhow!("failed to encrypt desktop frame"))?;
        frame.ciphertext = URL_SAFE_NO_PAD.encode(ciphertext);
        Ok(frame)
    }
}

pub fn handshake_proof(
    pairing_root: &[u8; 32],
    role: &str,
    desktop_id: &str,
    device_id: &str,
    connection_id: &str,
    public_key: &str,
) -> String {
    let canonical = handshake_canonical(role, desktop_id, device_id, connection_id, public_key);
    let mut mac = <HmacSha256 as Mac>::new_from_slice(pairing_root)
        .expect("HMAC accepts a key of any length");
    mac.update(canonical.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn verify_handshake_proof(
    pairing_root: &[u8; 32],
    role: &str,
    desktop_id: &str,
    device_id: &str,
    connection_id: &str,
    public_key: &str,
    proof: &str,
) -> anyhow::Result<()> {
    let proof = URL_SAFE_NO_PAD
        .decode(proof)
        .context("invalid handshake proof encoding")?;
    let canonical = handshake_canonical(role, desktop_id, device_id, connection_id, public_key);
    let mut mac = <HmacSha256 as Mac>::new_from_slice(pairing_root)
        .expect("HMAC accepts a key of any length");
    mac.update(canonical.as_bytes());
    mac.verify_slice(&proof)
        .map_err(|_| anyhow::anyhow!("handshake proof is invalid"))
}

fn handshake_canonical(
    role: &str,
    desktop_id: &str,
    device_id: &str,
    connection_id: &str,
    public_key: &str,
) -> String {
    format!("codeg-relay-v1|{role}|{desktop_id}|{device_id}|{connection_id}|{public_key}")
}

fn derive_keys(
    shared_secret: &[u8],
    pairing_root: &[u8; 32],
    connection_id: &str,
) -> anyhow::Result<(Aes256Gcm, Aes256Gcm)> {
    let hkdf = Hkdf::<Sha256>::new(Some(pairing_root), shared_secret);
    let mut mobile_to_desktop = [0_u8; 32];
    let mut desktop_to_mobile = [0_u8; 32];
    hkdf.expand(
        format!("codeg-relay-v1|{connection_id}|mobile-to-desktop").as_bytes(),
        &mut mobile_to_desktop,
    )
    .map_err(|_| anyhow::anyhow!("failed to derive mobile key"))?;
    hkdf.expand(
        format!("codeg-relay-v1|{connection_id}|desktop-to-mobile").as_bytes(),
        &mut desktop_to_mobile,
    )
    .map_err(|_| anyhow::anyhow!("failed to derive desktop key"))?;
    Ok((
        Aes256Gcm::new_from_slice(&mobile_to_desktop)
            .map_err(|_| anyhow::anyhow!("invalid mobile key"))?,
        Aes256Gcm::new_from_slice(&desktop_to_mobile)
            .map_err(|_| anyhow::anyhow!("invalid desktop key"))?,
    ))
}

fn relay_nonce(direction_tag: u32, sequence: u64) -> anyhow::Result<[u8; 12]> {
    if sequence == 0 {
        bail!("sequence zero has no valid nonce");
    }
    let mut nonce = [0_u8; 12];
    nonce[..4].copy_from_slice(&direction_tag.to_be_bytes());
    nonce[4..].copy_from_slice(&sequence.to_be_bytes());
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn mobile_hello(root: &[u8; 32]) -> (EphemeralSecret, PairEnvelope) {
        let secret = EphemeralSecret::random(&mut OsRng);
        let public = PublicKey::from(&secret);
        let public_key = URL_SAFE_NO_PAD.encode(public.to_encoded_point(false).as_bytes());
        let proof = handshake_proof(root, "mobile", "d_test", "m_phone", "c_test", &public_key);
        (
            secret,
            PairEnvelope {
                v: PROTOCOL_VERSION,
                phase: "mobile_hello".into(),
                desktop_id: "d_test".into(),
                device_id: "m_phone".into(),
                connection_id: "c_test".into(),
                public_key,
                proof,
            },
        )
    }

    #[test]
    fn rejects_forged_handshake() {
        let root = [9_u8; 32];
        let (_, mut hello) = mobile_hello(&root);
        hello.proof = handshake_proof(
            &[8_u8; 32],
            "mobile",
            "d_test",
            "m_phone",
            "c_test",
            &hello.public_key,
        );
        assert!(SessionCrypto::from_mobile_hello("d_test", &root, &hello).is_err());
    }

    #[test]
    fn authenticated_session_encrypts_and_rejects_replay() {
        let root = [9_u8; 32];
        let (mobile_secret, hello) = mobile_hello(&root);
        let (desktop, response) =
            SessionCrypto::from_mobile_hello("d_test", &root, &hello).unwrap();
        verify_handshake_proof(
            &root,
            "desktop",
            "d_test",
            "m_phone",
            "c_test",
            &response.public_key,
            &response.proof,
        )
        .unwrap();

        let desktop_public_bytes = URL_SAFE_NO_PAD.decode(response.public_key).unwrap();
        let desktop_public = PublicKey::from_sec1_bytes(&desktop_public_bytes).unwrap();
        let shared = mobile_secret.diffie_hellman(&desktop_public);
        let (mobile_to_desktop, desktop_to_mobile) =
            derive_keys(shared.raw_secret_bytes(), &root, "c_test").unwrap();

        let nonce = relay_nonce(MOBILE_TO_DESKTOP_NONCE_TAG, 1).unwrap();
        let mut inbound = RelayFrame {
            v: 1,
            message_type: "frame".into(),
            desktop_id: "d_test".into(),
            device_id: "m_phone".into(),
            connection_id: "c_test".into(),
            frame_id: "f_mobile".into(),
            seq: 1,
            ack: 0,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: String::new(),
        };
        let aes_nonce = Nonce::<U12>::from(nonce);
        let encrypted = mobile_to_desktop
            .encrypt(
                &aes_nonce,
                Payload {
                    msg: br#"{"kind":"request"}"#,
                    aad: &inbound.aad(),
                },
            )
            .unwrap();
        inbound.ciphertext = URL_SAFE_NO_PAD.encode(encrypted);
        assert_eq!(
            desktop.open_mobile_frame(&inbound).unwrap(),
            br#"{"kind":"request"}"#
        );
        assert!(desktop.open_mobile_frame(&inbound).is_err());

        let outbound = desktop
            .seal_desktop_payload("d_test", br#"{"kind":"response"}"#)
            .unwrap();
        let nonce = relay_nonce(DESKTOP_TO_MOBILE_NONCE_TAG, outbound.seq).unwrap();
        let ciphertext = URL_SAFE_NO_PAD.decode(&outbound.ciphertext).unwrap();
        let aes_nonce = Nonce::<U12>::from(nonce);
        let plaintext = desktop_to_mobile
            .decrypt(
                &aes_nonce,
                Payload {
                    msg: &ciphertext,
                    aad: &outbound.aad(),
                },
            )
            .unwrap();
        assert_eq!(plaintext, br#"{"kind":"response"}"#);
    }
}
