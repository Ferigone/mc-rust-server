//! Anvil region files (`r.<x>.<z>.mca`): reading and writing chunks.
//!
//! Layout: an 8 KiB header (1024 big-endian `(sector << 8) | sector_count` entries, then 1024 timestamps),
//! followed by 4 KiB sectors. A chunk is `u32 length | u8 compression | payload`. Chunks larger than 255
//! sectors are stored in an external `c.<x>.<z>.mcc` file (compression type | 128).

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use mc_protocol::nbt::{Compound, Tag};
use mc_protocol::Reader;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SECTOR: u64 = 4096;
const HEADER_SECTORS: usize = 2;

pub struct RegionFile {
    file: File,
    dir: PathBuf,
    rx: i32,
    rz: i32,
    offsets: Vec<u32>,
    timestamps: Vec<u32>,
    /// One flag per sector of the file (`true` = in use).
    used: Vec<bool>,
}

fn index(lx: i32, lz: i32) -> usize {
    ((lx & 31) + (lz & 31) * 32) as usize
}

impl RegionFile {
    pub fn open(path: &Path, rx: i32, rz: i32, create: bool) -> Result<Self> {
        let mut file = OpenOptions::new().read(true).write(true).create(create).truncate(false).open(path).with_context(|| path.display().to_string())?;
        let len = file.metadata()?.len();
        if len < SECTOR * HEADER_SECTORS as u64 {
            file.set_len(SECTOR * HEADER_SECTORS as u64)?;
        }
        let mut header = vec![0u8; 8192];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header)?;
        let offsets: Vec<u32> = (0..1024).map(|i| u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap())).collect();
        let timestamps: Vec<u32> = (0..1024).map(|i| u32::from_be_bytes(header[4096 + i * 4..4096 + i * 4 + 4].try_into().unwrap())).collect();
        let total_sectors = (file.metadata()?.len().div_ceil(SECTOR)) as usize;
        let mut used = vec![false; total_sectors.max(HEADER_SECTORS)];
        used[0] = true;
        used[1] = true;
        for &o in &offsets {
            let (start, count) = ((o >> 8) as usize, (o & 0xFF) as usize);
            if o != 0 && start >= HEADER_SECTORS {
                for s in start..start + count {
                    if s < used.len() {
                        used[s] = true;
                    }
                }
            }
        }
        Ok(Self { file, dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(), rx, rz, offsets, timestamps, used })
    }

    fn mcc_path(&self, lx: i32, lz: i32) -> PathBuf {
        self.dir.join(format!("c.{}.{}.mcc", self.rx * 32 + (lx & 31), self.rz * 32 + (lz & 31)))
    }

    /// Reads and decompresses a chunk; `Ok(None)` when it is absent.
    pub fn read_chunk(&mut self, lx: i32, lz: i32) -> Result<Option<Vec<u8>>> {
        let o = self.offsets[index(lx, lz)];
        if o == 0 {
            return Ok(None);
        }
        let (start, count) = ((o >> 8) as u64, (o & 0xFF) as u64);
        if start < HEADER_SECTORS as u64 || count == 0 {
            return Ok(None);
        }
        self.file.seek(SeekFrom::Start(start * SECTOR))?;
        let mut head = [0u8; 5];
        self.file.read_exact(&mut head)?;
        let len = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        let ctype = head[4];
        if len == 0 || len > count * SECTOR {
            bail!("corrupt chunk length {len} at ({lx},{lz})");
        }
        let external = ctype & 128 != 0;
        let payload = if external {
            std::fs::read(self.mcc_path(lx, lz))?
        } else {
            let mut buf = vec![0u8; (len - 1) as usize];
            self.file.read_exact(&mut buf)?;
            buf
        };
        Ok(Some(decompress(ctype & 127, &payload)?))
    }

    /// Writes an uncompressed NBT blob as a zlib-compressed chunk.
    pub fn write_chunk(&mut self, lx: i32, lz: i32, nbt: &[u8], now_secs: u32) -> Result<()> {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(nbt)?;
        let compressed = enc.finish()?;
        let i = index(lx, lz);
        let old = self.offsets[i];
        let (mut data, ctype) = (compressed, 2u8);
        let mut external = false;
        let total = data.len() as u64 + 5;
        if total.div_ceil(SECTOR) >= 256 {
            std::fs::write(self.mcc_path(lx, lz), &data)?;
            data = Vec::new();
            external = true;
        }
        let mut blob = Vec::with_capacity(data.len() + 5);
        blob.extend_from_slice(&((data.len() as u32 + 1).to_be_bytes()));
        blob.push(if external { ctype | 128 } else { ctype });
        blob.extend_from_slice(&data);
        let needed = (blob.len() as u64).div_ceil(SECTOR) as usize;
        // free the old allocation, then find a run of free sectors (or extend the file)
        let (ostart, ocount) = ((old >> 8) as usize, (old & 0xFF) as usize);
        if old != 0 {
            for s in ostart..ostart + ocount {
                if s < self.used.len() {
                    self.used[s] = false;
                }
            }
        }
        let start = self.find_free_run(needed);
        while self.used.len() < start + needed {
            self.used.push(false);
        }
        for s in start..start + needed {
            self.used[s] = true;
        }
        blob.resize(needed * SECTOR as usize, 0);
        self.file.seek(SeekFrom::Start(start as u64 * SECTOR))?;
        self.file.write_all(&blob)?;
        self.offsets[i] = ((start as u32) << 8) | needed as u32;
        self.timestamps[i] = now_secs;
        self.file.seek(SeekFrom::Start(i as u64 * 4))?;
        self.file.write_all(&self.offsets[i].to_be_bytes())?;
        self.file.seek(SeekFrom::Start(4096 + i as u64 * 4))?;
        self.file.write_all(&self.timestamps[i].to_be_bytes())?;
        self.file.flush()?;
        Ok(())
    }

    fn find_free_run(&self, needed: usize) -> usize {
        let mut run = 0;
        for s in HEADER_SECTORS..self.used.len() {
            if self.used[s] {
                run = 0;
            } else {
                run += 1;
                if run == needed {
                    return s + 1 - needed;
                }
            }
        }
        // extend: continue a free tail run if there is one
        let tail = self.used.iter().rev().take_while(|u| !**u).count().min(self.used.len().saturating_sub(HEADER_SECTORS));
        self.used.len() - tail
    }
}

/// Decompresses a chunk payload by Anvil compression type (1 gzip, 2 zlib, 3 none, 4 LZ4 block stream).
pub fn decompress(ctype: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match ctype {
        1 => {
            GzDecoder::new(payload).read_to_end(&mut out)?;
        }
        2 => {
            ZlibDecoder::new(payload).read_to_end(&mut out)?;
        }
        3 => out.extend_from_slice(payload),
        4 => return lz4_block_stream(payload),
        other => bail!("unsupported region compression type {other}"),
    }
    Ok(out)
}

/// `LZ4BlockOutputStream` format (lz4-java): blocks of
/// `"LZ4Block" | token | compressed len (LE) | decompressed len (LE) | xxhash32 (LE) | data`.
fn lz4_block_stream(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while !b.is_empty() {
        if b.len() < 21 || &b[..8] != b"LZ4Block" {
            bail!("bad LZ4 block header");
        }
        let token = b[8];
        let clen = u32::from_le_bytes(b[9..13].try_into().unwrap()) as usize;
        let dlen = u32::from_le_bytes(b[13..17].try_into().unwrap()) as usize;
        let data = b.get(21..21 + clen).ok_or_else(|| anyhow!("truncated LZ4 block"))?;
        match token & 0xF0 {
            0x10 => out.extend_from_slice(data), // stored
            0x20 => out.extend(lz4_flex::block::decompress(data, dlen).map_err(|e| anyhow!("lz4: {e}"))?),
            _ => bail!("unknown LZ4 block method"),
        }
        b = &b[21 + clen..];
    }
    Ok(out)
}

/// A directory of region files with an open-file cache.
pub struct RegionStorage {
    dir: PathBuf,
    files: HashMap<(i32, i32), RegionFile>,
}

impl RegionStorage {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, files: HashMap::new() }
    }

    fn region(&mut self, rx: i32, rz: i32, create: bool) -> Result<Option<&mut RegionFile>> {
        if !self.files.contains_key(&(rx, rz)) {
            let path = self.dir.join(format!("r.{rx}.{rz}.mca"));
            if !path.exists() && !create {
                return Ok(None);
            }
            std::fs::create_dir_all(&self.dir)?;
            self.files.insert((rx, rz), RegionFile::open(&path, rx, rz, create)?);
        }
        Ok(self.files.get_mut(&(rx, rz)))
    }

    /// Reads a chunk's NBT compound.
    pub fn read(&mut self, cx: i32, cz: i32) -> Result<Option<Compound>> {
        let Some(r) = self.region(cx >> 5, cz >> 5, false)? else { return Ok(None) };
        let Some(bytes) = r.read_chunk(cx & 31, cz & 31)? else { return Ok(None) };
        match Tag::read_named(&mut Reader::new(&bytes))? {
            Some((_, Tag::Compound(c))) => Ok(Some(c)),
            _ => bail!("chunk ({cx},{cz}) root is not a compound"),
        }
    }

    pub fn write(&mut self, cx: i32, cz: i32, nbt: &Compound) -> Result<()> {
        let mut bytes = Vec::new();
        Tag::Compound(nbt.clone()).write_named("", &mut bytes);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        let r = self.region(cx >> 5, cz >> 5, true)?.expect("created");
        r.write_chunk(cx & 31, cz & 31, &bytes, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-anvil-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(x: i32, z: i32, pad: usize) -> Compound {
        Compound::new().with("xPos", Tag::Int(x)).with("zPos", Tag::Int(z)).with("pad", Tag::ByteArray(vec![7; pad]))
    }

    #[test]
    fn write_read_roundtrip_and_reallocation() {
        let d = tmp("rt");
        let mut st = RegionStorage::new(d.clone());
        st.write(3, -2, &sample(3, -2, 100)).unwrap();
        st.write(4, -2, &sample(4, -2, 20_000)).unwrap(); // several sectors
        assert_eq!(st.read(3, -2).unwrap().unwrap().get_i32("xPos"), Some(3));
        // grow chunk (3,-2): must move and not corrupt the neighbour
        st.write(3, -2, &sample(3, -2, 50_000)).unwrap();
        assert_eq!(st.read(4, -2).unwrap().unwrap().get_byte_array("pad").unwrap().len(), 20_000);
        assert_eq!(st.read(3, -2).unwrap().unwrap().get_byte_array("pad").unwrap().len(), 50_000);
        assert!(st.read(5, -2).unwrap().is_none());
        // reopen from disk
        drop(st);
        let mut st = RegionStorage::new(d.clone());
        assert_eq!(st.read(4, -2).unwrap().unwrap().get_i32("zPos"), Some(-2));
        // file length is a multiple of the sector size
        assert_eq!(std::fs::metadata(d.join("r.0.-1.mca")).unwrap().len() % 4096, 0);
    }

    #[test]
    fn huge_chunk_goes_to_external_file() {
        let d = tmp("mcc");
        let mut st = RegionStorage::new(d.clone());
        // incompressible payload larger than 255 sectors
        let mut x: u32 = 12345;
        let pad: Vec<i8> = (0..1_200_000).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as i8 }).collect();
        st.write(0, 0, &Compound::new().with("big", Tag::ByteArray(pad.clone()))).unwrap();
        assert!(d.join("c.0.0.mcc").exists());
        assert_eq!(st.read(0, 0).unwrap().unwrap().get_byte_array("big").unwrap(), &pad);
    }
}
