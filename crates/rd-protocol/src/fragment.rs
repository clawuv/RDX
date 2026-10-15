//! Experimental P2 envelope; not yet a negotiated wire format.
use crate::{Frame, HEADER_LEN, MAX_PAYLOAD_LEN};
use bytes::{BufMut, Bytes, BytesMut};
use std::collections::BTreeMap;

pub const CHUNK: usize = 12 * 1024;
pub const ENVELOPE: usize = 21;
pub const MAX_ACTIVE: usize = 4;
pub const MAX_BUFFERED: usize = 32 * 1024 * 1024;
pub const TTL_MS: u64 = 500;
const MAX_WIRE: usize = MAX_PAYLOAD_LEN as usize + HEADER_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FragmentError {
    #[error("invalid fragment or frame")]
    Invalid,
    #[error("reassembly capacity exceeded")]
    Capacity,
    #[error("expired or replayed frame")]
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(size: usize) -> Frame {
        Frame::new(7, u16::MAX, true, Bytes::from(vec![37; size]))
    }
    #[test]
    fn reversed_duplicate_and_replay() {
        let original = frame(CHUNK * 2);
        let packets = fragment(&original, 5).unwrap();
        let mut r = Reassembler::default();
        assert!(r.push(&packets[2], 0).unwrap().is_none());
        assert!(r.push(&packets[2], 1).unwrap().is_none());
        assert!(r.push(&packets[1], 2).unwrap().is_none());
        let got = r.push(&packets[0], 3).unwrap().unwrap();
        assert_eq!(got.header, original.header);
        assert_eq!(got.payload, original.payload);
        assert_eq!(r.buffered_bytes(), 0);
        assert!(matches!(r.push(&packets[0], 4), Err(FragmentError::Stale)));
    }
    #[test]
    fn loss_expiry_and_reconnect() {
        let packets = fragment(&frame(CHUNK), 9).unwrap();
        let mut r = Reassembler::default();
        r.push(&packets[0], 0).unwrap();
        assert_eq!(r.expire(TTL_MS - 1), 0);
        assert_eq!(r.expire(TTL_MS), 1);
        assert_eq!(r.buffered_bytes(), 0);
        assert!(matches!(
            r.push(&packets[1], TTL_MS),
            Err(FragmentError::Stale)
        ));
        let mut fresh = Reassembler::default();
        fresh.push(&packets[0], 0).unwrap();
        assert!(fresh.push(&packets[1], 1).unwrap().is_some());
    }
    #[test]
    fn maximum_frame_and_capacity() {
        let original = frame(MAX_PAYLOAD_LEN as usize);
        let packets = fragment(&original, 0).unwrap();
        let mut r = Reassembler::default();
        r.push(&packets[0], 0).unwrap();
        let mut second = packets[0].to_vec();
        second[4..12].copy_from_slice(&1u64.to_be_bytes());
        assert!(matches!(r.push(&second, 1), Err(FragmentError::Capacity)));
        assert!(r.peak_bytes() <= MAX_BUFFERED);
        second[13..17].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(r.push(&second, 1), Err(FragmentError::Invalid)));
        let mut got = None;
        for packet in &packets[1..] {
            got = r.push(packet, 2).unwrap();
        }
        assert_eq!(got.unwrap().payload, original.payload);
        assert_eq!(r.buffered_bytes(), 0);
        let mut bad = frame(10);
        bad.header.payload_len = 11;
        assert!(fragment(&bad, 1).is_err());
    }
    #[test]
    fn concurrent_cap_conflict_and_sequence_wrap() {
        let mut r = Reassembler::default();
        for id in 0..MAX_ACTIVE as u64 {
            r.push(&fragment(&frame(CHUNK), id).unwrap()[0], 0).unwrap();
        }
        assert!(matches!(
            r.push(&fragment(&frame(CHUNK), 4).unwrap()[0], 1),
            Err(FragmentError::Capacity)
        ));
        let mut conflict = fragment(&frame(CHUNK), 0).unwrap()[0].to_vec();
        conflict[ENVELOPE] ^= 1;
        assert!(matches!(r.push(&conflict, 2), Err(FragmentError::Invalid)));
        let mut r = Reassembler::default();
        for (id, seq) in [(65535, u16::MAX), (65536, 0)] {
            let mut f = frame(0);
            f.header.seq = seq;
            let got = r.push(&fragment(&f, id).unwrap()[0], 0).unwrap().unwrap();
            assert_eq!(got.header.seq, seq);
        }
    }
}

/// ID increases for each access unit; a new connection owns new ID state.
pub fn fragment(frame: &Frame, id: u64) -> Result<Vec<Bytes>, FragmentError> {
    if frame.payload.len() > MAX_PAYLOAD_LEN as usize
        || frame.payload.len() != frame.header.payload_len as usize
    {
        return Err(FragmentError::Invalid);
    }
    let wire = frame.encode();
    let count = wire.len().div_ceil(CHUNK);
    Ok(wire
        .chunks(CHUNK)
        .enumerate()
        .map(|(index, data)| {
            let mut out = BytesMut::with_capacity(ENVELOPE + data.len());
            out.extend_from_slice(b"RDF1");
            out.put_u64(id);
            out.put_u8(frame.header.stream_id);
            out.put_u32(wire.len() as u32);
            out.put_u16(index as u16);
            out.put_u16(count as u16);
            out.extend_from_slice(data);
            out.freeze()
        })
        .collect())
}

struct Pending {
    born: u64,
    stream_id: u8,
    total: usize,
    parts: Vec<Option<Bytes>>,
    received: usize,
}

/// Caller supplies monotonic milliseconds and calls expire even during silence.
#[derive(Default)]
pub struct Reassembler {
    pending: BTreeMap<u64, Pending>,
    reserved: usize,
    peak: usize,
    highest: Option<u64>,
    retired: BTreeMap<u64, ()>,
}
impl Reassembler {
    pub fn buffered_bytes(&self) -> usize {
        self.reserved
    }
    pub fn peak_bytes(&self) -> usize {
        self.peak
    }
    /// Dropped frame count: caller must request a keyframe when nonzero.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        self.expire_requests(now_ms).len()
    }

    /// IDs whose partial assembly expired; use these to request recovery.
    pub fn expire_ids(&mut self, now_ms: u64) -> Vec<u64> {
        self.expire_requests(now_ms)
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }

    pub fn expire_requests(&mut self, now_ms: u64) -> Vec<(u8, u64)> {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| now_ms.saturating_sub(p.born) >= TTL_MS)
            .map(|(&id, p)| (p.stream_id, id))
            .collect();
        for (_, id) in &ids {
            self.retire(*id);
        }
        ids
    }
    fn retire(&mut self, id: u64) {
        if let Some(p) = self.pending.remove(&id) {
            self.reserved -= p.total;
        }
        self.retired.insert(id, ());
        let floor = self.highest.unwrap_or(id).saturating_sub(63);
        self.retired.retain(|id, _| *id >= floor);
    }
    pub fn push(&mut self, packet: &[u8], now_ms: u64) -> Result<Option<Frame>, FragmentError> {
        if packet.len() < ENVELOPE || packet.len() > ENVELOPE + CHUNK || &packet[..4] != b"RDF1" {
            return Err(FragmentError::Invalid);
        }
        let id = u64::from_be_bytes(packet[4..12].try_into().unwrap());
        let stream_id = packet[12];
        let total = u32::from_be_bytes(packet[13..17].try_into().unwrap()) as usize;
        let index = u16::from_be_bytes(packet[17..19].try_into().unwrap()) as usize;
        let count = u16::from_be_bytes(packet[19..21].try_into().unwrap()) as usize;
        if !(HEADER_LEN..=MAX_WIRE).contains(&total)
            || count != total.div_ceil(CHUNK)
            || index >= count
            || packet.len() - ENVELOPE != (total - index * CHUNK).min(CHUNK)
        {
            return Err(FragmentError::Invalid);
        }
        if self.retired.contains_key(&id) || self.highest.is_some_and(|h| id < h.saturating_sub(63))
        {
            return Err(FragmentError::Stale);
        }
        if let Some(p) = self.pending.get(&id) {
            if now_ms.saturating_sub(p.born) >= TTL_MS {
                self.retire(id);
                return Err(FragmentError::Stale);
            }
            if p.total != total {
                self.retire(id);
                return Err(FragmentError::Invalid);
            }
            if p.stream_id != stream_id {
                self.retire(id);
                return Err(FragmentError::Invalid);
            }
        } else {
            if self.pending.len() >= MAX_ACTIVE || self.reserved + total > MAX_BUFFERED {
                return Err(FragmentError::Capacity);
            }
            self.highest = Some(self.highest.map_or(id, |h| h.max(id)));
            self.reserved += total;
            self.peak = self.peak.max(self.reserved);
            self.pending.insert(
                id,
                Pending {
                    born: now_ms,
                    stream_id,
                    total,
                    parts: vec![None; count],
                    received: 0,
                },
            );
        }
        let p = self.pending.get_mut(&id).unwrap();
        let data = &packet[ENVELOPE..];
        if let Some(existing) = &p.parts[index] {
            if existing.as_ref() != data {
                self.retire(id);
                return Err(FragmentError::Invalid);
            }
            return Ok(None);
        }
        p.parts[index] = Some(Bytes::copy_from_slice(data));
        p.received += 1;
        if p.received != count {
            return Ok(None);
        }
        let p = self.pending.remove(&id).unwrap();
        self.reserved -= total;
        let stream_id = p.stream_id;
        self.retire(id);
        let mut wire = BytesMut::with_capacity(total);
        for part in p.parts {
            wire.extend_from_slice(&part.unwrap());
        }
        let frame = Frame::decode(&mut wire).map_err(|_| FragmentError::Invalid)?;
        if !wire.is_empty() || frame.header.stream_id != stream_id {
            return Err(FragmentError::Invalid);
        }
        Ok(Some(frame))
    }
}
