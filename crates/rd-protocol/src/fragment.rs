//! Experimental P2 envelope; not yet a negotiated wire format.
use std::collections::BTreeMap;
use bytes::{BufMut, Bytes, BytesMut};
use crate::{Frame, HEADER_LEN, MAX_PAYLOAD_LEN};

pub const CHUNK: usize = 12 * 1024;
pub const ENVELOPE: usize = 20;
pub const MAX_ACTIVE: usize = 4;
pub const MAX_BUFFERED: usize = 32 * 1024 * 1024;
pub const TTL_MS: u64 = 500;
const MAX_WIRE: usize = MAX_PAYLOAD_LEN as usize + HEADER_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FragmentError {
    #[error("invalid fragment or frame")] Invalid,
    #[error("reassembly capacity exceeded")] Capacity,
    #[error("expired or replayed frame")] Stale,
}

/// ID increases for each access unit; a new connection owns new ID state.
pub fn fragment(frame: &Frame, id: u64) -> Result<Vec<Bytes>, FragmentError> {
    if frame.payload.len() > MAX_PAYLOAD_LEN as usize || frame.payload.len() != frame.header.payload_len as usize {
        return Err(FragmentError::Invalid);
    }
    let wire = frame.encode();
    let count = wire.len().div_ceil(CHUNK);
    Ok(wire.chunks(CHUNK).enumerate().map(|(index, data)| {
        let mut out = BytesMut::with_capacity(ENVELOPE + data.len());
        out.extend_from_slice(b"RDF1");
        out.put_u64(id);
        out.put_u32(wire.len() as u32);
        out.put_u16(index as u16);
        out.put_u16(count as u16);
        out.extend_from_slice(data);
        out.freeze()
    }).collect())
}

struct Pending { born: u64, total: usize, parts: Vec<Option<Bytes>>, received: usize }

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
    pub fn buffered_bytes(&self) -> usize { self.reserved }
    pub fn peak_bytes(&self) -> usize { self.peak }
    /// Dropped frame count: caller must request a keyframe when nonzero.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let ids: Vec<_> = self.pending.iter().filter(|(_, p)| now_ms.saturating_sub(p.born) >= TTL_MS).map(|(&id, _)| id).collect();
        for id in &ids { self.retire(*id); }
        ids.len()
    }
    fn retire(&mut self, id: u64) {
        if let Some(p) = self.pending.remove(&id) { self.reserved -= p.total; }
        self.retired.insert(id, ());
        let floor = self.highest.unwrap_or(id).saturating_sub(63);
        self.retired.retain(|id, _| *id >= floor);
    }
    pub fn push(&mut self, packet: &[u8], now_ms: u64) -> Result<Option<Frame>, FragmentError> {
        if packet.len() < ENVELOPE || packet.len() > ENVELOPE + CHUNK || &packet[..4] != b"RDF1" { return Err(FragmentError::Invalid); }
        let id = u64::from_be_bytes(packet[4..12].try_into().unwrap());
        let total = u32::from_be_bytes(packet[12..16].try_into().unwrap()) as usize;
        let index = u16::from_be_bytes(packet[16..18].try_into().unwrap()) as usize;
        let count = u16::from_be_bytes(packet[18..20].try_into().unwrap()) as usize;
        if !(HEADER_LEN..=MAX_WIRE).contains(&total) || count != total.div_ceil(CHUNK) || index >= count || packet.len() - ENVELOPE != (total - index * CHUNK).min(CHUNK) { return Err(FragmentError::Invalid); }
        if self.retired.contains_key(&id) || self.highest.is_some_and(|h| id < h.saturating_sub(63)) { return Err(FragmentError::Stale); }
        if let Some(p) = self.pending.get(&id) {
            if now_ms.saturating_sub(p.born) >= TTL_MS { self.retire(id); return Err(FragmentError::Stale); }
            if p.total != total { self.retire(id); return Err(FragmentError::Invalid); }
        } else {
            if self.pending.len() >= MAX_ACTIVE || self.reserved + total > MAX_BUFFERED { return Err(FragmentError::Capacity); }
            self.highest = Some(self.highest.map_or(id, |h| h.max(id)));
            self.reserved += total;
            self.peak = self.peak.max(self.reserved);
            self.pending.insert(id, Pending { born: now_ms, total, parts: vec![None; count], received: 0 });
        }
        let p = self.pending.get_mut(&id).unwrap();
        let data = &packet[ENVELOPE..];
        if let Some(existing) = &p.parts[index] {
            if existing.as_ref() != data { self.retire(id); return Err(FragmentError::Invalid); }
            return Ok(None);
        }
        p.parts[index] = Some(Bytes::copy_from_slice(data));
        p.received += 1;
        if p.received != count { return Ok(None); }
        let p = self.pending.remove(&id).unwrap();
        self.reserved -= total;
        self.retire(id);
        let mut wire = BytesMut::with_capacity(total);
        for part in p.parts { wire.extend_from_slice(&part.unwrap()); }
        let frame = Frame::decode(&mut wire).map_err(|_| FragmentError::Invalid)?;
        if !wire.is_empty() { return Err(FragmentError::Invalid); }
        Ok(Some(frame))
    }
}
