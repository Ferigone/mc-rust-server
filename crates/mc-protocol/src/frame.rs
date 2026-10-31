//! Packet framing: VarInt length, optional zlib compression and AES-128-CFB8 encryption.
//!
//! Frame (no compression):   VarInt(len) | VarInt(id) | body
//! Frame (compression):      VarInt(packet_len) | VarInt(data_len) | data
//!                           data_len == 0 -> uncompressed data (below the threshold)

use crate::buf::{Put, Reader};
use crate::error::{Error, Result};
use aes::Aes128;
use cfb8::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

/// Maximum packet size after decompression (vanilla: 8 MiB for serverbound).
pub const MAX_UNCOMPRESSED: usize = 8 * 1024 * 1024;
/// Maximum frame size on the wire (3-byte VarInt, like vanilla).
pub const MAX_FRAME: usize = 2_097_151;

type Dec = cfb8::Decryptor<Aes128>;
type Enc = cfb8::Encryptor<Aes128>;

/// Stream decoder: takes raw bytes from the socket and yields complete `(id, body)` packets.
pub struct FrameDecoder {
    buf: Vec<u8>,
    start: usize,
    threshold: Option<i32>,
    cipher: Option<Dec>,
    inflater: Decompress,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(4096), start: 0, threshold: None, cipher: None, inflater: Decompress::new(true) }
    }

    pub fn enable_encryption(&mut self, key: &[u8; 16]) {
        // Note: bytes already buffered are plain text; encryption is enabled between packets.
        self.cipher = Some(Dec::new(key.into(), key.into()));
    }

    pub fn set_compression(&mut self, threshold: i32) {
        self.threshold = if threshold >= 0 { Some(threshold) } else { None };
    }

    /// Appends fresh bytes from the socket (decrypting them if encryption is enabled).
    pub fn feed(&mut self, data: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > 65536 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        let from = self.buf.len();
        self.buf.extend_from_slice(data);
        if let Some(c) = self.cipher.as_mut() {
            for b in &mut self.buf[from..] {
                c.decrypt_block_mut(std::slice::from_mut(b).into());
            }
        }
    }

    /// Tries to extract the next complete packet. `Ok(None)` = more data is needed.
    pub fn next_packet(&mut self) -> Result<Option<(i32, Vec<u8>)>> {
        let avail = &self.buf[self.start..];
        // Frame length
        let mut r = Reader::new(avail);
        let len = match r.varint() {
            Ok(l) => l,
            Err(Error::Eof) => return Ok(None),
            Err(e) => return Err(e),
        };
        if len < 0 || len as usize > MAX_UNCOMPRESSED {
            return Err(Error::TooLarge(len.max(0) as usize));
        }
        let len = len as usize;
        let hdr = avail.len() - r.remaining();
        if r.remaining() < len {
            return Ok(None);
        }
        let payload = &avail[hdr..hdr + len];
        let packet = if self.threshold.is_some() {
            let mut pr = Reader::new(payload);
            let data_len = pr.varint()?;
            if data_len == 0 {
                pr.rest().to_vec()
            } else {
                if data_len < 0 || data_len as usize > MAX_UNCOMPRESSED {
                    return Err(Error::TooLarge(data_len.max(0) as usize));
                }
                // A single call limited to the declared size (+1 to detect overflow): resistant to zip bombs.
                let mut out = Vec::with_capacity(data_len as usize + 1);
                self.inflater.reset(true);
                let status = self
                    .inflater
                    .decompress_vec(pr.rest(), &mut out, FlushDecompress::Finish)
                    .map_err(|e| Error::Decompress(e.to_string()))?;
                if !matches!(status, Status::StreamEnd) {
                    return Err(Error::Decompress("stream did not end within declared size".into()));
                }
                if out.len() != data_len as usize {
                    return Err(Error::Decompress(format!("size mismatch: {} != {}", out.len(), data_len)));
                }
                out
            }
        } else {
            payload.to_vec()
        };
        self.start += hdr + len;

        let mut pr = Reader::new(&packet);
        let id = pr.varint()?;
        let body_start = packet.len() - pr.remaining();
        let mut packet = packet;
        packet.drain(..body_start);
        Ok(Some((id, packet)))
    }
}

/// Encoder: builds a ready-to-send frame from a payload (`VarInt(id) | body`).
/// Compression is independent of the connection (global threshold), so frames can be shared
/// between players; encryption is done only by the writer of a particular connection.
pub struct FrameEncoder {
    threshold: Option<i32>,
    deflater: Compress,
    scratch: Vec<u8>,
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new(None)
    }
}

impl FrameEncoder {
    pub fn new(threshold: Option<i32>) -> Self {
        Self { threshold, deflater: Compress::new(Compression::default(), true), scratch: Vec::new() }
    }

    pub fn threshold(&self) -> Option<i32> {
        self.threshold
    }

    /// Appends a frame for `payload` (id+body) to `out`.
    pub fn encode_into(&mut self, payload: &[u8], out: &mut Vec<u8>) {
        match self.threshold {
            None => {
                out.put_varint(payload.len() as i32);
                out.extend_from_slice(payload);
            }
            Some(t) if (payload.len() as i64) < t as i64 => {
                out.put_varint(payload.len() as i32 + 1);
                out.put_varint(0);
                out.extend_from_slice(payload);
            }
            Some(_) => {
                self.scratch.clear();
                self.deflater.reset();
                self.scratch.reserve(payload.len() / 2 + 64);
                loop {
                    let consumed = self.deflater.total_in() as usize;
                    let st = self
                        .deflater
                        .compress_vec(&payload[consumed..], &mut self.scratch, FlushCompress::Finish)
                        .expect("deflate cannot fail");
                    if matches!(st, Status::StreamEnd) {
                        break;
                    }
                    self.scratch.reserve(4096);
                }
                let mut dl = Vec::with_capacity(5);
                dl.put_varint(payload.len() as i32);
                out.put_varint((dl.len() + self.scratch.len()) as i32);
                out.extend_from_slice(&dl);
                out.extend_from_slice(&self.scratch);
            }
        }
    }

    pub fn encode(&mut self, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(payload.len() + 8);
        self.encode_into(payload, &mut v);
        v
    }
}

/// Outgoing AES-128-CFB8 encryptor (state is kept between writes).
pub struct Encryptor(Enc);

impl Encryptor {
    pub fn new(key: &[u8; 16]) -> Self {
        Self(Enc::new(key.into(), key.into()))
    }
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for b in data {
            self.0.encrypt_block_mut(std::slice::from_mut(b).into());
        }
    }
}

/// Builds a packet payload: `VarInt(id) | body`.
pub fn payload(id: i32, body: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut v = Vec::with_capacity(64);
    v.put_varint(id);
    body(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(threshold: Option<i32>, n: usize) {
        let mut enc = FrameEncoder::new(threshold);
        let mut dec = FrameDecoder::new();
        if let Some(t) = threshold {
            dec.set_compression(t);
        }
        let body: Vec<u8> = (0..n).map(|i| (i % 7) as u8).collect();
        let p = payload(0x27, |w| w.extend_from_slice(&body));
        let frame = enc.encode(&p);
        // feed it in tiny chunks to exercise partial-frame handling
        let mut got = None;
        for chunk in frame.chunks(3) {
            dec.feed(chunk);
            if let Some(x) = dec.next_packet().unwrap() {
                got = Some(x);
            }
        }
        let (id, b) = got.expect("packet");
        assert_eq!(id, 0x27);
        assert_eq!(b, body);
    }

    #[test]
    fn plain() {
        roundtrip(None, 0);
        roundtrip(None, 1000);
    }

    #[test]
    fn compressed_below_and_above_threshold() {
        roundtrip(Some(256), 10);
        roundtrip(Some(256), 255);
        roundtrip(Some(256), 5000);
        roundtrip(Some(0), 50);
    }

    #[test]
    fn encryption_roundtrip() {
        let key = [7u8; 16];
        let mut enc = FrameEncoder::new(Some(64));
        let mut e = Encryptor::new(&key);
        let mut dec = FrameDecoder::new();
        dec.set_compression(64);
        dec.enable_encryption(&key);
        for n in [3usize, 300, 70000] {
            let body = vec![0xABu8; n];
            let mut frame = enc.encode(&payload(1, |w| w.extend_from_slice(&body)));
            e.encrypt(&mut frame);
            dec.feed(&frame);
            let (id, b) = dec.next_packet().unwrap().unwrap();
            assert_eq!((id, b), (1, body));
        }
    }
}
