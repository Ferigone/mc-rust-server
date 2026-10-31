//! Reading and writing the basic protocol types (the equivalent of `FriendlyByteBuf`).

use crate::error::{Error, Result};
use uuid::Uuid;

/// Read cursor over a packet buffer.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Returns the rest of the buffer and moves the cursor to the end.
    pub fn rest(&mut self) -> &'a [u8] {
        let r = &self.data[self.pos..];
        self.pos = self.data.len();
        r
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Eof);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.i32()? as u32))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.i64()? as u64))
    }

    pub fn varint(&mut self) -> Result<i32> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v as i32);
            }
        }
        Err(Error::VarTooLong)
    }

    pub fn varlong(&mut self) -> Result<i64> {
        let mut v: u64 = 0;
        for i in 0..10 {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v as i64);
            }
        }
        Err(Error::VarTooLong)
    }

    /// `FriendlyByteBuf.readUtf(maxChars)`: byte length <= max*3, number of UTF-16 units <= max.
    pub fn string(&mut self, max_chars: usize) -> Result<String> {
        let len = self.varint()?;
        if len < 0 {
            return Err(Error::Invalid("negative string length"));
        }
        let len = len as usize;
        if len > max_chars * 3 {
            return Err(Error::StringTooLong { len, max: max_chars * 3 });
        }
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)?;
        let utf16 = s.encode_utf16().count();
        if utf16 > max_chars {
            return Err(Error::StringTooLong { len: utf16, max: max_chars });
        }
        Ok(s.to_owned())
    }

    pub fn identifier(&mut self) -> Result<String> {
        self.string(32767)
    }

    pub fn uuid(&mut self) -> Result<Uuid> {
        let hi = self.i64()? as u64;
        let lo = self.i64()? as u64;
        Ok(Uuid::from_u128(((hi as u128) << 64) | lo as u128))
    }

    pub fn byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.varint()?;
        if len < 0 || len as usize > max {
            return Err(Error::Invalid("byte array length out of range"));
        }
        self.take(len as usize)
    }

    /// `BlockPos` spakowany w `long`: x(26) | z(26) | y(12), ze znakiem.
    pub fn block_pos(&mut self) -> Result<(i32, i32, i32)> {
        Ok(unpack_block_pos(self.i64()?))
    }
}

pub fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

pub fn unpack_block_pos(v: i64) -> (i32, i32, i32) {
    ((v >> 38) as i32, ((v << 52) >> 52) as i32, ((v << 26) >> 38) as i32)
}

/// Writing protocol types into a `Vec<u8>`.
pub trait Put {
    fn put_u8(&mut self, v: u8);
    fn put_i8(&mut self, v: i8) {
        self.put_u8(v as u8)
    }
    fn put_bool(&mut self, v: bool) {
        self.put_u8(v as u8)
    }
    fn put_u16(&mut self, v: u16);
    fn put_i16(&mut self, v: i16) {
        self.put_u16(v as u16)
    }
    fn put_i32(&mut self, v: i32);
    fn put_i64(&mut self, v: i64);
    fn put_f32(&mut self, v: f32) {
        self.put_i32(v.to_bits() as i32)
    }
    fn put_f64(&mut self, v: f64) {
        self.put_i64(v.to_bits() as i64)
    }
    fn put_varint(&mut self, v: i32);
    fn put_varlong(&mut self, v: i64);
    fn put_bytes(&mut self, v: &[u8]);
    fn put_string(&mut self, s: &str) {
        self.put_varint(s.len() as i32);
        self.put_bytes(s.as_bytes());
    }
    fn put_byte_array(&mut self, v: &[u8]) {
        self.put_varint(v.len() as i32);
        self.put_bytes(v);
    }
    fn put_uuid(&mut self, u: Uuid) {
        let v = u.as_u128();
        self.put_i64((v >> 64) as i64);
        self.put_i64(v as i64);
    }
    fn put_block_pos(&mut self, x: i32, y: i32, z: i32) {
        self.put_i64(pack_block_pos(x, y, z));
    }
    /// `writeLongArray`: VarInt length + longs.
    fn put_long_array(&mut self, v: &[i64]) {
        self.put_varint(v.len() as i32);
        for x in v {
            self.put_i64(*x);
        }
    }
}

impl Put for Vec<u8> {
    #[inline]
    fn put_u8(&mut self, v: u8) {
        self.push(v)
    }
    #[inline]
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_be_bytes())
    }
    #[inline]
    fn put_i32(&mut self, v: i32) {
        self.extend_from_slice(&v.to_be_bytes())
    }
    #[inline]
    fn put_i64(&mut self, v: i64) {
        self.extend_from_slice(&v.to_be_bytes())
    }
    #[inline]
    fn put_varint(&mut self, v: i32) {
        let mut v = v as u32;
        loop {
            if v & !0x7F == 0 {
                self.push(v as u8);
                return;
            }
            self.push((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
    }
    #[inline]
    fn put_varlong(&mut self, v: i64) {
        let mut v = v as u64;
        loop {
            if v & !0x7F == 0 {
                self.push(v as u8);
                return;
            }
            self.push((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
    }
    #[inline]
    fn put_bytes(&mut self, v: &[u8]) {
        self.extend_from_slice(v)
    }
}

/// Offline UUID like vanilla: `UUID.nameUUIDFromBytes("OfflinePlayer:<name>")` (MD5, version 3).
pub fn offline_uuid(name: &str) -> Uuid {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[6] = (b[6] & 0x0F) | 0x30;
    b[8] = (b[8] & 0x3F) | 0x80;
    Uuid::from_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_and_known_vectors() {
        // Vectors from the protocol documentation.
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (25565, &[0xdd, 0xc7, 0x01]),
            (2097151, &[0xff, 0xff, 0x7f]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for (v, enc) in cases {
            let mut w = Vec::new();
            w.put_varint(*v);
            assert_eq!(&w[..], *enc, "encode {v}");
            assert_eq!(Reader::new(enc).varint().unwrap(), *v, "decode {v}");
        }
    }

    #[test]
    fn varint_too_long_rejected() {
        assert!(matches!(Reader::new(&[0x80; 6]).varint(), Err(Error::VarTooLong)));
    }

    #[test]
    fn block_pos_roundtrip() {
        for &(x, y, z) in &[(0, 0, 0), (-1, -64, -1), (33554431, 2047, -33554432), (18357644, 831, -20882616)] {
            assert_eq!(unpack_block_pos(pack_block_pos(x, y, z)), (x, y, z));
        }
        // Example from the documentation: (18357644, 831, -20882616)
        assert_eq!(pack_block_pos(18357644, 831, -20882616), 0x4607632C15B4833F);
    }

    #[test]
    fn offline_uuid_matches_java() {
        // UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes(UTF_8)) w Javie
        assert_eq!(offline_uuid("Notch").to_string(), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }

    #[test]
    fn string_limits() {
        let mut w = Vec::new();
        w.put_string("hello");
        assert_eq!(Reader::new(&w).string(5).unwrap(), "hello");
        assert!(Reader::new(&w).string(4).is_err());
    }
}
