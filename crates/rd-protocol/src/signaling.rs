//! Signaling-plane protobuf messages (P3). The signaling server relays these
//! between peers without interpreting SDP or candidate payloads; media access
//! units never appear here (ADR-0003).

use prost::Message;

/// One message per WebSocket binary frame. Unknown variants decode to `None`
/// fields and must not crash older peers.
#[derive(Clone, PartialEq, Message)]
pub struct SignalingMessage {
    #[prost(oneof = "signaling_message::Kind", tags = "1, 2, 3, 4, 5, 6")]
    pub kind: Option<signaling_message::Kind>,
}

pub mod signaling_message {
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        Register(super::Register),
        #[prost(message, tag = "2")]
        RegisterResult(super::RegisterResult),
        #[prost(message, tag = "3")]
        Signal(super::Signal),
        #[prost(message, tag = "4")]
        SignalError(super::SignalError),
        #[prost(message, tag = "5")]
        Presence(super::Presence),
        #[prost(message, tag = "6")]
        Heartbeat(super::Heartbeat),
    }
}

/// Claim a device ID for the lifetime of this connection. Registration is the
/// only ID ownership mechanism in P3; it carries no endpoint authentication.
#[derive(Clone, PartialEq, Message)]
pub struct Register {
    #[prost(string, tag = "1")]
    pub device_id: String,
    /// Signaling protocol version; mismatched versions are rejected.
    #[prost(uint32, tag = "2")]
    pub protocol_version: u32,
}

pub const SIGNALING_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum RegisterReason {
    Ok = 0,
    /// The ID is already registered by a live connection.
    IdInUse = 1,
    /// Malformed ID or unsupported protocol version.
    Invalid = 2,
    /// Server rejected the registration (rate limit, capacity).
    Rejected = 3,
}

/// Server reply to [`Register`]; carries time-limited TURN credentials issued
/// with the shared coturn REST secret. The server never learns the media keys.
#[derive(Clone, PartialEq, Message)]
pub struct RegisterResult {
    #[prost(string, tag = "1")]
    pub device_id: String,
    #[prost(enumeration = "RegisterReason", tag = "2")]
    pub reason: i32,
    #[prost(message, optional, tag = "3")]
    pub turn: Option<TurnCredential>,
}

/// TURN REST API credential (coturn `use-auth-secret` scheme):
/// `username = "<unix expiry>:<device id>"` and
/// `password = base64(HMAC-SHA1(static secret, username))`.
#[derive(Clone, PartialEq, Message)]
pub struct TurnCredential {
    #[prost(string, tag = "1")]
    pub username: String,
    #[prost(string, tag = "2")]
    pub password: String,
    /// Unix seconds after which coturn rejects the allocation.
    #[prost(uint64, tag = "3")]
    pub expires_unix: u64,
    /// `host:port` of the TURN (UDP) endpoint.
    #[prost(string, tag = "4")]
    pub turn_addr: String,
    /// `host:port` of the STUN endpoint (same coturn deployment in v1).
    #[prost(string, tag = "5")]
    pub stun_addr: String,
}

/// Peer-to-peer relay payload. `body` stays opaque to the server; it is either
/// an SDP offer/answer text or one ICE candidate.
#[derive(Clone, PartialEq, Message)]
pub struct Signal {
    #[prost(string, tag = "1")]
    pub peer: String,
    #[prost(oneof = "signal::Body", tags = "2, 3, 4")]
    pub body: Option<signal::Body>,
}

pub mod signal {
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Body {
        #[prost(string, tag = "2")]
        SdpOffer(String),
        #[prost(string, tag = "3")]
        SdpAnswer(String),
        #[prost(message, tag = "4")]
        Candidate(super::Candidate),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum CandidateKind {
    Host = 0,
    Srflx = 1,
    Relayed = 2,
}

/// ICE candidate in signaling form; `related` is the base address for srflx
/// and relayed candidates (empty string when not applicable).
#[derive(Clone, PartialEq, Message)]
pub struct Candidate {
    #[prost(enumeration = "CandidateKind", tag = "1")]
    pub kind: i32,
    /// `ip:port` of the candidate as reachable by the peer.
    #[prost(string, tag = "2")]
    pub addr: String,
    #[prost(string, tag = "3")]
    pub related: String,
    /// Transport protocol; only `"udp"` exists in v1.
    #[prost(string, tag = "4")]
    pub protocol: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum SignalErrorCode {
    /// No registration ever existed for the target ID.
    UnknownPeer = 0,
    /// The target ID exists but has no live connection.
    PeerOffline = 1,
    /// Sender exceeded the signaling rate limit.
    RateLimited = 2,
    /// Payload exceeded the server-side size cap.
    TooLarge = 3,
}

#[derive(Clone, PartialEq, Message)]
pub struct SignalError {
    #[prost(string, tag = "1")]
    pub peer: String,
    #[prost(enumeration = "SignalErrorCode", tag = "2")]
    pub code: i32,
}

/// Presence change for a peer this connection previously signaled. The server
/// sends these without a poll request; watchers are implicit.
#[derive(Clone, PartialEq, Message)]
pub struct Presence {
    #[prost(string, tag = "1")]
    pub peer: String,
    #[prost(bool, tag = "2")]
    pub online: bool,
}

/// Application-level liveness probe; the server echoes every heartbeat back so
/// both sides can enforce their own idle deadlines.
#[derive(Clone, PartialEq, Message)]
pub struct Heartbeat {}

impl SignalingMessage {
    pub fn encode(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    pub fn decode_wire(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(bytes)
    }

    pub fn register(device_id: &str) -> Self {
        Self {
            kind: Some(signaling_message::Kind::Register(Register {
                device_id: device_id.to_string(),
                protocol_version: SIGNALING_PROTOCOL_VERSION,
            })),
        }
    }

    pub fn sdp_offer(peer: &str, sdp: String) -> Self {
        Self {
            kind: Some(signaling_message::Kind::Signal(Signal {
                peer: peer.to_string(),
                body: Some(signal::Body::SdpOffer(sdp)),
            })),
        }
    }

    pub fn sdp_answer(peer: &str, sdp: String) -> Self {
        Self {
            kind: Some(signaling_message::Kind::Signal(Signal {
                peer: peer.to_string(),
                body: Some(signal::Body::SdpAnswer(sdp)),
            })),
        }
    }

    pub fn candidate(peer: &str, candidate: Candidate) -> Self {
        Self {
            kind: Some(signaling_message::Kind::Signal(Signal {
                peer: peer.to_string(),
                body: Some(signal::Body::Candidate(candidate)),
            })),
        }
    }
}

impl Candidate {
    pub fn udp(kind: CandidateKind, addr: impl Into<String>, related: Option<String>) -> Self {
        Self {
            kind: kind as i32,
            addr: addr.into(),
            related: related.unwrap_or_default(),
            protocol: "udp".to_string(),
        }
    }

    pub fn candidate_kind(&self) -> Option<CandidateKind> {
        CandidateKind::try_from(self.kind).ok()
    }
}

impl TurnCredential {
    /// The unix expiry encoded in the REST username (`"<expiry>:<id>"`).
    pub fn username_expiry(&self) -> Option<u64> {
        self.username
            .split_once(':')
            .and_then(|(expiry, _)| expiry.parse().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> Candidate {
        Candidate::udp(
            CandidateKind::Relayed,
            "203.0.113.7:49170",
            Some("192.168.1.4:52311".to_string()),
        )
    }

    #[test]
    fn signal_candidate_roundtrip() {
        let message = SignalingMessage::candidate("peer-a", candidate());
        let decoded = SignalingMessage::decode_wire(&message.encode()).unwrap();
        assert_eq!(decoded, message);
        let Some(signaling_message::Kind::Signal(signal)) = decoded.kind else {
            panic!("signal variant expected");
        };
        let Some(signal::Body::Candidate(info)) = signal.body else {
            panic!("candidate body expected");
        };
        assert_eq!(info.candidate_kind(), Some(CandidateKind::Relayed));
        assert_eq!(info.addr, "203.0.113.7:49170");
        assert_eq!(info.related, "192.168.1.4:52311");
        assert_eq!(info.protocol, "udp");
        assert_eq!(
            Candidate::udp(CandidateKind::Host, "127.0.0.1:1", None).related,
            ""
        );
    }

    #[test]
    fn register_and_turn_credential_roundtrip() {
        let message = SignalingMessage {
            kind: Some(signaling_message::Kind::RegisterResult(RegisterResult {
                device_id: "dev9".into(),
                reason: RegisterReason::Ok as i32,
                turn: Some(TurnCredential {
                    username: "1790000000:dev9".into(),
                    password: "c2VjcmV0".into(),
                    expires_unix: 1_790_000_000,
                    turn_addr: "turn.example.com:3478".into(),
                    stun_addr: "turn.example.com:3478".into(),
                }),
            })),
        };
        let decoded = SignalingMessage::decode_wire(&message.encode()).unwrap();
        assert_eq!(decoded, message);
        let Some(signaling_message::Kind::RegisterResult(result)) = decoded.kind else {
            panic!("register result expected");
        };
        let turn = result.turn.unwrap();
        assert_eq!(turn.username_expiry(), Some(1_790_000_000));
        assert_eq!(
            RegisterReason::try_from(result.reason),
            Ok(RegisterReason::Ok)
        );
    }

    #[test]
    fn empty_heartbeat_and_default_free_omissions_survive_roundtrip() {
        let message = SignalingMessage {
            kind: Some(signaling_message::Kind::Heartbeat(Heartbeat {})),
        };
        let encoded = message.encode();
        // An empty message still emits the oneof tag so the variant survives.
        assert!(!encoded.is_empty());
        assert_eq!(SignalingMessage::decode_wire(&encoded).unwrap(), message);

        let presence = SignalingMessage {
            kind: Some(signaling_message::Kind::Presence(Presence {
                peer: "gone".into(),
                online: false,
            })),
        };
        assert_eq!(
            SignalingMessage::decode_wire(&presence.encode()).unwrap(),
            presence
        );
    }

    #[test]
    fn sdp_bodies_roundtrip_and_error_codes_decode() {
        let offer =
            SignalingMessage::sdp_offer("peer-b", "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n".into());
        let decoded = SignalingMessage::decode_wire(&offer.encode()).unwrap();
        assert_eq!(decoded, offer);

        let answer = SignalingMessage::sdp_answer("peer-b", "v=0\r\n".into());
        assert_eq!(
            SignalingMessage::decode_wire(&answer.encode()).unwrap(),
            answer
        );

        let error = SignalingMessage {
            kind: Some(signaling_message::Kind::SignalError(SignalError {
                peer: "peer-b".into(),
                code: SignalErrorCode::PeerOffline as i32,
            })),
        };
        let decoded = SignalingMessage::decode_wire(&error.encode()).unwrap();
        assert_eq!(decoded, error);
        let Some(signaling_message::Kind::SignalError(error)) = decoded.kind else {
            panic!("signal error expected");
        };
        assert_eq!(
            SignalErrorCode::try_from(error.code),
            Ok(SignalErrorCode::PeerOffline)
        );
    }
}
