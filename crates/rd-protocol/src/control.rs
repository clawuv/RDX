//! Control-plane protobuf messages. Media access units stay on the data channel.
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct ControlMessage {
    #[prost(oneof = "control_message::Kind", tags = "1, 2, 3")]
    pub kind: Option<control_message::Kind>,
}

pub mod control_message {
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        KeyframeRequest(super::KeyframeRequest),
        #[prost(message, tag = "2")]
        Capabilities(super::Capabilities),
        #[prost(message, tag = "3")]
        Heartbeat(super::Heartbeat),
    }
}

/// Fixed experimental profile. Limits describe receive capacity; v1 rejects
/// insufficient peers instead of pretending to support dynamic fragmentation.
#[derive(Clone, PartialEq, Message)]
pub struct Capabilities {
    #[prost(uint32, tag = "1")]
    pub profile_version: u32,
    #[prost(uint32, tag = "2")]
    pub max_video_message: u32,
    #[prost(uint32, tag = "3")]
    pub max_control_message: u32,
    #[prost(uint32, tag = "4")]
    pub max_frame_payload: u32,
    #[prost(bool, tag = "5")]
    pub h264_timestamp_v1: bool,
    #[prost(bool, tag = "6")]
    pub keyframe_recovery: bool,
    #[prost(bool, tag = "7")]
    pub heartbeat: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("peer does not support the complete experimental media profile v1")]
pub struct IncompatibleCapabilities;

impl Capabilities {
    pub fn local() -> Self {
        Self {
            profile_version: 1,
            max_video_message: (crate::fragment::CHUNK + crate::fragment::ENVELOPE) as u32,
            max_control_message: 16 * 1024,
            max_frame_payload: crate::MAX_PAYLOAD_LEN,
            h264_timestamp_v1: true,
            keyframe_recovery: true,
            heartbeat: true,
        }
    }

    pub fn validate_peer(&self) -> Result<(), IncompatibleCapabilities> {
        let required = Self::local();
        if self.profile_version != required.profile_version
            || self.max_video_message < required.max_video_message
            || self.max_control_message < required.max_control_message
            || self.max_frame_payload < required.max_frame_payload
            || !self.h264_timestamp_v1
            || !self.keyframe_recovery
            || !self.heartbeat
        {
            return Err(IncompatibleCapabilities);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyframeRequest {
    #[prost(uint32, tag = "1")]
    pub stream_id: u32,
    /// Last access-unit ID known incomplete at the receiver.
    #[prost(uint64, tag = "2")]
    pub after_frame_id: u64,
}

/// Transport-internal liveness probe; only the matching response acknowledges it.
#[derive(Clone, PartialEq, Message)]
pub struct Heartbeat {
    #[prost(uint64, tag = "1")]
    pub nonce: u64,
    #[prost(bool, tag = "2")]
    pub response: bool,
}

impl ControlMessage {
    pub fn keyframe_request(stream_id: u8, after_frame_id: u64) -> Self {
        Self {
            kind: Some(control_message::Kind::KeyframeRequest(KeyframeRequest {
                stream_id: u32::from(stream_id),
                after_frame_id,
            })),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_wire(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_wire_roundtrip_and_incompatible_profiles() {
        let local = Capabilities::local();
        let message = ControlMessage {
            kind: Some(control_message::Kind::Capabilities(local.clone())),
        };
        assert_eq!(
            ControlMessage::decode_wire(&message.encode()).unwrap(),
            message
        );
        assert!(local.validate_peer().is_ok());
        for field in 0..7 {
            let mut peer = local.clone();
            match field {
                0 => peer.profile_version += 1,
                1 => peer.max_video_message -= 1,
                2 => peer.max_control_message -= 1,
                3 => peer.max_frame_payload -= 1,
                4 => peer.h264_timestamp_v1 = false,
                5 => peer.keyframe_recovery = false,
                _ => peer.heartbeat = false,
            }
            assert_eq!(peer.validate_peer(), Err(IncompatibleCapabilities));
        }
        let mut larger = local;
        larger.max_frame_payload += 1;
        assert!(larger.validate_peer().is_ok());
    }

    #[test]
    fn keyframe_request_round_trips_frame_id_across_sequence_wrap() {
        let message = ControlMessage::keyframe_request(7, 65_536);
        let decoded = ControlMessage::decode(message.encode().as_slice()).unwrap();
        assert_eq!(decoded, message);
        let Some(control_message::Kind::KeyframeRequest(request)) = decoded.kind else {
            panic!("keyframe request variant expected")
        };
        assert_eq!(request.stream_id, 7);
        assert_eq!(request.after_frame_id, 65_536);
    }
}
