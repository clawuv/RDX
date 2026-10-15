//! Minimal TURN client (RFC 5766 subset) for relayed ICE candidates.
//!
//! str0m ships no TURN client ("TURN is like enumerating NICs"), so this
//! implements the point-to-point subset a remote-desktop session needs:
//! one allocation with long-term credentials (coturn REST API style),
//! per-peer CreatePermission, Refresh, and Send/Data indications. Channel
//! binding, TCP relays and IPv6 relays are out of scope for v1.
//!
//! The client runs synchronously on the str0m worker thread; every request
//! bounds its retransmission budget so an unreachable server fails fast.

use std::collections::{HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use sha1::Sha1;

const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
const HEADER_LEN: usize = 20;

// Message types: (class << 4) | method. Classes: 0 request, 1 indication,
// 2 success, 3 error.
const ALLOCATE_REQUEST: u16 = 0x0003;
const ALLOCATE_SUCCESS: u16 = 0x0103;
const REFRESH_REQUEST: u16 = 0x0004;
const REFRESH_SUCCESS: u16 = 0x0104;
const PERMISSION_REQUEST: u16 = 0x0008;
const PERMISSION_SUCCESS: u16 = 0x0108;
const SEND_INDICATION: u16 = 0x0016;
const DATA_INDICATION: u16 = 0x0017;

const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_LIFETIME: u16 = 0x000d;
const ATTR_REALM: u16 = 0x0014;
const ATTR_NONCE: u16 = 0x0015;
const ATTR_XOR_PEER_ADDRESS: u16 = 0x0012;
const ATTR_DATA: u16 = 0x0013;
const ATTR_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const ATTR_REQUESTED_TRANSPORT: u16 = 0x0019;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const FAMILY_IPV4: u8 = 0x01;

const UDP_TRANSPORT: u8 = 17;
const DEFAULT_LIFETIME_SECS: u32 = 600;
/// Refresh the allocation and its permissions well before expiry.
const REFRESH_MARGIN: Duration = Duration::from_secs(120);

const RETRANSMIT_DELAYS: [Duration; 4] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
];

#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    #[error("turn server did not answer in time")]
    Timeout,
    #[error("turn protocol error: {0}")]
    Protocol(&'static str),
    #[error("turn rejected the request with code {code}")]
    Rejected { code: u16 },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn push_attribute(message: &mut Vec<u8>, attribute: u16, value: &[u8]) {
    message.extend_from_slice(&attribute.to_be_bytes());
    message.extend_from_slice(&(value.len() as u16).to_be_bytes());
    message.extend_from_slice(value);
    while !message.len().is_multiple_of(4) {
        message.push(0);
    }
}

fn update_message_length(message: &mut [u8]) {
    let length = (message.len() - HEADER_LEN) as u16;
    message[2..4].copy_from_slice(&length.to_be_bytes());
}

fn transaction_of(message: &[u8]) -> Option<[u8; 12]> {
    let tail = message.get(8..HEADER_LEN)?;
    <[u8; 12]>::try_from(tail).ok()
}

fn walk_attributes(message: &[u8], mut visit: impl FnMut(u16, &[u8])) -> Result<(), TurnError> {
    let mut offset = HEADER_LEN;
    while message.len() >= offset + 4 {
        let attribute = u16::from_be_bytes([message[offset], message[offset + 1]]);
        let length = usize::from(u16::from_be_bytes([
            message[offset + 2],
            message[offset + 3],
        ]));
        let value = message
            .get(offset + 4..offset + 4 + length)
            .ok_or(TurnError::Protocol("attribute overruns message"))?;
        visit(attribute, value);
        offset += 4 + ((length + 3) & !3);
    }
    if offset != message.len() {
        return Err(TurnError::Protocol("trailing garbage"));
    }
    Ok(())
}

fn xor_mask(transaction: &[u8; 12]) -> [u8; 4] {
    let mut mask = MAGIC_COOKIE;
    for (byte, seed) in mask.iter_mut().zip(transaction.iter()) {
        *byte ^= *seed;
    }
    mask
}

fn decode_xor_address(value: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    if value.len() < 8 || value[1] != FAMILY_IPV4 {
        return None;
    }
    let mask = xor_mask(transaction);
    let port = u16::from_be_bytes([value[2], value[3]]) ^ u16::from_be_bytes([mask[0], mask[1]]);
    let octets = [
        value[4] ^ mask[0],
        value[5] ^ mask[1],
        value[6] ^ mask[2],
        value[7] ^ mask[3],
    ];
    Some(SocketAddr::from((IpAddr::V4(octets.into()), port)))
}

fn encode_xor_address(address: SocketAddr, transaction: &[u8; 12]) -> Vec<u8> {
    let port_raw = address.port();
    let IpAddr::V4(address) = address.ip() else {
        return Vec::new();
    };
    let mask = xor_mask(transaction);
    let port = port_raw ^ u16::from_be_bytes([mask[0], mask[1]]);
    let mut value = vec![0x00, FAMILY_IPV4];
    value.extend_from_slice(&port.to_be_bytes());
    for (byte, key) in address.octets().iter().zip(mask.iter()) {
        value.push(byte ^ key);
    }
    value
}

/// Long-term credential key: MD5(username ":" realm ":" password).
fn long_term_key(username: &str, realm: &str, password: &str) -> Vec<u8> {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(username.as_bytes());
    hasher.update(b":");
    hasher.update(realm.as_bytes());
    hasher.update(b":");
    hasher.update(password.as_bytes());
    hasher.finalize().to_vec()
}

/// MESSAGE-INTEGRITY (RFC 5389 §15.4): HMAC-SHA1 over the message with the
/// header length adjusted to end just after the (not yet appended) attribute.
fn append_message_integrity(
    message: &mut Vec<u8>,
    username: &str,
    realm: &str,
    nonce: &[u8],
    key: &[u8],
) {
    push_attribute(message, ATTR_USERNAME, username.as_bytes());
    push_attribute(message, ATTR_REALM, realm.as_bytes());
    push_attribute(message, ATTR_NONCE, nonce);
    let mut signed = message.clone();
    let length = (signed.len() - HEADER_LEN + 24) as u16;
    signed[2..4].copy_from_slice(&length.to_be_bytes());
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(&signed);
    push_attribute(
        message,
        ATTR_MESSAGE_INTEGRITY,
        &mac.finalize().into_bytes(),
    );
}

/// Build the signed prefix (message up to the MESSAGE-INTEGRITY attribute,
/// length adjusted) so servers can verify requests the same way.
#[cfg(test)]
pub(crate) fn signed_prefix(message: &[u8]) -> Vec<u8> {
    let mut signed = message[..message.len() - 24].to_vec();
    let length = (signed.len() - HEADER_LEN + 24) as u16;
    signed[2..4].copy_from_slice(&length.to_be_bytes());
    signed
}

#[derive(Debug)]
enum ServerReply {
    Success {
        relayed: Option<SocketAddr>,
        mapped: Option<SocketAddr>,
        lifetime: Option<u32>,
    },
    Error {
        code: u16,
        realm: Option<String>,
        nonce: Option<Vec<u8>>,
    },
    Ignored,
}

fn parse_reply(message: &[u8]) -> Result<ServerReply, TurnError> {
    if message.len() < HEADER_LEN {
        return Err(TurnError::Protocol("shorter than header"));
    }
    let message_type = u16::from_be_bytes([message[0], message[1]]);
    let class = (((message_type >> 8) & 1) << 1) | ((message_type >> 4) & 1);
    let transaction = transaction_of(message).ok_or(TurnError::Protocol("no txid"))?;
    match class {
        3 => {
            let mut code = 0u16;
            let mut realm = None;
            let mut nonce = None;
            walk_attributes(message, |attribute, value| match attribute {
                ATTR_ERROR_CODE if value.len() >= 4 => {
                    code = u16::from(value[2] >> 5) * 100 + u16::from(value[3]);
                }
                ATTR_REALM => realm = Some(String::from_utf8_lossy(value).into_owned()),
                ATTR_NONCE => nonce = Some(value.to_vec()),
                _ => {}
            })?;
            Ok(ServerReply::Error { code, realm, nonce })
        }
        2 => {
            let is_allocate = message_type == ALLOCATE_SUCCESS;
            if !is_allocate && message_type != REFRESH_SUCCESS && message_type != PERMISSION_SUCCESS
            {
                return Ok(ServerReply::Ignored);
            }
            let mut relayed = None;
            let mut mapped = None;
            let mut lifetime = None;
            walk_attributes(message, |attribute, value| match attribute {
                ATTR_XOR_RELAYED_ADDRESS if is_allocate => {
                    relayed = decode_xor_address(value, &transaction);
                }
                ATTR_XOR_MAPPED_ADDRESS if is_allocate => {
                    mapped = decode_xor_address(value, &transaction);
                }
                ATTR_LIFETIME if value.len() >= 4 => {
                    lifetime = Some(u32::from_be_bytes([value[0], value[1], value[2], value[3]]));
                }
                _ => {}
            })?;
            Ok(ServerReply::Success {
                relayed,
                mapped,
                lifetime,
            })
        }
        _ => Ok(ServerReply::Ignored),
    }
}

/// Context handed to per-request attribute builders; the transaction id is
/// final before the closure runs so XOR-protected attributes can use it.
pub struct RequestContext {
    pub transaction: [u8; 12],
    pub authenticated: bool,
}

/// One TURN allocation on a dedicated socket plus its maintenance state.
pub struct TurnClient {
    socket: UdpSocket,
    server: SocketAddr,
    username: String,
    password: String,
    realm: String,
    nonce: Vec<u8>,
    relayed: SocketAddr,
    mapped: SocketAddr,
    lifetime: Duration,
    next_refresh: Instant,
    permissions: HashSet<SocketAddr>,
    pending_data: VecDeque<(SocketAddr, Vec<u8>)>,
    next_seed: u64,
}

impl TurnClient {
    /// Allocate a relay with long-term credentials. The first unauthenticated
    /// request learns realm and nonce from a 401, mirroring coturn's REST API
    /// auth; a server that accepts unauthenticated allocations also works.
    pub fn allocate(
        socket: UdpSocket,
        server: SocketAddr,
        username: &str,
        password: &str,
    ) -> Result<Self, TurnError> {
        let mut client = Self {
            socket,
            server,
            username: username.to_string(),
            password: password.to_string(),
            realm: String::new(),
            nonce: Vec::new(),
            relayed: server,
            mapped: server,
            lifetime: Duration::from_secs(u64::from(DEFAULT_LIFETIME_SECS)),
            next_refresh: Instant::now() + REFRESH_MARGIN,
            permissions: HashSet::new(),
            pending_data: VecDeque::new(),
            next_seed: u64::from(std::process::id() as u32) | 1,
        };
        let add_allocate_attributes = |message: &mut Vec<u8>, context: &RequestContext| {
            if !context.authenticated {
                push_attribute(message, ATTR_REQUESTED_TRANSPORT, &[UDP_TRANSPORT, 0, 0, 0]);
                push_attribute(message, ATTR_LIFETIME, &DEFAULT_LIFETIME_SECS.to_be_bytes());
            }
        };
        let reply = client.request(ALLOCATE_REQUEST, &add_allocate_attributes)?;
        let reply = match reply {
            ServerReply::Error {
                code: 401,
                realm: Some(realm),
                nonce: Some(nonce),
            } => {
                client.realm = realm;
                client.nonce = nonce;
                client.request(ALLOCATE_REQUEST, &add_allocate_attributes)?
            }
            ServerReply::Error { code, .. } => return Err(TurnError::Rejected { code }),
            other => other,
        };
        match reply {
            ServerReply::Success {
                relayed,
                mapped,
                lifetime,
            } => {
                client.relayed =
                    relayed.ok_or(TurnError::Protocol("allocation without relayed address"))?;
                client.mapped =
                    mapped.ok_or(TurnError::Protocol("allocation without mapped address"))?;
                if let Some(secs) = lifetime {
                    client.lifetime = Duration::from_secs(u64::from(secs));
                }
                client.schedule_refresh();
                Ok(client)
            }
            ServerReply::Error { code, .. } => Err(TurnError::Rejected { code }),
            ServerReply::Ignored => Err(TurnError::Protocol("unexpected reply to allocate")),
        }
    }

    fn schedule_refresh(&mut self) {
        self.next_refresh = Instant::now()
            + self
                .lifetime
                .saturating_sub(REFRESH_MARGIN)
                .max(Duration::from_secs(30));
    }

    fn next_transaction(&mut self) -> [u8; 12] {
        self.next_seed = self
            .next_seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut transaction = [0u8; 12];
        transaction[..8].copy_from_slice(&self.next_seed.to_be_bytes());
        transaction
    }

    fn request(
        &mut self,
        request_type: u16,
        prepare: &dyn Fn(&mut Vec<u8>, &RequestContext),
    ) -> Result<ServerReply, TurnError> {
        let transaction = self.next_transaction();
        let mut message = Vec::with_capacity(96);
        message.extend_from_slice(&request_type.to_be_bytes());
        message.extend_from_slice(&0u16.to_be_bytes());
        message.extend_from_slice(&MAGIC_COOKIE);
        message.extend_from_slice(&transaction);
        let authenticated = !self.realm.is_empty();
        prepare(
            &mut message,
            &RequestContext {
                transaction,
                authenticated,
            },
        );
        if authenticated {
            let key = long_term_key(&self.username, &self.realm, &self.password);
            let username = self.username.clone();
            let realm = self.realm.clone();
            let nonce = self.nonce.clone();
            append_message_integrity(&mut message, &username, &realm, &nonce, &key);
        }
        update_message_length(&mut message);
        self.transact(&message, transaction)
    }

    fn transact(
        &mut self,
        message: &[u8],
        transaction: [u8; 12],
    ) -> Result<ServerReply, TurnError> {
        let mut buffer = vec![0u8; 2048];
        for delay in RETRANSMIT_DELAYS {
            self.socket.send_to(message, self.server)?;
            let deadline = Instant::now() + delay;
            while Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                self.socket
                    .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;
                match self.socket.recv_from(&mut buffer) {
                    Ok((length, source)) if source == self.server => {
                        let datagram = &buffer[..length];
                        if transaction_of(datagram) == Some(transaction) {
                            if let Ok(
                                reply @ (ServerReply::Success { .. } | ServerReply::Error { .. }),
                            ) = parse_reply(datagram)
                            {
                                return Ok(reply);
                            }
                        }
                        // Non-matching datagrams still carry media.
                        self.absorb(datagram);
                    }
                    Ok(_) => continue,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind() == std::io::ErrorKind::TimedOut
                            || error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Err(TurnError::Timeout)
    }

    /// Queue media payloads found in datagrams that are not the response the
    /// caller is waiting for.
    pub fn absorb(&mut self, datagram: &[u8]) {
        if let Some((peer, data)) = Self::parse_data_indication(datagram) {
            self.pending_data.push_back((peer, data));
        }
    }

    fn parse_data_indication(datagram: &[u8]) -> Option<(SocketAddr, Vec<u8>)> {
        if datagram.len() < HEADER_LEN {
            return None;
        }
        let message_type = u16::from_be_bytes([datagram[0], datagram[1]]);
        if message_type != DATA_INDICATION {
            return None;
        }
        let transaction = transaction_of(datagram)?;
        let mut peer = None;
        let mut data = None;
        let _ = walk_attributes(datagram, |attribute, value| match attribute {
            ATTR_XOR_PEER_ADDRESS => peer = decode_xor_address(value, &transaction),
            ATTR_DATA => data = Some(value.to_vec()),
            _ => {}
        });
        Some((peer?, data?))
    }

    pub fn relayed_address(&self) -> SocketAddr {
        self.relayed
    }

    /// Create a permission for one peer if absent. Blocking with a bounded
    /// budget; called on the worker thread when routing switches to relay.
    pub fn ensure_permission(&mut self, peer: SocketAddr) -> Result<(), TurnError> {
        if self.permissions.contains(&peer) {
            return Ok(());
        }
        let build = move |message: &mut Vec<u8>, context: &RequestContext| {
            if context.authenticated {
                push_attribute(
                    message,
                    ATTR_XOR_PEER_ADDRESS,
                    &encode_xor_address(peer, &context.transaction),
                );
            }
        };
        match self.request(PERMISSION_REQUEST, &build)? {
            ServerReply::Success { .. } => {
                self.permissions.insert(peer);
                Ok(())
            }
            ServerReply::Error { code, .. } => Err(TurnError::Rejected { code }),
            ServerReply::Ignored => Err(TurnError::Protocol("unexpected reply to permission")),
        }
    }

    /// Refresh the allocation and recreate permissions when due. A failed
    /// refresh means the relay is going away and surfaces as an error.
    pub fn refresh_if_due(&mut self, now: Instant) -> Result<bool, TurnError> {
        if now < self.next_refresh {
            return Ok(false);
        }
        let lifetime_secs = self.lifetime.as_secs() as u32;
        let build = move |message: &mut Vec<u8>, context: &RequestContext| {
            if context.authenticated {
                push_attribute(message, ATTR_LIFETIME, &lifetime_secs.to_be_bytes());
            }
        };
        match self.request(REFRESH_REQUEST, &build)? {
            ServerReply::Success { lifetime, .. } => {
                if let Some(secs) = lifetime {
                    if secs == 0 {
                        return Err(TurnError::Rejected { code: 0 });
                    }
                    self.lifetime = Duration::from_secs(u64::from(secs));
                }
            }
            ServerReply::Error { code, .. } => return Err(TurnError::Rejected { code }),
            ServerReply::Ignored => return Err(TurnError::Protocol("unexpected reply to refresh")),
        }
        self.schedule_refresh();
        let peers: Vec<SocketAddr> = self.permissions.drain().collect();
        for peer in peers {
            self.ensure_permission(peer)?;
        }
        Ok(true)
    }

    /// Wrap `payload` in a Send indication toward `peer`. Indications carry
    /// no authentication and expect no response.
    pub fn send_data(&mut self, peer: SocketAddr, payload: &[u8]) -> Result<(), TurnError> {
        let transaction = self.next_transaction();
        let mut message = Vec::with_capacity(HEADER_LEN + 48 + payload.len());
        message.extend_from_slice(&SEND_INDICATION.to_be_bytes());
        message.extend_from_slice(&0u16.to_be_bytes());
        message.extend_from_slice(&MAGIC_COOKIE);
        message.extend_from_slice(&transaction);
        push_attribute(
            &mut message,
            ATTR_XOR_PEER_ADDRESS,
            &encode_xor_address(peer, &transaction),
        );
        push_attribute(&mut message, ATTR_DATA, payload);
        update_message_length(&mut message);
        self.socket.send_to(&message, self.server)?;
        Ok(())
    }

    /// Pop one queued media payload previously absorbed from the server.
    pub fn poll_data(&mut self) -> Option<(SocketAddr, Vec<u8>)> {
        self.pending_data.pop_front()
    }

    pub fn server_address(&self) -> SocketAddr {
        self.server
    }

    /// Local address of the TURN control socket, used to tag inbound relayed
    /// packets when feeding them into the ICE agent.
    pub fn socket_local_addr(&self) -> SocketAddr {
        self.socket.local_addr().unwrap_or(self.server)
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.next_refresh
    }

    /// Read the TURN socket once (blocking up to `timeout`) and absorb any
    /// media indication it carried. Control responses for in-flight
    /// transactions are picked up here as well; unmatched ones are ignored.
    pub fn poll_socket(&mut self, timeout: Duration) {
        let mut buffer = [0u8; 2048];
        if self.socket.set_read_timeout(Some(timeout)).is_err() {
            return;
        }
        match self.socket.recv_from(&mut buffer) {
            Ok((length, source)) if source == self.server => {
                self.absorb(&buffer[..length]);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
pub mod mock;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_term_key_is_md5_of_colon_join() {
        use md5::{Digest, Md5};
        let key = long_term_key("user", "realm", "pass");
        let mut hasher = Md5::new();
        hasher.update(b"user:realm:pass");
        assert_eq!(key, hasher.finalize().to_vec());
    }

    #[test]
    fn message_integrity_signs_adjusted_prefix() {
        let transaction = [7u8; 12];
        let mut message = Vec::new();
        message.extend_from_slice(&ALLOCATE_REQUEST.to_be_bytes());
        message.extend_from_slice(&0u16.to_be_bytes());
        message.extend_from_slice(&MAGIC_COOKIE);
        message.extend_from_slice(&transaction);
        push_attribute(&mut message, ATTR_REQUESTED_TRANSPORT, &[17, 0, 0, 0]);

        let mut with_integrity = message.clone();
        append_message_integrity(&mut with_integrity, "u", "r", b"n", b"key");
        // The MI attribute is the last 24 bytes and signs the adjusted prefix.
        let attribute = u16::from_be_bytes([
            with_integrity[with_integrity.len() - 24],
            with_integrity[with_integrity.len() - 23],
        ]);
        assert_eq!(attribute, ATTR_MESSAGE_INTEGRITY);
        let prefix = signed_prefix(&with_integrity);
        let mut mac = Hmac::<Sha1>::new_from_slice(b"key").unwrap();
        mac.update(&prefix);
        assert_eq!(
            &with_integrity[with_integrity.len() - 20..],
            mac.finalize().into_bytes().as_slice()
        );
    }

    #[test]
    fn error_reply_decodes_realm_and_nonce() {
        let mut message = Vec::new();
        message.extend_from_slice(&0x0113u16.to_be_bytes());
        message.extend_from_slice(&0u16.to_be_bytes());
        message.extend_from_slice(&MAGIC_COOKIE);
        message.extend_from_slice(&[9u8; 12]);
        push_attribute(&mut message, ATTR_ERROR_CODE, &[0, 0, 4 << 5, 1]);
        push_attribute(&mut message, ATTR_REALM, b"example.org");
        push_attribute(&mut message, ATTR_NONCE, b"nonce-1");
        let ServerReply::Error { code, realm, nonce } = parse_reply(&message).unwrap() else {
            panic!("error reply expected");
        };
        assert_eq!(code, 401);
        assert_eq!(realm.as_deref(), Some("example.org"));
        assert_eq!(nonce.as_deref(), Some(&b"nonce-1"[..]));
    }

    #[test]
    fn allocate_success_decodes_addresses_and_lifetime() {
        let transaction = [3u8; 12];
        let relayed: SocketAddr = "198.51.100.9:49152".parse().unwrap();
        let mapped: SocketAddr = "192.0.2.30:54321".parse().unwrap();
        let mut message = Vec::new();
        message.extend_from_slice(&ALLOCATE_SUCCESS.to_be_bytes());
        message.extend_from_slice(&0u16.to_be_bytes());
        message.extend_from_slice(&MAGIC_COOKIE);
        message.extend_from_slice(&transaction);
        push_attribute(
            &mut message,
            ATTR_XOR_RELAYED_ADDRESS,
            &encode_xor_address(relayed, &transaction),
        );
        push_attribute(
            &mut message,
            ATTR_XOR_MAPPED_ADDRESS,
            &encode_xor_address(mapped, &transaction),
        );
        push_attribute(&mut message, ATTR_LIFETIME, &600u32.to_be_bytes());
        let ServerReply::Success {
            relayed: got_relayed,
            mapped: got_mapped,
            lifetime,
        } = parse_reply(&message).unwrap()
        else {
            panic!("success reply expected");
        };
        assert_eq!(got_relayed, Some(relayed));
        assert_eq!(got_mapped, Some(mapped));
        assert_eq!(lifetime, Some(600));
    }

    #[test]
    fn send_and_data_indications_roundtrip() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut client = TurnClient {
            socket,
            server: server_addr,
            username: "u".into(),
            password: "p".into(),
            realm: String::new(),
            nonce: Vec::new(),
            relayed: server_addr,
            mapped: server_addr,
            lifetime: Duration::from_secs(600),
            next_refresh: Instant::now() + Duration::from_secs(120),
            permissions: HashSet::new(),
            pending_data: VecDeque::new(),
            next_seed: 1,
        };
        let peer: SocketAddr = "203.0.113.5:33333".parse().unwrap();
        client.send_data(peer, b"payload").unwrap();
        let mut buffer = [0u8; 1500];
        server
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let (length, _) = server.recv_from(&mut buffer).unwrap();
        let send_indication = &buffer[..length];

        // Server side: unwrap Send indication, rewrap as Data indication back.
        let transaction = transaction_of(send_indication).unwrap();
        let mut data = Vec::new();
        let mut send_peer = None;
        walk_attributes(send_indication, |attribute, value| match attribute {
            ATTR_XOR_PEER_ADDRESS => send_peer = decode_xor_address(value, &transaction),
            ATTR_DATA => data = value.to_vec(),
            _ => {}
        })
        .unwrap();
        assert_eq!(send_peer, Some(peer));
        assert_eq!(data, b"payload");
        let mut inbound = Vec::new();
        inbound.extend_from_slice(&DATA_INDICATION.to_be_bytes());
        inbound.extend_from_slice(&0u16.to_be_bytes());
        inbound.extend_from_slice(&MAGIC_COOKIE);
        inbound.extend_from_slice(&transaction);
        push_attribute(
            &mut inbound,
            ATTR_XOR_PEER_ADDRESS,
            &encode_xor_address(peer, &transaction),
        );
        push_attribute(&mut inbound, ATTR_DATA, b"payload");
        server
            .send_to(&inbound, client.mapped_receive_addr())
            .unwrap();
        let (from, got) = TurnClient::parse_data_indication(&inbound).unwrap();
        assert_eq!(from, peer);
        assert_eq!(got, b"payload");
    }

    impl TurnClient {
        fn mapped_receive_addr(&self) -> SocketAddr {
            self.socket.local_addr().unwrap()
        }
    }

    #[test]
    fn allocate_against_mock_server_with_rest_credentials() {
        let server = mock::MockTurnServer::start();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut client =
            TurnClient::allocate(socket, server.addr(), &server.username, &server.password)
                .expect("allocate");
        assert!(client.relayed_address().ip().is_loopback());
        let peer: SocketAddr = "198.51.100.4:40000".parse().unwrap();
        client.ensure_permission(peer).expect("permission");
        assert!(client
            .refresh_if_due(Instant::now() + Duration::from_secs(600))
            .unwrap());
        assert!(!client.refresh_if_due(Instant::now()).unwrap());
    }

    #[test]
    fn allocate_rejects_wrong_password() {
        let server = mock::MockTurnServer::start();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let result = TurnClient::allocate(socket, server.addr(), &server.username, "wrong");
        assert!(matches!(
            result,
            Err(TurnError::Rejected { code: 401 }) | Err(TurnError::Timeout)
        ));
    }

    #[test]
    fn two_allocations_relay_data_through_the_mock_server() {
        let server = mock::MockTurnServer::start();
        let socket_a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket_b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut a =
            TurnClient::allocate(socket_a, server.addr(), &server.username, &server.password)
                .unwrap();
        let mut b =
            TurnClient::allocate(socket_b, server.addr(), &server.username, &server.password)
                .unwrap();
        a.ensure_permission(b.relayed_address()).unwrap();
        b.ensure_permission(a.relayed_address()).unwrap();

        a.send_data(b.relayed_address(), b"hello over relay")
            .unwrap();
        let got = wait_for_data(&mut b);
        assert_eq!(got.0, a.relayed_address());
        assert_eq!(got.1, b"hello over relay");

        b.send_data(a.relayed_address(), b"reply").unwrap();
        let got = wait_for_data(&mut a);
        assert_eq!(got.0, b.relayed_address());
        assert_eq!(got.1, b"reply");
    }

    fn wait_for_data(client: &mut TurnClient) -> (SocketAddr, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(payload) = client.poll_data() {
                return payload;
            }
            let mut buffer = [0u8; 2048];
            client
                .socket
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            match client.socket.recv_from(&mut buffer) {
                Ok((length, source)) if source == client.server => {
                    client.absorb(&buffer[..length]);
                }
                _ => continue,
            }
        }
        panic!("no relayed data within deadline");
    }
}
