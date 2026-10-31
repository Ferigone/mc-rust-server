//! NBT: zapis sieciowy (korzeń bez nazwy, od 1.20.2) oraz pliki (korzeń nazwany).
//! Zachowuje kolejność wstawiania kluczy (stabilne bajty dla testów 1:1).

use crate::buf::{Put, Reader};
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Compound(pub Vec<(String, Tag)>);

impl Compound {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn put(&mut self, k: impl Into<String>, v: Tag) -> &mut Self {
        let k = k.into();
        if let Some(slot) = self.0.iter_mut().find(|(kk, _)| *kk == k) {
            slot.1 = v;
        } else {
            self.0.push((k, v));
        }
        self
    }
    pub fn with(mut self, k: impl Into<String>, v: Tag) -> Self {
        self.put(k, v);
        self
    }
    pub fn get(&self, k: &str) -> Option<&Tag> {
        self.0.iter().find(|(kk, _)| kk == k).map(|(_, v)| v)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    pub fn string(s: impl Into<String>) -> Tag {
        Tag::String(s.into())
    }

    /// Zapis sieciowy: bajt typu + ładunek (bez nazwy korzenia).
    pub fn write_network(&self, w: &mut Vec<u8>) {
        w.put_u8(self.id());
        self.write_payload(w);
    }

    /// Zapis „plikowy”: bajt typu + nazwa + ładunek.
    pub fn write_named(&self, name: &str, w: &mut Vec<u8>) {
        w.put_u8(self.id());
        write_mutf8(name, w);
        self.write_payload(w);
    }

    pub fn write_payload(&self, w: &mut Vec<u8>) {
        match self {
            Tag::Byte(v) => w.put_i8(*v),
            Tag::Short(v) => w.put_i16(*v),
            Tag::Int(v) => w.put_i32(*v),
            Tag::Long(v) => w.put_i64(*v),
            Tag::Float(v) => w.put_f32(*v),
            Tag::Double(v) => w.put_f64(*v),
            Tag::ByteArray(v) => {
                w.put_i32(v.len() as i32);
                for b in v {
                    w.put_i8(*b);
                }
            }
            Tag::String(s) => write_mutf8(s, w),
            Tag::List(items) => {
                let id = items.first().map_or(0, Tag::id);
                w.put_u8(id);
                w.put_i32(items.len() as i32);
                for it in items {
                    debug_assert_eq!(it.id(), id, "NBT list must be homogeneous");
                    it.write_payload(w);
                }
            }
            Tag::Compound(c) => {
                for (k, v) in &c.0 {
                    w.put_u8(v.id());
                    write_mutf8(k, w);
                    v.write_payload(w);
                }
                w.put_u8(0);
            }
            Tag::IntArray(v) => {
                w.put_i32(v.len() as i32);
                for x in v {
                    w.put_i32(*x);
                }
            }
            Tag::LongArray(v) => {
                w.put_i32(v.len() as i32);
                for x in v {
                    w.put_i64(*x);
                }
            }
        }
    }

    /// Odczyt korzenia sieciowego (bez nazwy). `None` dla TAG_End (pusty NBT).
    pub fn read_network(r: &mut Reader<'_>) -> Result<Option<Tag>> {
        let id = r.u8()?;
        if id == 0 {
            return Ok(None);
        }
        Ok(Some(Self::read_payload(id, r, 0)?))
    }

    /// Odczyt korzenia plikowego (z nazwą).
    pub fn read_named(r: &mut Reader<'_>) -> Result<Option<(String, Tag)>> {
        let id = r.u8()?;
        if id == 0 {
            return Ok(None);
        }
        let name = read_mutf8(r)?;
        Ok(Some((name, Self::read_payload(id, r, 0)?)))
    }

    fn read_payload(id: u8, r: &mut Reader<'_>, depth: u32) -> Result<Tag> {
        if depth > 512 {
            return Err(Error::Invalid("NBT nesting too deep"));
        }
        let len = |r: &mut Reader<'_>| -> Result<usize> {
            let n = r.i32()?;
            if n < 0 {
                return Ok(0);
            }
            Ok(n as usize)
        };
        Ok(match id {
            1 => Tag::Byte(r.i8()?),
            2 => Tag::Short(r.i16()?),
            3 => Tag::Int(r.i32()?),
            4 => Tag::Long(r.i64()?),
            5 => Tag::Float(r.f32()?),
            6 => Tag::Double(r.f64()?),
            7 => {
                let n = len(r)?;
                let b = r.take(n)?;
                Tag::ByteArray(b.iter().map(|&x| x as i8).collect())
            }
            8 => Tag::String(read_mutf8(r)?),
            9 => {
                let eid = r.u8()?;
                let n = len(r)?;
                if eid == 0 && n > 0 {
                    return Err(Error::Invalid("NBT list of END"));
                }
                let mut v = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    v.push(Self::read_payload(eid, r, depth + 1)?);
                }
                Tag::List(v)
            }
            10 => {
                let mut c = Compound::new();
                loop {
                    let tid = r.u8()?;
                    if tid == 0 {
                        break;
                    }
                    let k = read_mutf8(r)?;
                    let v = Self::read_payload(tid, r, depth + 1)?;
                    c.0.push((k, v));
                }
                Tag::Compound(c)
            }
            11 => {
                let n = len(r)?;
                let mut v = Vec::with_capacity(n.min(65536));
                for _ in 0..n {
                    v.push(r.i32()?);
                }
                Tag::IntArray(v)
            }
            12 => {
                let n = len(r)?;
                let mut v = Vec::with_capacity(n.min(65536));
                for _ in 0..n {
                    v.push(r.i64()?);
                }
                Tag::LongArray(v)
            }
            _ => return Err(Error::Invalid("unknown NBT tag id")),
        })
    }
}

/// Modified UTF-8 (Java `DataOutput.writeUTF`): U+0000 jako C0 80, znaki
/// uzupełniające jako dwie pary zastępcze po 3 bajty.
pub fn write_mutf8(s: &str, w: &mut Vec<u8>) {
    let mut out = Vec::with_capacity(s.len());
    for u in s.encode_utf16() {
        match u {
            0x0001..=0x007F => out.push(u as u8),
            0x0000 | 0x0080..=0x07FF => {
                out.push(0xC0 | (u >> 6) as u8);
                out.push(0x80 | (u & 0x3F) as u8);
            }
            _ => {
                out.push(0xE0 | (u >> 12) as u8);
                out.push(0x80 | ((u >> 6) & 0x3F) as u8);
                out.push(0x80 | (u & 0x3F) as u8);
            }
        }
    }
    w.put_u16(out.len() as u16);
    w.extend_from_slice(&out);
}

pub fn read_mutf8(r: &mut Reader<'_>) -> Result<String> {
    let n = r.u16()? as usize;
    let b = r.take(n)?;
    let mut units: Vec<u16> = Vec::with_capacity(n);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c >> 4 {
            0..=7 => {
                units.push(c as u16);
                i += 1;
            }
            12 | 13 => {
                let c2 = *b.get(i + 1).ok_or(Error::Invalid("bad mutf8"))?;
                units.push((((c & 0x1F) as u16) << 6) | (c2 & 0x3F) as u16);
                i += 2;
            }
            14 => {
                let (c2, c3) = (*b.get(i + 1).ok_or(Error::Invalid("bad mutf8"))?, *b.get(i + 2).ok_or(Error::Invalid("bad mutf8"))?);
                units.push((((c & 0x0F) as u16) << 12) | (((c2 & 0x3F) as u16) << 6) | (c3 & 0x3F) as u16);
                i += 3;
            }
            _ => return Err(Error::Invalid("bad mutf8")),
        }
    }
    Ok(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_roundtrip() {
        let c = Compound::new()
            .with("a", Tag::Int(5))
            .with("b", Tag::string("zażółć 𝄞"))
            .with("c", Tag::List(vec![Tag::Long(1), Tag::Long(2)]))
            .with("d", Tag::LongArray(vec![1, -2, 3]));
        let t = Tag::Compound(c);
        let mut w = Vec::new();
        t.write_network(&mut w);
        assert_eq!(w[0], 10);
        let back = Tag::read_network(&mut Reader::new(&w)).unwrap().unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn network_empty_compound_bytes() {
        let mut w = Vec::new();
        Tag::Compound(Compound::new()).write_network(&mut w);
        assert_eq!(w, [0x0a, 0x00]);
    }

    #[test]
    fn mutf8_null_and_supplementary() {
        let mut w = Vec::new();
        write_mutf8("\0", &mut w);
        assert_eq!(w, [0, 2, 0xC0, 0x80]);
        let mut w = Vec::new();
        write_mutf8("𝄞", &mut w); // U+1D11E -> D834 DD1E
        assert_eq!(w, [0, 6, 0xED, 0xA0, 0xB4, 0xED, 0xB4, 0x9E]);
        assert_eq!(read_mutf8(&mut Reader::new(&w)).unwrap(), "𝄞");
    }
}
