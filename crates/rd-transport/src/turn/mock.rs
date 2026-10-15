//! In-process TURN server for tests: one UDP socket, allocations with
//! long-term credentials, permissions, refresh, and internal relaying of
//! Send/Data indications between allocations. Coturn parity is validated at
//! deployment time; this mock keeps the client logic testable offline.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha1::Sha1;

use super::{
    decode_xor_address, encode_xor_address, long_term_key, push_attribute, signed_prefix,
    transaction_of, update_message_length, walk_attributes, MAGIC_COOKIE,
};

const HEADER_LEN: usize = 20;
const ALLOCATE_REQUEST: u16 = 0x0003;
const ALLOCATE_SUCCESS: u16 = 0x0103;
const REFRESH_REQUEST: u16 = 0x0004;
const REFRESH_SUCCESS: u16 = 0x0104;
const PERMISSION_REQUEST: u16 = 0x0008;
const PERMISSION_SUCCESS: u16 = 0x0108;
const SEND_INDICATION: u16 = 0x0016;
const DATA_INDICATION: u16 = 0x0017;
const ERROR_RESPONSE: u16 = 0x0111;

const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_LIFETIME: u16 = 0x000d;
const ATTR_REALM: u16 = 0x0014;
const ATTR_NONCE: u16 = 0x0015;
const ATTR_XOR_PEER_ADDRESS: u16 = 0x0012;
const ATTR_DATA: u16 = 0x0013;
const ATTR_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

const REALM: &str = "mock.turn";
const EXPECTED_PASSWORD: &str = "mock-shared-secret-password";

struct Allocation {
    /// Synthetic relayed address; only meaningful inside this server.
    relayed: SocketAddr,
    permissions: Vec<SocketAddr>,
}

enum Action {
    Reply(Vec<u8>),
    RelayData {
        to_relayed: SocketAddr,
        from_relayed: SocketAddr,
        payload: Vec<u8>,
    },
    None,
}

pub struct MockTurnServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    pub username: String,
    pub password: String,
}

impl MockTurnServer {
    /// Fixed long-term credentials; tests configure the client with exactly
    /// these (mirroring coturn REST credential usage).
    pub fn start() -> Self {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind mock turn");
        let addr = socket.local_addr().expect("local addr");
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("read timeout");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handle = std::thread::Builder::new()
            .name("mock-turn".into())
            .spawn(move || run(socket, thread_stop))
            .expect("spawn mock turn thread");
        Self {
            addr,
            stop,
            handle: Some(handle),
            username: "1790000000:test-device".to_string(),
            password: EXPECTED_PASSWORD.to_string(),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for MockTurnServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(socket: std::net::UdpSocket, stop: Arc<AtomicBool>) {
    let mut allocations: HashMap<SocketAddr, Allocation> = HashMap::new();
    let mut next_relay_port: u16 = 40000;
    let mut buffer = [0u8; 2048];
    while !stop.load(Ordering::Acquire) {
        let (length, source) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => return,
        };
        match handle_datagram(
            &buffer[..length],
            source,
            &mut allocations,
            &mut next_relay_port,
        ) {
            Action::Reply(bytes) => {
                let _ = socket.send_to(&bytes, source);
            }
            Action::RelayData {
                to_relayed,
                from_relayed,
                payload,
            } => {
                if let Some((client, target)) = allocations
                    .iter()
                    .find(|(_, allocation)| allocation.relayed == to_relayed)
                {
                    // Permissions are enforced like coturn: no permission,
                    // no relay.
                    if target.permissions.contains(&from_relayed) {
                        let transaction = [7u8; 12];
                        let mut indication = Vec::with_capacity(HEADER_LEN + 48 + payload.len());
                        indication.extend_from_slice(&DATA_INDICATION.to_be_bytes());
                        indication.extend_from_slice(&0u16.to_be_bytes());
                        indication.extend_from_slice(&MAGIC_COOKIE);
                        indication.extend_from_slice(&transaction);
                        push_attribute(
                            &mut indication,
                            ATTR_XOR_PEER_ADDRESS,
                            &encode_xor_address(from_relayed, &transaction),
                        );
                        push_attribute(&mut indication, ATTR_DATA, &payload);
                        update_message_length(&mut indication);
                        let _ = socket.send_to(&indication, *client);
                    }
                }
            }
            Action::None => {}
        }
    }
}

fn handle_datagram(
    datagram: &[u8],
    source: SocketAddr,
    allocations: &mut HashMap<SocketAddr, Allocation>,
    next_relay_port: &mut u16,
) -> Action {
    if datagram.len() < HEADER_LEN {
        return Action::None;
    }
    let message_type = u16::from_be_bytes([datagram[0], datagram[1]]);
    let class = (((message_type >> 8) & 1) << 1) | ((message_type >> 4) & 1);
    let Some(transaction) = transaction_of(datagram) else {
        return Action::None;
    };

    if class == 1 {
        if message_type != SEND_INDICATION {
            return Action::None;
        }
        let mut peer = None;
        let mut data = None;
        if walk_attributes(datagram, |attribute, value| match attribute {
            ATTR_XOR_PEER_ADDRESS => peer = decode_xor_address(value, &transaction),
            ATTR_DATA => data = Some(value.to_vec()),
            _ => {}
        })
        .is_err()
        {
            return Action::None;
        }
        let (Some(to_relayed), Some(payload)) = (peer, data) else {
            return Action::None;
        };
        let Some(from_relayed) = allocations.get(&source).map(|a| a.relayed) else {
            return Action::None;
        };
        return Action::RelayData {
            to_relayed,
            from_relayed,
            payload,
        };
    }
    if class != 0 {
        return Action::None;
    }

    let mut username = None;
    let mut integrity = None;
    if walk_attributes(datagram, |attribute, value| match attribute {
        ATTR_USERNAME => username = Some(String::from_utf8_lossy(value).into_owned()),
        ATTR_MESSAGE_INTEGRITY => integrity = Some(value.to_vec()),
        _ => {}
    })
    .is_err()
    {
        return Action::None;
    }
    let authenticated = match (&username, &integrity) {
        (Some(username), Some(integrity)) => {
            let key = long_term_key(username, REALM, EXPECTED_PASSWORD);
            let mut mac = Hmac::<Sha1>::new_from_slice(&key).expect("hmac key");
            mac.update(&signed_prefix(datagram));
            mac.verify_slice(integrity).is_ok()
        }
        _ => false,
    };

    let respond = |message_type: u16, attributes: &[(u16, Vec<u8>)]| -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(&message_type.to_be_bytes());
        message.extend_from_slice(&0u16.to_be_bytes());
        message.extend_from_slice(&MAGIC_COOKIE);
        message.extend_from_slice(&transaction);
        for (attribute, value) in attributes {
            push_attribute(&mut message, *attribute, value);
        }
        update_message_length(&mut message);
        message
    };
    let error = |code: u16, with_auth: bool| -> Vec<u8> {
        let mut attributes = vec![(
            ATTR_ERROR_CODE,
            [0u8, 0, ((code / 100) as u8) << 5, (code % 100) as u8].to_vec(),
        )];
        if with_auth {
            attributes.push((ATTR_REALM, REALM.as_bytes().to_vec()));
            attributes.push((ATTR_NONCE, b"mock-nonce".to_vec()));
        }
        respond(ERROR_RESPONSE, &attributes)
    };
    let xor_address =
        |address: SocketAddr| -> Vec<u8> { encode_xor_address(address, &transaction) };

    match message_type {
        ALLOCATE_REQUEST => {
            if !authenticated {
                return Action::Reply(error(401, true));
            }
            let relayed = SocketAddr::from((IpAddr::V4([127, 0, 0, 1].into()), *next_relay_port));
            *next_relay_port += 1;
            allocations.insert(
                source,
                Allocation {
                    relayed,
                    permissions: Vec::new(),
                },
            );
            Action::Reply(respond(
                ALLOCATE_SUCCESS,
                &[
                    (ATTR_XOR_RELAYED_ADDRESS, xor_address(relayed)),
                    (ATTR_XOR_MAPPED_ADDRESS, xor_address(source)),
                    (ATTR_LIFETIME, 600u32.to_be_bytes().to_vec()),
                ],
            ))
        }
        PERMISSION_REQUEST => {
            if !authenticated {
                return Action::Reply(error(401, true));
            }
            let mut peer = None;
            if walk_attributes(datagram, |attribute, value| {
                if attribute == ATTR_XOR_PEER_ADDRESS {
                    peer = decode_xor_address(value, &transaction);
                }
            })
            .is_err()
            {
                return Action::None;
            }
            if let Some(allocation) = allocations.get_mut(&source) {
                if let Some(peer) = peer {
                    if !allocation.permissions.contains(&peer) {
                        allocation.permissions.push(peer);
                    }
                }
            }
            Action::Reply(respond(PERMISSION_SUCCESS, &[]))
        }
        REFRESH_REQUEST => {
            if !authenticated {
                return Action::Reply(error(401, true));
            }
            Action::Reply(respond(
                REFRESH_SUCCESS,
                &[(ATTR_LIFETIME, 600u32.to_be_bytes().to_vec())],
            ))
        }
        _ => Action::None,
    }
}
