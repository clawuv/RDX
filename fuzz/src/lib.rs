use bytes::BytesMut;
use rd_protocol::control::ControlMessage;
use rd_protocol::fragment::{Reassembler, MAX_BUFFERED, TTL_MS};
use rd_protocol::{Frame, FrameHeader, HEADER_LEN, MAX_PAYLOAD_LEN};

/// Exercise raw parsers and a sequence of length-prefixed reassembly operations.
/// Per operation: advance:u8 (4ms ticks), length:u16 BE, packet:[u8; length].
pub fn exercise(data: &[u8]) {
    let _ = FrameHeader::peek(data);
    let mut wire = BytesMut::from(data);
    match Frame::decode(&mut wire) {
        Ok(frame) => {
            let consumed = HEADER_LEN + frame.payload.len();
            assert_eq!(frame.header.payload_len as usize, frame.payload.len());
            assert!(frame.payload.len() <= MAX_PAYLOAD_LEN as usize);
            assert_eq!(frame.encode().as_ref(), &data[..consumed]);
            assert_eq!(wire.as_ref(), &data[consumed..]);
        }
        Err(_) => assert_eq!(wire.as_ref(), data),
    }
    if let Ok(message) = ControlMessage::decode_wire(data) {
        let encoded = message.encode();
        assert_eq!(ControlMessage::decode_wire(&encoded).unwrap(), message);
    }

    let mut reassembler = Reassembler::default();
    let mut now = 0u64;
    let mut operations = data;
    while operations.len() >= 3 {
        now += u64::from(operations[0]) * 4;
        let length = usize::from(u16::from_be_bytes([operations[1], operations[2]]));
        operations = &operations[3..];
        let length = length.min(operations.len());
        reassembler.expire(now);
        if let Ok(Some(frame)) = reassembler.push(&operations[..length], now) {
            let mut encoded = BytesMut::from(frame.encode().as_ref());
            let decoded = Frame::decode(&mut encoded).unwrap();
            assert!(encoded.is_empty());
            assert_eq!(decoded.header, frame.header);
            assert_eq!(decoded.payload, frame.payload);
        }
        assert!(reassembler.buffered_bytes() <= MAX_BUFFERED);
        assert!(reassembler.peak_bytes() <= MAX_BUFFERED);
        operations = &operations[length..];
    }
    reassembler.expire(now + TTL_MS);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_seeds_and_mutations() {
        for seed in [
            include_bytes!("../corpus/protocol/frame").as_slice(),
            include_bytes!("../corpus/protocol/control").as_slice(),
            include_bytes!("../corpus/protocol/fragments").as_slice(),
        ] {
            exercise(seed);
            for index in 0..seed.len() {
                let mut changed = seed.to_vec();
                changed[index] ^= 0xff;
                exercise(&changed);
                exercise(&seed[..index]);
            }
        }
    }
}
