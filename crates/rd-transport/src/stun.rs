//! Minimal STUN Binding client for server-reflexive candidate gathering.
//!
//! str0m deliberately leaves candidate gathering to the application
//! ("NIC enumeration and TURN"), so this module provides just enough
//! RFC 5389 to learn the mapped address behind a NAT from the same UDP
//! socket the ICE agent will use.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
const HEADER_LEN: usize = 20;
const TYPE_BINDING_REQUEST: u16 = 0x0001;
const TYPE_BINDING_SUCCESS: u16 = 0x0101;
const TYPE_BINDING_ERROR: u16 = 0x0111;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const FAMILY_IPV4: u8 = 0x01;

const RETRANSMIT_DELAYS: [Duration; 4] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
];

#[derive(Debug, thiserror::Error)]
pub enum StunError {
    #[error("stun server did not answer in time")]
    Timeout,
    #[error("malformed stun response: {0}")]
    Malformed(&'static str),
    #[error("stun error response (code {0})")]
    Server(u16),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 96-bit transaction id; the first 4 bytes double as the XOR mask tail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TransactionId(pub [u8; 12]);

impl TransactionId {
    /// Deterministic ids keep unit tests reproducible; callers pass random ids.
    pub fn from_seed(seed: u64) -> Self {
        let mut id = [0u8; 12];
        id[..8].copy_from_slice(&seed.to_be_bytes());
        Self(id)
    }

    fn xor_mask(&self) -> [u8; 4] {
        let mut mask = MAGIC_COOKIE;
        for (byte, seed) in mask.iter_mut().zip(self.0.iter()) {
            *byte ^= *seed;
        }
        mask
    }
}

pub fn encode_binding_request(transaction: TransactionId) -> Vec<u8> {
    let mut message = Vec::with_capacity(HEADER_LEN);
    message.extend_from_slice(&TYPE_BINDING_REQUEST.to_be_bytes());
    message.extend_from_slice(&0u16.to_be_bytes()); // no attributes
    message.extend_from_slice(&MAGIC_COOKIE);
    message.extend_from_slice(&transaction.0);
    message
}

/// Decode one address attribute value: `[reserved, family, port:2, addr]`.
/// XOR-protected values use the cookie|transaction mask for port and address.
fn decode_address(value: &[u8], mask: Option<&[u8; 4]>) -> Option<SocketAddr> {
    if value.len() < 8 || value[1] != FAMILY_IPV4 {
        // v1 only gathers IPv4 reflexive addresses.
        return None;
    }
    let raw_port = u16::from_be_bytes([value[2], value[3]]);
    let address = <[u8; 4]>::try_from(&value[4..8]).ok()?;
    let (port, octets) = match mask {
        Some(mask) => (
            raw_port ^ u16::from_be_bytes([mask[0], mask[1]]),
            [
                address[0] ^ mask[0],
                address[1] ^ mask[1],
                address[2] ^ mask[2],
                address[3] ^ mask[3],
            ],
        ),
        None => (raw_port, address),
    };
    Some(SocketAddr::from((
        std::net::IpAddr::V4(octets.into()),
        port,
    )))
}

/// Parse a Binding success response and return the mapped address, preferring
/// XOR-MAPPED-ADDRESS over the legacy MAPPED-ADDRESS attribute.
pub fn parse_binding_response(
    response: &[u8],
    expected: TransactionId,
) -> Result<SocketAddr, StunError> {
    if response.len() < HEADER_LEN {
        return Err(StunError::Malformed("shorter than header"));
    }
    let message_type = u16::from_be_bytes([response[0], response[1]]);
    let transaction = TransactionId(<[u8; 12]>::try_from(&response[8..20]).expect("txid bytes"));
    if transaction != expected {
        return Err(StunError::Malformed("transaction id mismatch"));
    }
    if message_type == TYPE_BINDING_ERROR {
        return Err(StunError::Server(message_type));
    }
    if message_type != TYPE_BINDING_SUCCESS {
        return Err(StunError::Malformed("not a binding success response"));
    }
    let mask = expected.xor_mask();
    let mut attributes = &response[HEADER_LEN..];
    let mut xored = None;
    let mut plain = None;
    while attributes.len() >= 4 {
        let attribute = u16::from_be_bytes([attributes[0], attributes[1]]);
        let length = usize::from(u16::from_be_bytes([attributes[2], attributes[3]]));
        let value = attributes
            .get(4..4 + length)
            .ok_or(StunError::Malformed("attribute overruns message"))?;
        match attribute {
            ATTR_XOR_MAPPED_ADDRESS if xored.is_none() => {
                xored = decode_address(value, Some(&mask));
            }
            ATTR_MAPPED_ADDRESS if plain.is_none() => {
                plain = decode_address(value, None);
            }
            _ => {}
        }
        // Attributes are padded to 4-byte boundaries.
        let padded = (length + 3) & !3;
        attributes = attributes
            .get(4 + padded..)
            .ok_or(StunError::Malformed("padding overruns message"))?;
    }
    // RFC 5389: agents should prefer XOR-MAPPED-ADDRESS when both exist.
    xored
        .or(plain)
        .ok_or(StunError::Malformed("no mapped address attribute"))
}

/// Send Binding requests over `socket` until the server answers or the
/// retransmission budget runs out. The mapped address is only useful for ICE
/// when this is the same socket the agent later uses.
pub fn request_mapped_address(
    socket: &UdpSocket,
    server: SocketAddr,
) -> Result<SocketAddr, StunError> {
    let transaction = TransactionId::from_seed(rand_transaction_seed());
    let request = encode_binding_request(transaction);
    let mut response = vec![0u8; 576];
    for delay in RETRANSMIT_DELAYS {
        socket.send_to(&request, server)?;
        let deadline = std::time::Instant::now() + delay;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            socket.set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;
            match socket.recv_from(&mut response) {
                Ok((length, source)) if source == server => {
                    return parse_binding_response(&response[..length], transaction);
                }
                Ok(_) => continue, // stray packet from another peer
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Err(StunError::Timeout)
}

/// Cheap per-call randomness from the OS clock; transaction ids only need
/// uniqueness per socket, not unpredictability.
fn rand_transaction_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default();
    nanos.rotate_left(17) ^ (std::process::id() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a success response with one XOR-MAPPED-ADDRESS for the given
    /// clear-text address, mirroring what a STUN server produces.
    fn success_with_xor_mapped(transaction: TransactionId, address: SocketAddr) -> Vec<u8> {
        let mask = transaction.xor_mask();
        let octets = match address.ip() {
            std::net::IpAddr::V4(address) => address.octets(),
            std::net::IpAddr::V6(_) => unreachable!("ipv4 only"),
        };
        let port = address.port() ^ u16::from_be_bytes([mask[0], mask[1]]);
        let mut response = encode_binding_request(transaction);
        response[0..2].copy_from_slice(&TYPE_BINDING_SUCCESS.to_be_bytes());
        response[2..4].copy_from_slice(&8u16.to_be_bytes());
        response.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&8u16.to_be_bytes());
        response.extend_from_slice(&[0x00, FAMILY_IPV4]);
        response.extend_from_slice(&port.to_be_bytes());
        for (byte, key) in octets.iter().zip(mask.iter()) {
            response.push(byte ^ key);
        }
        response
    }

    #[test]
    fn binding_request_wire_format() {
        let transaction = TransactionId::from_seed(0x0102_0304_0506_0708);
        let request = encode_binding_request(transaction);
        assert_eq!(request.len(), HEADER_LEN);
        assert_eq!(&request[..2], &[0x00, 0x01]);
        assert_eq!(&request[4..8], &MAGIC_COOKIE);
        assert_eq!(&request[8..], &transaction.0);
    }

    #[test]
    fn parses_xor_mapped_ipv4() {
        let transaction = TransactionId::from_seed(0x1122_3344_5566_7788);
        let expected: SocketAddr = "192.0.2.1:32853".parse().unwrap();
        let response = success_with_xor_mapped(transaction, expected);
        assert_eq!(
            parse_binding_response(&response, transaction).unwrap(),
            expected
        );
    }

    #[test]
    fn prefers_xor_and_falls_back_to_plain_mapped() {
        let transaction = TransactionId::from_seed(9);
        let mut response = encode_binding_request(transaction);
        response[0..2].copy_from_slice(&TYPE_BINDING_SUCCESS.to_be_bytes());
        response[2..4].copy_from_slice(&12u16.to_be_bytes());
        // Legacy MAPPED-ADDRESS first, XOR variant second: XOR must win.
        response.extend_from_slice(&ATTR_MAPPED_ADDRESS.to_be_bytes());
        response.extend_from_slice(&8u16.to_be_bytes());
        response.extend_from_slice(&[0x00, FAMILY_IPV4]);
        response.extend_from_slice(&1234u16.to_be_bytes());
        response.extend_from_slice(&[127, 0, 0, 1]);
        let expected: SocketAddr = "198.51.100.7:5555".parse().unwrap();
        let xor = success_with_xor_mapped(transaction, expected);
        response.extend_from_slice(&xor[HEADER_LEN..]);
        assert_eq!(
            parse_binding_response(&response, transaction).unwrap(),
            expected
        );

        // Plain-only response decodes without a mask.
        let mut plain = encode_binding_request(transaction);
        plain[0..2].copy_from_slice(&TYPE_BINDING_SUCCESS.to_be_bytes());
        plain[2..4].copy_from_slice(&8u16.to_be_bytes());
        plain.extend_from_slice(&ATTR_MAPPED_ADDRESS.to_be_bytes());
        plain.extend_from_slice(&8u16.to_be_bytes());
        plain.extend_from_slice(&[0x00, FAMILY_IPV4]);
        plain.extend_from_slice(&1234u16.to_be_bytes());
        plain.extend_from_slice(&[10, 1, 2, 3]);
        assert_eq!(
            parse_binding_response(&plain, transaction).unwrap(),
            "10.1.2.3:1234".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn rejects_foreign_transaction_and_errors() {
        let transaction = TransactionId::from_seed(1);
        let response = encode_binding_request(transaction);
        assert!(matches!(
            parse_binding_response(&response, TransactionId::from_seed(2)),
            Err(StunError::Malformed(_))
        ));
        let mut error_response = encode_binding_request(transaction);
        error_response[0..2].copy_from_slice(&TYPE_BINDING_ERROR.to_be_bytes());
        assert!(matches!(
            parse_binding_response(&error_response, transaction),
            Err(StunError::Server(_))
        ));
    }

    #[test]
    fn live_request_times_out_without_server() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        // 203.0.113.0/24 is TEST-NET-3; nothing answers there on loopback
        // routing, so the retransmission budget runs out quickly.
        let server: SocketAddr = "203.0.113.1:3478".parse().unwrap();
        let started = std::time::Instant::now();
        let result = request_mapped_address(&socket, server);
        assert!(matches!(
            result,
            Err(StunError::Timeout) | Err(StunError::Io(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn gathers_mapped_address_from_mock_stun_server() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_addr = server.local_addr().unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let expected = socket.local_addr().unwrap();
        let responder = std::thread::spawn(move || {
            let mut buffer = vec![0u8; 576];
            let (_, client) = server.recv_from(&mut buffer).unwrap();
            let mut transaction = [0u8; 12];
            transaction.copy_from_slice(&buffer[8..20]);
            let response = success_with_xor_mapped(TransactionId(transaction), client);
            server.send_to(&response, client).unwrap();
        });
        let mapped = request_mapped_address(&socket, server_addr).unwrap();
        responder.join().unwrap();
        // On loopback the mapped address is the socket's own address.
        assert_eq!(mapped, expected);
    }
}
