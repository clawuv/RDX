//! Signaling client (P3, ADR-0008): WebSocket connection to `rd-server`
//! plus the offer/answer/candidate orchestration that used to happen
//! in-process in `connect_pair`. The server relays SDP text and candidates
//! opaquely; media never touches it.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rd_protocol::signaling::{
    signal, signaling_message, RegisterReason, SignalErrorCode, SignalingMessage, TurnCredential,
};
use str0m::change::{SdpAnswer, SdpOffer};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;

use crate::str0m_impl::IceConfig;
use crate::Str0mTransport;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Extra drain window so late candidates from the peer land before dial
/// and answer return; gathering is synchronous so this is usually one burst.
const CANDIDATE_DRAIN: Duration = Duration::from_millis(150);

#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("signaling websocket failed: {0}")]
    Ws(String),
    #[error("signaling connection closed")]
    Closed,
    #[error("registration rejected: {0:?}")]
    Rejected(RegisterReason),
    #[error("signaling to {peer} failed: {code:?}")]
    Peer { peer: String, code: SignalErrorCode },
    #[error("handshake timed out")]
    Timeout,
    #[error("protocol error: {0}")]
    Protocol(String),
}

#[derive(Debug)]
pub enum SignalingEvent {
    Signal {
        peer: String,
        body: signal::Body,
    },
    Presence {
        peer: String,
        online: bool,
    },
    /// Echo of our own heartbeat; callers enforce their own deadline.
    Heartbeat,
}

/// One WebSocket to the signaling server. Not sync: register first, then
/// send signals and pump `recv` (during handshakes `dial`/`answer` pump it).
pub struct SignalingClient {
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    device_id: String,
    turn: Option<TurnCredential>,
}

impl SignalingClient {
    pub async fn connect(url: &str) -> Result<Self, SignalingError> {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|error| SignalingError::Ws(error.to_string()))?;
        Ok(Self {
            ws,
            device_id: String::new(),
            turn: None,
        })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn turn(&self) -> Option<&TurnCredential> {
        self.turn.as_ref()
    }

    /// Claim `device_id`; the reply carries TURN credentials for this session.
    pub async fn register(&mut self, device_id: &str) -> Result<&TurnCredential, SignalingError> {
        self.send(&SignalingMessage::register(device_id)).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let frame = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .map_err(|_| SignalingError::Timeout)?
                .transpose()
                .map_err(|error| SignalingError::Ws(error.to_string()))?;
            let Some(WsMessage::Binary(bytes)) = frame else {
                return Err(SignalingError::Closed);
            };
            let message = SignalingMessage::decode_wire(&bytes)
                .map_err(|error| SignalingError::Protocol(error.to_string()))?;
            if let Some(signaling_message::Kind::RegisterResult(result)) = message.kind {
                let reason = RegisterReason::try_from(result.reason)
                    .map_err(|_| SignalingError::Protocol("bad register reason".into()))?;
                match reason {
                    RegisterReason::Ok => {
                        self.device_id = result.device_id;
                        let Some(turn) = result.turn else {
                            return Err(SignalingError::Protocol("no TURN credential".into()));
                        };
                        self.turn = Some(turn);
                        return Ok(self.turn.as_ref().expect("just set"));
                    }
                    other => return Err(SignalingError::Rejected(other)),
                }
            }
        }
    }

    async fn send(&mut self, message: &SignalingMessage) -> Result<(), SignalingError> {
        self.ws
            .send(WsMessage::Binary(message.encode().into()))
            .await
            .map_err(|error| SignalingError::Ws(error.to_string()))?;
        Ok(())
    }

    pub async fn send_signal(
        &mut self,
        peer: &str,
        message: &SignalingMessage,
    ) -> Result<(), SignalingError> {
        let forwarded = match &message.kind {
            Some(signaling_message::Kind::Signal(signal)) if signal.peer == peer => message.clone(),
            _ => return Err(SignalingError::Protocol("not a signal for the peer".into())),
        };
        self.send(&forwarded).await
    }

    /// Application-level liveness probe; the server echoes it back.
    pub async fn heartbeat(&mut self) -> Result<(), SignalingError> {
        self.send(&SignalingMessage {
            kind: Some(signaling_message::Kind::Heartbeat(
                rd_protocol::signaling::Heartbeat {},
            )),
        })
        .await
    }

    /// Next server event. SignalingError::Peer carries relay rejections.
    pub async fn recv(&mut self) -> Result<SignalingEvent, SignalingError> {
        loop {
            let frame = self
                .ws
                .next()
                .await
                .transpose()
                .map_err(|error| SignalingError::Ws(error.to_string()))?
                .ok_or(SignalingError::Closed)?;
            let bytes = match frame {
                WsMessage::Binary(bytes) => bytes,
                WsMessage::Close(_) => return Err(SignalingError::Closed),
                _ => continue,
            };
            let message = SignalingMessage::decode_wire(&bytes)
                .map_err(|error| SignalingError::Protocol(error.to_string()))?;
            match message.kind {
                Some(signaling_message::Kind::Signal(signal)) => {
                    return Ok(SignalingEvent::Signal {
                        peer: signal.peer,
                        body: signal.body.ok_or_else(|| {
                            SignalingError::Protocol("signal without body".into())
                        })?,
                    });
                }
                Some(signaling_message::Kind::SignalError(error)) => {
                    let code = SignalErrorCode::try_from(error.code)
                        .map_err(|_| SignalingError::Protocol("bad signal error".into()))?;
                    return Err(SignalingError::Peer {
                        peer: error.peer,
                        code,
                    });
                }
                Some(signaling_message::Kind::Presence(presence)) => {
                    return Ok(SignalingEvent::Presence {
                        peer: presence.peer,
                        online: presence.online,
                    });
                }
                Some(signaling_message::Kind::Heartbeat(_)) => {
                    return Ok(SignalingEvent::Heartbeat);
                }
                Some(signaling_message::Kind::Register(_))
                | Some(signaling_message::Kind::RegisterResult(_))
                | None => {
                    return Err(SignalingError::Protocol("unexpected message".into()));
                }
            }
        }
    }
}

/// Caller side: create the offerer with `ice`, relay the offer and local
/// candidates, then pump signals until the answer and remote candidates
/// arrive. The transport keeps working afterwards; later candidates need
/// the caller to keep pumping `recv` itself (v1 gathers everything up front,
/// so a final drain window is enough).
pub async fn dial(
    signaling: &mut SignalingClient,
    peer: &str,
    ice: &IceConfig,
) -> Result<Str0mTransport, SignalingError> {
    let (mut transport, offer, pending, local) = crate::Str0mTransport::new_offerer_ice(ice)
        .map_err(|error| SignalingError::Protocol(format!("ice setup: {error}")))?;
    signaling
        .send(&SignalingMessage::sdp_offer(peer, offer.to_sdp_string()))
        .await?;
    for candidate in local.to_signaling() {
        signaling
            .send(&SignalingMessage::candidate(peer, candidate))
            .await?;
    }

    let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    let mut answered = false;
    let mut remotes = 0usize;
    let mut pending = Some(pending);
    while tokio::time::Instant::now() < deadline && (!answered || remotes == 0) {
        let event = tokio::time::timeout_at(deadline, signaling.recv())
            .await
            .map_err(|_| SignalingError::Timeout)??;
        match event {
            SignalingEvent::Signal { peer: from, body } if from == peer => match body {
                signal::Body::SdpAnswer(text) if !answered => {
                    let answer = SdpAnswer::from_sdp_string(&text).map_err(|error| {
                        SignalingError::Protocol(format!("answer sdp: {error}"))
                    })?;
                    let pending = pending
                        .take()
                        .ok_or(SignalingError::Protocol("double answer".into()))?;
                    transport
                        .accept_answer(pending, answer)
                        .map_err(|_| SignalingError::Closed)?;
                    answered = true;
                }
                signal::Body::Candidate(info) => {
                    transport
                        .add_remote_candidate(info)
                        .await
                        .map_err(|_| SignalingError::Closed)?;
                    remotes += 1;
                }
                _ => {
                    return Err(SignalingError::Protocol(
                        "unexpected offer on dialer".into(),
                    ))
                }
            },
            SignalingEvent::Signal { .. } => {}
            SignalingEvent::Presence { .. } | SignalingEvent::Heartbeat => {}
        }
    }
    if !answered {
        return Err(SignalingError::Timeout);
    }
    drain_candidates(signaling, peer, &mut transport).await?;
    Ok(transport)
}

/// Callee side: wait for one offer (from any registered peer), answer it,
/// send local candidates, and pump until the caller's candidates arrive.
/// Returns the remote device id with the ready transport.
pub async fn answer(
    signaling: &mut SignalingClient,
    ice: &IceConfig,
) -> Result<(String, Str0mTransport), SignalingError> {
    let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    let (peer, offer) = loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(SignalingError::Timeout);
        }
        let event = tokio::time::timeout_at(deadline, signaling.recv())
            .await
            .map_err(|_| SignalingError::Timeout)??;
        if let SignalingEvent::Signal {
            peer,
            body: signal::Body::SdpOffer(text),
        } = event
        {
            let offer = SdpOffer::from_sdp_string(&text)
                .map_err(|error| SignalingError::Protocol(format!("offer sdp: {error}")))?;
            break (peer, offer);
        }
    };
    let (mut transport, answer, local) = crate::Str0mTransport::new_answerer_ice(offer, ice)
        .map_err(|error| SignalingError::Protocol(format!("ice setup: {error}")))?;
    signaling
        .send(&SignalingMessage::sdp_answer(&peer, answer.to_sdp_string()))
        .await?;
    for candidate in local.to_signaling() {
        signaling
            .send(&SignalingMessage::candidate(&peer, candidate))
            .await?;
    }

    let mut remotes = 0usize;
    while tokio::time::Instant::now() < deadline && remotes == 0 {
        let event = tokio::time::timeout_at(deadline, signaling.recv())
            .await
            .map_err(|_| SignalingError::Timeout)??;
        if let SignalingEvent::Signal {
            peer: from,
            body: signal::Body::Candidate(info),
        } = event
        {
            if from == peer {
                transport
                    .add_remote_candidate(info)
                    .await
                    .map_err(|_| SignalingError::Closed)?;
                remotes += 1;
            }
        }
    }
    drain_candidates(signaling, &peer, &mut transport).await?;
    Ok((peer, transport))
}

async fn drain_candidates(
    signaling: &mut SignalingClient,
    peer: &str,
    transport: &mut Str0mTransport,
) -> Result<(), SignalingError> {
    let deadline = tokio::time::Instant::now() + CANDIDATE_DRAIN;
    while tokio::time::Instant::now() < deadline {
        let event = match tokio::time::timeout_at(deadline, signaling.recv()).await {
            Ok(event) => event?,
            Err(_) => break,
        };
        if let SignalingEvent::Signal {
            peer: from,
            body: signal::Body::Candidate(info),
        } = event
        {
            if from == peer {
                let _ = transport.add_remote_candidate(info).await;
            }
        }
    }
    Ok(())
}

/// Cheap random device id ([a-z0-9]{10}); uniqueness, not unpredictability,
/// is the property the registry needs. Rejected ids retry with a new draw.
pub fn generate_device_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default()
        ^ (std::process::id() as u64);
    let mut id = String::with_capacity(10);
    for _ in 0..10 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        id.push(ALPHABET[(seed >> 33) as usize % ALPHABET.len()] as char);
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transport as _;
    use rd_protocol::Frame;

    async fn spawn_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            rd_server::serve(
                listener,
                rd_server::ServerConfig {
                    turn_secret: b"test".to_vec(),
                    turn_addr: "127.0.0.1:3478".into(),
                    stun_addr: "127.0.0.1:3478".into(),
                    ..rd_server::ServerConfig::default()
                },
            )
            .await
            .unwrap();
        });
        format!("ws://{addr}/ws")
    }

    async fn registered(url: &str, id: &str) -> SignalingClient {
        let mut client = SignalingClient::connect(url).await.unwrap();
        client.register(id).await.unwrap();
        assert_eq!(client.device_id(), id);
        assert!(client.turn().is_some());
        client
    }

    fn frame(payload: &[u8]) -> Frame {
        Frame::new(1, 0, false, bytes::Bytes::copy_from_slice(payload))
    }

    #[tokio::test]
    async fn dial_and_answer_exchange_frames_over_real_signaling() {
        let url = spawn_server().await;
        let (mut caller, mut callee) = (
            registered(&url, "caller01").await,
            registered(&url, "callee01").await,
        );
        // Swap one heartbeat each and observe the echo before dialing.
        caller.heartbeat().await.unwrap();
        callee.heartbeat().await.unwrap();
        match caller.recv().await.unwrap() {
            SignalingEvent::Heartbeat => {}
            other => panic!("expected heartbeat echo, got {other:?}"),
        }

        let caller_ice = IceConfig::default();
        let callee_ice = IceConfig::default();
        let (dial_result, answer_result) = tokio::join!(
            dial(&mut caller, "callee01", &caller_ice),
            answer(&mut callee, &callee_ice),
        );
        let mut a = dial_result.expect("dial");
        let (peer, mut b) = answer_result.expect("answer");
        assert_eq!(peer, "caller01");

        let payload: &[u8] = &[9, 8, 7];
        a.send(frame(payload)).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(10), b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&got.payload[..], payload);
        b.send(frame(b"upstream")).await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(10), a.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&got.payload[..], b"upstream");
    }

    #[tokio::test]
    async fn dial_to_unknown_peer_returns_typed_error() {
        let url = spawn_server().await;
        let mut caller = registered(&url, "caller02").await;
        let ice = IceConfig::default();
        let error = match dial(&mut caller, "ghost00", &ice).await {
            Ok(_) => panic!("dial to a ghost must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            SignalingError::Peer {
                code: SignalErrorCode::UnknownPeer,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn duplicate_registration_is_rejected() {
        let url = spawn_server().await;
        let _first = registered(&url, "twice00").await;
        let mut second = SignalingClient::connect(&url).await.unwrap();
        let error = second.register("twice00").await.unwrap_err();
        assert!(matches!(
            error,
            SignalingError::Rejected(RegisterReason::IdInUse)
        ));
    }

    #[test]
    fn generated_ids_fit_the_registry_charset() {
        for _ in 0..100 {
            let id = generate_device_id();
            assert_eq!(id.len(), 10);
            assert!(id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()));
        }
    }
}
