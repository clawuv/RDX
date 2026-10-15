//! Signaling WebSocket service: ID registration/addressing, opaque SDP and
//! ICE relay, presence, and bounded resource use (ADR-0008).
//!
//! The server never interprets `Signal` bodies, keeps no media state, and logs
//! no payloads. Registration is in-memory and per-connection; a durable
//! Postgres device registry is deferred to deployment hardening.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use rd_protocol::signaling::{
    RegisterReason, SignalErrorCode, SignalingMessage, SIGNALING_PROTOCOL_VERSION,
};
use tokio::sync::mpsc;

use crate::turn::TurnIssuer;

/// Inbound WS frames above this size are protocol violations and close the
/// socket; app-level payload caps answer with `SignalError::TooLarge` instead.
const MAX_WS_MESSAGE: usize = 128 * 1024;
/// Outbound queue per connection; a full queue means a dead peer.
const OUTBOUND_BUFFER: usize = 128;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub turn_secret: Vec<u8>,
    /// Advertised TURN (UDP) `host:port`, usually the same coturn as STUN.
    pub turn_addr: String,
    pub stun_addr: String,
    pub credential_ttl: Duration,
    /// Silence longer than this unregisters the device and closes the socket.
    pub heartbeat_timeout: Duration,
    /// Largest accepted SDP offer/answer payload.
    pub max_signal_payload: usize,
    /// Signal messages allowed per window per connection.
    pub signal_rate: u32,
    pub signal_rate_window: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            turn_secret: Vec::new(),
            turn_addr: String::new(),
            stun_addr: String::new(),
            credential_ttl: Duration::from_secs(600),
            heartbeat_timeout: Duration::from_secs(30),
            max_signal_payload: 64 * 1024,
            signal_rate: 100,
            signal_rate_window: Duration::from_secs(5),
        }
    }
}

struct DeviceHandle {
    tx: mpsc::Sender<SignalingMessage>,
}

#[derive(Clone)]
struct Shared {
    config: Arc<ServerConfig>,
    devices: Arc<RwLock<HashMap<String, DeviceHandle>>>,
    /// watched id -> live watcher ids, plus the reverse map for cleanup.
    watchers: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    watching: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// Ids seen since process start, so "never seen" stays distinguishable
    /// from "seen but currently offline".
    known_ids: Arc<RwLock<HashSet<String>>>,
}

impl Shared {
    fn new(config: ServerConfig) -> Self {
        Self {
            config: Arc::new(config),
            devices: Arc::new(RwLock::new(HashMap::new())),
            watchers: Arc::new(RwLock::new(HashMap::new())),
            watching: Arc::new(RwLock::new(HashMap::new())),
            known_ids: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Non-blocking delivery under no lock; a full or closed queue drops the
    /// message and liveness enforcement reaps the peer either way.
    fn try_deliver(&self, id: &str, message: SignalingMessage) -> bool {
        let tx = self
            .devices
            .read()
            .expect("devices lock")
            .get(id)
            .map(|device| device.tx.clone());
        match tx {
            Some(tx) => tx.try_send(message).is_ok(),
            None => false,
        }
    }

    fn notify_watchers(&self, id: &str, online: bool) {
        let watchers = self
            .watchers
            .read()
            .expect("watchers lock")
            .get(id)
            .cloned()
            .unwrap_or_default();
        for watcher in watchers {
            self.try_deliver(
                &watcher,
                SignalingMessage {
                    kind: Some(signaling_kind::Kind::Presence(
                        rd_protocol::signaling::Presence {
                            peer: id.to_string(),
                            online,
                        },
                    )),
                },
            );
        }
    }
}

use rd_protocol::signaling::signaling_message as signaling_kind;

pub fn app(config: ServerConfig) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/healthz", get(health))
        .with_state(Shared::new(config))
}

async fn health() -> &'static str {
    "ok"
}

async fn ws_handler(ws: WebSocketUpgrade, State(shared): State<Shared>) -> Response {
    ws.max_message_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| connection(socket, shared))
}

enum RelayOutcome {
    Delivered,
    PeerOffline,
    UnknownPeer,
}

/// Forward one opaque Signal to its target if it has a live connection.
/// Watchers are recorded for every attempt, not only deliveries, so dialers
/// learn when an offline peer reappears.
fn relay_signal(
    shared: &Shared,
    from: &str,
    signal: rd_protocol::signaling::Signal,
) -> RelayOutcome {
    let outcome = {
        let devices = shared.devices.read().expect("devices lock");
        match devices.get(&signal.peer) {
            Some(target) => {
                let forwarded = SignalingMessage {
                    kind: Some(signaling_kind::Kind::Signal(
                        rd_protocol::signaling::Signal {
                            peer: from.to_string(),
                            body: signal.body,
                        },
                    )),
                };
                if target.tx.try_send(forwarded).is_ok() {
                    RelayOutcome::Delivered
                } else {
                    RelayOutcome::PeerOffline
                }
            }
            None => {
                if shared
                    .known_ids
                    .read()
                    .expect("known ids lock")
                    .contains(&signal.peer)
                {
                    RelayOutcome::PeerOffline
                } else {
                    RelayOutcome::UnknownPeer
                }
            }
        }
    };
    let mut watchers = shared.watchers.write().expect("watchers lock");
    watchers
        .entry(signal.peer.clone())
        .or_default()
        .insert(from.to_string());
    drop(watchers);
    let mut watching = shared.watching.write().expect("watching lock");
    watching
        .entry(from.to_string())
        .or_default()
        .insert(signal.peer);
    outcome
}

struct RateLimiter {
    window: Duration,
    capacity: u32,
    events: VecDeque<Instant>,
}

impl RateLimiter {
    fn new(capacity: u32, window: Duration) -> Self {
        Self {
            window,
            capacity,
            events: VecDeque::new(),
        }
    }

    fn allow(&mut self, now: Instant) -> bool {
        while let Some(oldest) = self.events.back() {
            if now.duration_since(*oldest) > self.window {
                self.events.pop_back();
            } else {
                break;
            }
        }
        if self.events.len() >= self.capacity as usize {
            return false;
        }
        self.events.push_front(now);
        true
    }
}

fn valid_device_id(id: &str) -> bool {
    (4..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// Queue a typed error reply; `false` means the outbound queue is gone.
fn signal_error(tx: &mpsc::Sender<SignalingMessage>, peer: &str, code: SignalErrorCode) -> bool {
    tx.try_send(SignalingMessage {
        kind: Some(signaling_kind::Kind::SignalError(
            rd_protocol::signaling::SignalError {
                peer: peer.to_string(),
                code: code as i32,
            },
        )),
    })
    .is_ok()
}

async fn connection(socket: WebSocket, shared: Shared) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<SignalingMessage>(OUTBOUND_BUFFER);
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink
                .send(Message::Binary(message.encode().into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let mut registered: Option<String> = None;
    let mut limiter = RateLimiter::new(shared.config.signal_rate, shared.config.signal_rate_window);
    let issuer = TurnIssuer::new(
        shared.config.turn_secret.clone(),
        shared.config.credential_ttl,
        shared.config.turn_addr.clone(),
        shared.config.stun_addr.clone(),
    );

    let timeout = shared.config.heartbeat_timeout;
    loop {
        let frame = match tokio::time::timeout(timeout, stream.next()).await {
            Err(_) => break,           // idle past the heartbeat deadline
            Ok(None) => break,         // socket closed
            Ok(Some(Err(_))) => break, // protocol/transport error
            Ok(Some(Ok(frame))) => frame,
        };
        let bytes = match frame {
            Message::Binary(bytes) => bytes,
            Message::Close(_) => break,
            // Ping/Pong are answered by the WS layer itself.
            Message::Ping(_) | Message::Pong(_) => continue,
            // Text frames and unknown control frames are protocol violations.
            Message::Text(_) => break,
        };
        let message = match SignalingMessage::decode_wire(&bytes) {
            Ok(message) => message,
            Err(_) => break,
        };
        match message.kind {
            Some(signaling_kind::Kind::Register(register)) => {
                let device_id = register.device_id;
                let reason = if register.protocol_version != SIGNALING_PROTOCOL_VERSION
                    || !valid_device_id(&device_id)
                {
                    RegisterReason::Invalid
                } else {
                    let mut devices = shared.devices.write().expect("devices lock");
                    if devices.contains_key(&device_id) {
                        RegisterReason::IdInUse
                    } else {
                        devices.insert(device_id.clone(), DeviceHandle { tx: tx.clone() });
                        drop(devices);
                        shared
                            .known_ids
                            .write()
                            .expect("known ids lock")
                            .insert(device_id.clone());
                        registered = Some(device_id.clone());
                        shared.notify_watchers(&device_id, true);
                        RegisterReason::Ok
                    }
                };
                let turn =
                    (reason == RegisterReason::Ok).then(|| issuer.issue(&device_id, unix_now()));
                if tx
                    .try_send(SignalingMessage {
                        kind: Some(signaling_kind::Kind::RegisterResult(
                            rd_protocol::signaling::RegisterResult {
                                device_id,
                                reason: reason as i32,
                                turn,
                            },
                        )),
                    })
                    .is_err()
                {
                    break;
                }
            }
            Some(signaling_kind::Kind::Signal(signal)) => {
                let Some(from) = registered.clone() else {
                    break; // signaling before registration
                };
                if !limiter.allow(Instant::now()) {
                    if !signal_error(&tx, &signal.peer, SignalErrorCode::RateLimited) {
                        break;
                    }
                    continue;
                }
                let payload_size = match &signal.body {
                    Some(rd_protocol::signaling::signal::Body::SdpOffer(sdp)) => sdp.len(),
                    Some(rd_protocol::signaling::signal::Body::SdpAnswer(sdp)) => sdp.len(),
                    _ => 0,
                };
                if payload_size > shared.config.max_signal_payload {
                    if !signal_error(&tx, &signal.peer, SignalErrorCode::TooLarge) {
                        break;
                    }
                    continue;
                }
                match relay_signal(&shared, &from, signal) {
                    RelayOutcome::Delivered => {}
                    RelayOutcome::PeerOffline => {
                        if !signal_error(&tx, &from, SignalErrorCode::PeerOffline) {
                            break;
                        }
                    }
                    RelayOutcome::UnknownPeer => {
                        if !signal_error(&tx, &from, SignalErrorCode::UnknownPeer) {
                            break;
                        }
                    }
                }
            }
            Some(signaling_kind::Kind::Heartbeat(_)) => {
                if tx.try_send(message).is_err() {
                    break;
                }
            }
            // Server-to-client messages and empty frames are violations.
            Some(signaling_kind::Kind::RegisterResult(_))
            | Some(signaling_kind::Kind::SignalError(_))
            | Some(signaling_kind::Kind::Presence(_))
            | None => break,
        }
    }

    if let Some(id) = registered.take() {
        shared.devices.write().expect("devices lock").remove(&id);
        shared.notify_watchers(&id, false);
        let watching = shared
            .watching
            .write()
            .expect("watching lock")
            .remove(&id)
            .unwrap_or_default();
        let mut watchers = shared.watchers.write().expect("watchers lock");
        for watched in watching {
            if let Some(set) = watchers.get_mut(&watched) {
                set.remove(&id);
            }
        }
    }
    drop(tx);
    let _ = writer.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rd_protocol::signaling::{Candidate, CandidateKind};
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use tokio_tungstenite::MaybeTlsStream;
    use tokio_tungstenite::WebSocketStream;

    type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    async fn start(config: ServerConfig) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app(config)).await.unwrap();
        });
        addr
    }

    async fn connect(addr: std::net::SocketAddr) -> Client {
        let (stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("connect signaling ws");
        stream
    }

    async fn send(client: &mut Client, message: &SignalingMessage) {
        client
            .send(WsMessage::Binary(message.encode().into()))
            .await
            .unwrap();
    }

    async fn recv(client: &mut Client) -> SignalingMessage {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), client.next())
                .await
                .expect("frame within 5s")
                .expect("connection open")
                .expect("frame ok");
            if let WsMessage::Binary(bytes) = frame {
                return SignalingMessage::decode_wire(&bytes).unwrap();
            }
        }
    }

    fn base_config() -> ServerConfig {
        ServerConfig {
            turn_secret: b"test-secret".to_vec(),
            turn_addr: "127.0.0.1:3478".into(),
            stun_addr: "127.0.0.1:3478".into(),
            ..ServerConfig::default()
        }
    }

    #[tokio::test]
    async fn register_rejects_duplicate_and_invalid_ids() {
        let addr = start(base_config()).await;
        let mut a = connect(addr).await;
        send(&mut a, &SignalingMessage::register("alpha")).await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::RegisterResult(result)) => {
                assert_eq!(
                    RegisterReason::try_from(result.reason),
                    Ok(RegisterReason::Ok)
                );
                let turn = result.turn.expect("turn credential attached");
                assert!(turn.username.ends_with(":alpha"));
                assert_eq!(turn.turn_addr, "127.0.0.1:3478");
            }
            other => panic!("expected register result, got {other:?}"),
        }

        let mut b = connect(addr).await;
        send(&mut b, &SignalingMessage::register("alpha")).await;
        match recv(&mut b).await.kind {
            Some(signaling_kind::Kind::RegisterResult(result)) => assert_eq!(
                RegisterReason::try_from(result.reason),
                Ok(RegisterReason::IdInUse)
            ),
            other => panic!("expected register result, got {other:?}"),
        }
        send(&mut b, &SignalingMessage::register("bad-id!")).await;
        match recv(&mut b).await.kind {
            Some(signaling_kind::Kind::RegisterResult(result)) => assert_eq!(
                RegisterReason::try_from(result.reason),
                Ok(RegisterReason::Invalid)
            ),
            other => panic!("expected register result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn signals_relay_opaquely_between_registered_peers() {
        let addr = start(base_config()).await;
        let (mut a, mut b) = (connect(addr).await, connect(addr).await);
        send(&mut a, &SignalingMessage::register("alpha")).await;
        send(&mut b, &SignalingMessage::register("beta")).await;
        let _ = futures_util::future::join_all(vec![recv(&mut a), recv(&mut b)]).await;

        let candidate = Candidate::udp(CandidateKind::Srflx, "198.51.100.2:60000", None);
        send(
            &mut a,
            &SignalingMessage::candidate("beta", candidate.clone()),
        )
        .await;
        match recv(&mut b).await.kind {
            Some(signaling_kind::Kind::Signal(signal)) => {
                assert_eq!(signal.peer, "alpha");
                assert_eq!(
                    signal.body,
                    Some(rd_protocol::signaling::signal::Body::Candidate(candidate))
                );
            }
            other => panic!("expected relayed signal, got {other:?}"),
        }

        send(
            &mut b,
            &SignalingMessage::sdp_answer("alpha", "v=0\r\n".into()),
        )
        .await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::Signal(signal)) => {
                assert_eq!(signal.peer, "beta");
                assert_eq!(
                    signal.body,
                    Some(rd_protocol::signaling::signal::Body::SdpAnswer(
                        "v=0\r\n".into()
                    ))
                );
            }
            other => panic!("expected relayed answer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_and_offline_peers_return_typed_errors() {
        let addr = start(base_config()).await;
        let mut a = connect(addr).await;
        send(&mut a, &SignalingMessage::register("alpha")).await;
        let _ = recv(&mut a).await;

        send(
            &mut a,
            &SignalingMessage::sdp_offer("ghost", "v=0\r\n".into()),
        )
        .await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::SignalError(error)) => assert_eq!(
                SignalErrorCode::try_from(error.code),
                Ok(SignalErrorCode::UnknownPeer)
            ),
            other => panic!("expected unknown peer, got {other:?}"),
        }

        let mut b = connect(addr).await;
        send(&mut b, &SignalingMessage::register("beta")).await;
        let _ = recv(&mut b).await;
        send(
            &mut a,
            &SignalingMessage::sdp_offer("beta", "v=0\r\n".into()),
        )
        .await;
        let _ = recv(&mut b).await; // delivered to beta
        drop(b);
        // alpha watches beta now, so the offline presence arrives first.
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::Presence(presence)) => {
                assert_eq!(presence.peer, "beta");
                assert!(!presence.online);
            }
            other => panic!("expected offline presence, got {other:?}"),
        }
        send(
            &mut a,
            &SignalingMessage::sdp_offer("beta", "v=0\r\n".into()),
        )
        .await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::SignalError(error)) => assert_eq!(
                SignalErrorCode::try_from(error.code),
                Ok(SignalErrorCode::PeerOffline)
            ),
            other => panic!("expected peer offline, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn watchers_receive_presence_transitions() {
        let addr = start(base_config()).await;
        let mut a = connect(addr).await;
        send(&mut a, &SignalingMessage::register("alpha")).await;
        let _ = recv(&mut a).await;

        send(
            &mut a,
            &SignalingMessage::sdp_offer("beta", "v=0\r\n".into()),
        )
        .await;
        let _ = recv(&mut a).await; // UnknownPeer error; alpha now watches beta

        let mut b = connect(addr).await;
        send(&mut b, &SignalingMessage::register("beta")).await;
        let _ = recv(&mut b).await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::Presence(presence)) => {
                assert_eq!(presence.peer, "beta");
                assert!(presence.online);
            }
            other => panic!("expected online presence, got {other:?}"),
        }

        drop(b);
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::Presence(presence)) => {
                assert_eq!(presence.peer, "beta");
                assert!(!presence.online);
            }
            other => panic!("expected offline presence, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversized_payload_answers_too_large_without_closing() {
        let addr = start(base_config()).await;
        let mut a = connect(addr).await;
        send(&mut a, &SignalingMessage::register("alpha")).await;
        let _ = recv(&mut a).await;

        let huge = "x".repeat(64 * 1024 + 1);
        send(&mut a, &SignalingMessage::sdp_offer("beta", huge)).await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::SignalError(error)) => assert_eq!(
                SignalErrorCode::try_from(error.code),
                Ok(SignalErrorCode::TooLarge)
            ),
            other => panic!("expected too large, got {other:?}"),
        }
        // Connection survives and keeps answering normally afterwards.
        send(
            &mut a,
            &SignalingMessage::sdp_offer("beta", "v=0\r\n".into()),
        )
        .await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::SignalError(error)) => assert_eq!(
                SignalErrorCode::try_from(error.code),
                Ok(SignalErrorCode::UnknownPeer)
            ),
            other => panic!("expected unknown peer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn signal_rate_limit_answers_rate_limited() {
        let mut config = base_config();
        config.signal_rate = 3;
        config.signal_rate_window = Duration::from_secs(60);
        let addr = start(config).await;
        let mut a = connect(addr).await;
        send(&mut a, &SignalingMessage::register("alpha")).await;
        let _ = recv(&mut a).await;

        for _ in 0..3 {
            send(
                &mut a,
                &SignalingMessage::sdp_offer("beta", "v=0\r\n".into()),
            )
            .await;
            let _ = recv(&mut a).await; // UnknownPeer
        }
        send(
            &mut a,
            &SignalingMessage::sdp_offer("beta", "v=0\r\n".into()),
        )
        .await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::SignalError(error)) => assert_eq!(
                SignalErrorCode::try_from(error.code),
                Ok(SignalErrorCode::RateLimited)
            ),
            other => panic!("expected rate limited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn idle_connection_is_reaped_and_unregistered() {
        let mut config = base_config();
        config.heartbeat_timeout = Duration::from_millis(150);
        let addr = start(config).await;
        let mut watcher = connect(addr).await;
        send(&mut watcher, &SignalingMessage::register("watcher")).await;
        let _ = recv(&mut watcher).await;

        let mut silent = connect(addr).await;
        send(&mut silent, &SignalingMessage::register("silent")).await;
        let _ = recv(&mut silent).await;
        send(
            &mut watcher,
            &SignalingMessage::sdp_offer("silent", "v=0\r\n".into()),
        )
        .await;
        let _ = recv(&mut silent).await; // delivered; watcher now watches silent

        // watcher only receives from here on; silent goes idle and is reaped.
        match recv(&mut watcher).await.kind {
            Some(signaling_kind::Kind::Presence(presence)) => {
                assert_eq!(presence.peer, "silent");
                assert!(!presence.online);
            }
            other => panic!("expected offline presence, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn heartbeats_keep_an_active_connection_registered() {
        let mut config = base_config();
        config.heartbeat_timeout = Duration::from_millis(300);
        let addr = start(config).await;
        let mut a = connect(addr).await;
        send(&mut a, &SignalingMessage::register("alpha")).await;
        let _ = recv(&mut a).await;
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            send(
                &mut a,
                &SignalingMessage {
                    kind: Some(signaling_kind::Kind::Heartbeat(
                        rd_protocol::signaling::Heartbeat {},
                    )),
                },
            )
            .await;
            match recv(&mut a).await.kind {
                Some(signaling_kind::Kind::Heartbeat(_)) => {}
                other => panic!("expected heartbeat echo, got {other:?}"),
            }
        }
        // Still registered: signaling to self relays back.
        send(
            &mut a,
            &SignalingMessage::sdp_offer("alpha", "v=0\r\n".into()),
        )
        .await;
        match recv(&mut a).await.kind {
            Some(signaling_kind::Kind::Signal(signal)) => assert_eq!(signal.peer, "alpha"),
            other => panic!("expected self relay, got {other:?}"),
        }
    }
}
