//! NBT: network encoding (unnamed root, since 1.20.2) and file encoding (named root).
//! Keeps key insertion order (stable bytes for 1:1 tests).

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

    pub fn remove(&mut self, k: &str) -> Option<Tag> {
        let i = self.0.iter().position(|(kk, _)| kk == k)?;
        Some(self.0.remove(i).1)
    }

    // Typed getters with the numeric coercion `CompoundTag` performs (any numeric tag -> number).
    pub fn get_i64(&self, k: &str) -> Option<i64> {
        self.get(k).and_then(Tag::as_i64)
    }
    pub fn get_i32(&self, k: &str) -> Option<i32> {
        self.get_i64(k).map(|v| v as i32)
    }
    pub fn get_f64(&self, k: &str) -> Option<f64> {
        self.get(k).and_then(Tag::as_f64)
    }
    pub fn get_bool(&self, k: &str) -> Option<bool> {
        self.get_i64(k).map(|v| v != 0)
    }
    pub fn get_str(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Tag::String(s)) => Some(s),
            _ => None,
        }
    }
    pub fn get_compound(&self, k: &str) -> Option<&Compound> {
        match self.get(k) {
            Some(Tag::Compound(c)) => Some(c),
            _ => None,
        }
    }
    pub fn get_list(&self, k: &str) -> Option<&Vec<Tag>> {
        match self.get(k) {
            Some(Tag::List(l)) => Some(l),
            _ => None,
        }
    }
    pub fn get_long_array(&self, k: &str) -> Option<&Vec<i64>> {
        match self.get(k) {
            Some(Tag::LongArray(l)) => Some(l),
            _ => None,
        }
    }
    pub fn get_byte_array(&self, k: &str) -> Option<&Vec<i8>> {
        match self.get(k) {
            Some(Tag::ByteArray(l)) => Some(l),
            _ => None,
        }
    }

    /// Key write order like Mojang's `CompoundTag`, which keeps its entries in a `java.util.HashMap`:
    /// bucket index = `(h ^ (h >>> 16)) & (n - 1)`, `h = String.hashCode()`, `n` = power of two >= 16
    /// with size <= 0.75·n; insertion order is kept within a bucket.
    pub fn java_order(&self) -> Vec<usize> {
        let hashes: Vec<i32> = self.0.iter().map(|(k, _)| java_string_hash(k)).collect();
        java_hashmap_order(&hashes)
    }
}

/// Iteration order of a `java.util.HashMap` filled by successive `put`s of keys with the given hash codes
/// (table size: power of two >= 16 with size <= 0.75·n; insertion order is kept within a bucket).
pub fn java_hashmap_order(hashes: &[i32]) -> Vec<usize> {
    let len = hashes.len();
    let mut idx: Vec<usize> = (0..len).collect();
    if len < 2 {
        return idx;
    }
    let mut n = 16usize;
    while len > n * 3 / 4 {
        n *= 2;
    }
    let bucket = |h: i32| {
        let h = h as u32;
        ((h ^ (h >> 16)) as usize) & (n - 1)
    };
    idx.sort_by_key(|&i| bucket(hashes[i])); // sort_by_key is stable
    idx
}

/// Iteration order of a fastutil `Object2ObjectOpenHashMap` (default constructor, load factor 0.75) filled by
/// successive `put`s of keys with the given `hashCode()`s: linear probing on `mix(h) & mask`, table doubling when
/// full, iteration from the end of the table towards the start.
pub fn fastutil_open_hash_order(hashes: &[i32]) -> Vec<usize> {
    fn mix(x: i32) -> u32 {
        let h = (x as u32).wrapping_mul(0x9E37_79B9);
        h ^ (h >> 16)
    }
    let max_fill = |n: usize| ((n as f64 * 0.75).ceil() as usize).min(n - 1);
    let mut n = 32usize; // arraySize(16, 0.75)
    let mut table: Vec<Option<usize>> = vec![None; n];
    let mut size = 0usize;
    let place = |table: &mut Vec<Option<usize>>, n: usize, i: usize| {
        let mut pos = mix(hashes[i]) as usize & (n - 1);
        while table[pos].is_some() {
            pos = (pos + 1) & (n - 1);
        }
        table[pos] = Some(i);
    };
    for i in 0..hashes.len() {
        place(&mut table, n, i);
        let was = size;
        size += 1;
        if was >= max_fill(n) {
            // rehash(arraySize(size + 1, 0.75)): re-insert from the end of the old table
            let mut new_n = ((size + 1) as f64 / 0.75).ceil() as usize;
            new_n = new_n.next_power_of_two().max(2);
            let mut new_table: Vec<Option<usize>> = vec![None; new_n];
            for slot in (0..n).rev() {
                if let Some(k) = table[slot] {
                    place(&mut new_table, new_n, k);
                }
            }
            table = new_table;
            n = new_n;
        }
    }
    (0..n).rev().filter_map(|slot| table[slot]).collect()
}

/// `java.lang.String.hashCode()` po jednostkach UTF-16.
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(u as i32))
}

/// List homogeneity like `NbtOps`: when element types differ, every element that is not a
/// compound is wrapped in `{"": value}`.
pub fn unify_list(items: Vec<Tag>) -> Vec<Tag> {
    let Some(first) = items.first() else { return items };
    let id = first.id();
    if items.iter().all(|t| t.id() == id) {
        return items;
    }
    items.into_iter().map(|t| if matches!(t, Tag::Compound(_)) { t } else { Tag::Compound(Compound::new().with("", t)) }).collect()
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

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Tag::Byte(v) => Some(*v as i64),
            Tag::Short(v) => Some(*v as i64),
            Tag::Int(v) => Some(*v as i64),
            Tag::Long(v) => Some(*v),
            Tag::Float(v) => Some(*v as i64),
            Tag::Double(v) => Some(*v as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Tag::Byte(v) => Some(*v as f64),
            Tag::Short(v) => Some(*v as f64),
            Tag::Int(v) => Some(*v as f64),
            Tag::Long(v) => Some(*v as f64),
            Tag::Float(v) => Some(*v as f64),
            Tag::Double(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// Network encoding: type byte + payload (no root name).
    pub fn write_network(&self, w: &mut Vec<u8>) {
        w.put_u8(self.id());
        self.write_payload(w);
    }

    /// File encoding: type byte + name + payload.
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
                for i in c.java_order() {
                    let (k, v) = &c.0[i];
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

    /// Reads a network root (unnamed). `None` for TAG_End (empty NBT).
    pub fn read_network(r: &mut Reader<'_>) -> Result<Option<Tag>> {
        let id = r.u8()?;
        if id == 0 {
            return Ok(None);
        }
        Ok(Some(Self::read_payload(id, r, 0)?))
    }

    /// Reads a file root (named).
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

/// Modified UTF-8 (Java `DataOutput.writeUTF`): U+0000 as C0 80, supplementary
/// characters as two surrogates of 3 bytes each.
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

    fn order(keys: &[&str]) -> Vec<String> {
        let mut c = Compound::new();
        for k in keys {
            c.put(*k, Tag::Byte(0));
        }
        c.java_order().into_iter().map(|i| c.0[i].0.clone()).collect()
    }

    /// Orders measured on the real vanilla 1.21.1 server (system_chat / player_chat packets).
    #[test]
    fn java_hashmap_order_matches_vanilla_observations() {
        assert_eq!(order(&["text", "color", "extra"]), ["color", "extra", "text"]);
        assert_eq!(order(&["text", "color", "extra", "clickEvent"]), ["clickEvent", "color", "extra", "text"]);
        assert_eq!(order(&["text", "color", "underlined"]), ["color", "underlined", "text"]);
        assert_eq!(order(&["translate", "color", "italic"]), ["color", "italic", "translate"]);
        assert_eq!(order(&["text", "clickEvent", "hoverEvent", "insertion"]), ["hoverEvent", "clickEvent", "insertion", "text"]);
        assert_eq!(order(&["type", "id", "name"]), ["name", "id", "type"]);
        assert_eq!(order(&["action", "contents"]), ["contents", "action"]);
        assert_eq!(order(&["action", "value"]), ["action", "value"]);
        assert_eq!(order(&["translate", "with"]), ["with", "translate"]); // the vanilla /list bytes start with `with`
        // heightmaps in the chunk packet: MOTION_BLOCKING, WORLD_SURFACE (bytes identical to vanilla)
        assert_eq!(order(&["WORLD_SURFACE", "MOTION_BLOCKING"]), ["MOTION_BLOCKING", "WORLD_SURFACE"]);
    }

    #[test]
    fn fastutil_order_small_cases() {
        // a single entry and trivially distinct slots: iteration is by descending slot index
        assert_eq!(fastutil_open_hash_order(&[]), Vec::<usize>::new());
        assert_eq!(fastutil_open_hash_order(&[5]), vec![0]);
        let o = fastutil_open_hash_order(&[1, 2, 3, 4]);
        assert_eq!(o.len(), 4);
        let mut sorted = o.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }

    #[test]
    fn mixed_list_is_wrapped_like_nbtops() {
        let v = unify_list(vec![Tag::Int(1), Tag::Compound(Compound::new().with("text", Tag::string("x")))]);
        assert_eq!(v[0], Tag::Compound(Compound::new().with("", Tag::Int(1))));
        assert!(matches!(v[1], Tag::Compound(_)));
        let same = unify_list(vec![Tag::Int(1), Tag::Int(2)]);
        assert_eq!(same, vec![Tag::Int(1), Tag::Int(2)]);
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
