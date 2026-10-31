//! Ramkowanie pakietów: długość VarInt, opcjonalna kompresja zlib i szyfrowanie AES-128-CFB8.
//!
//! Ramka (bez kompresji):  VarInt(len) | VarInt(id) | body
//! Ramka (z kompresją):    VarInt(packet_len) | VarInt(data_len) | dane
//!                         data_len == 0 -> dane niezkompresowane (poniżej progu)

use crate::buf::{Put, Reader};
use crate::error::{Error, Result};
use aes::Aes128;
use cfb8::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

/// Maksymalny rozmiar pakietu po dekompresji (vanilla: 8 MiB dla serverbound).
pub const MAX_UNCOMPRESSED: usize = 8 * 1024 * 1024;
/// Maksymalny rozmiar ramki na drucie (3-bajtowy VarInt, jak w vanilla).
pub const MAX_FRAME: usize = 2_097_151;

type Dec = cfb8::Decryptor<Aes128>;
type Enc = cfb8::Encryptor<Aes128>;

/// Dekoder strumienia: dostaje surowe bajty z gniazda, oddaje kompletne pakiety `(id, body)`.
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
        // Uwaga: bajty już zbuforowane są jawne; włączenie następuje między pakietami.
        self.cipher = Some(Dec::new(key.into(), key.into()));
    }

    pub fn set_compression(&mut self, threshold: i32) {
        self.threshold = if threshold >= 0 { Some(threshold) } else { None };
    }

    /// Dopisuje świeże bajty z gniazda (odszyfrowuje je, jeśli szyfrowanie jest włączone).
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

    /// Próbuje wyciągnąć kolejny kompletny pakiet. `Ok(None)` = potrzeba więcej danych.
    pub fn next_packet(&mut self) -> Result<Option<(i32, Vec<u8>)>> {
        let avail = &self.buf[self.start..];
        // Długość ramki
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
                // Jedno wywołanie z limitem = deklarowany rozmiar (+1, by wykryć nadmiar): odporność na zip-bomby.
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

/// Koder: z ładunku (`VarInt(id) | body`) buduje ramkę gotową do wysłania.
/// Kompresja jest niezależna od połączenia (próg globalny), więc ramki można współdzielić
/// między graczami; szyfrowanie robi dopiero writer konkretnego połączenia.
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

    /// Dodaje ramkę dla `payload` (id+body) na koniec `out`.
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

/// Szyfrator wychodzący AES-128-CFB8 (stan zachowany między zapisami).
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

/// Buduje ładunek pakietu: `VarInt(id) | body`.
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
        // podaj bajt po bajcie, żeby sprawdzić ramkowanie częściowe
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
