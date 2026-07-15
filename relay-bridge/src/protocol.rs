use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u8 = 1;
pub const MOBILE_TO_DESKTOP_NONCE_TAG: u32 = 0x004d_3244;
pub const DESKTOP_TO_MOBILE_NONCE_TAG: u32 = 0x0044_324d;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingEnvelope {
    #[serde(rename = "pair")]
    Pair(PairEnvelope),
    #[serde(rename = "frame")]
    Frame(RelayFrame),
    #[serde(rename = "error")]
    Error { code: String },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PairEnvelope {
    pub v: u8,
    pub phase: String,
    pub desktop_id: String,
    pub device_id: String,
    pub connection_id: String,
    pub public_key: String,
    pub proof: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RelayFrame {
    pub v: u8,
    #[serde(rename = "type")]
    pub message_type: String,
    pub desktop_id: String,
    pub device_id: String,
    pub connection_id: String,
    pub frame_id: String,
    pub seq: u64,
    pub ack: u64,
    pub nonce: String,
    pub ciphertext: String,
}

impl RelayFrame {
    pub fn aad(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.v,
            self.desktop_id,
            self.device_id,
            self.connection_id,
            self.frame_id,
            self.seq,
            self.ack
        )
        .into_bytes()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RelayRequest {
    pub request_id: String,
    pub command: String,
    #[serde(default)]
    pub args: Value,
    pub idempotency_key: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum EncryptedPayload {
    #[serde(rename = "request")]
    Request(RelayRequest),
    #[serde(rename = "ws_frame")]
    WsFrame { frame: Value },
    #[serde(rename = "cancel")]
    Cancel { request_id: String },
    #[serde(other)]
    Other,
}

pub fn valid_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

pub fn valid_command(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}
