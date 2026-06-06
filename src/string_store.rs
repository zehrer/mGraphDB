//! Append-only, deduplicated UTF-8 string store.
//!
//! Strings are interned once and addressed two ways:
//! - `StrId` (u64): compact internal reference, stable within a segment.
//! - `StrHash` (128-bit): content hash for dedup and cross-segment identity.
//!
//! On-disk segment layout (little-endian throughout):
//! ```text
//! [Header]                      magic + version + flags
//! [Block 0 | payload | trailer] payload = concatenated entries, trailer = entry offsets
//! [Block 1 | ... ]
//! ...
//! [BlockDirectory]              per-block file offset, payload len, entry count
//! [Footer]                      directory location, counts, flags, crc, magic
//! ```
//! Each entry: `[hash128 (16)] [varint len] [utf8 bytes] [crc32 (4)]`.
//!
//! v1 limitations (seams left for later): block payloads are uncompressed and the
//! optional Bloom filter is not built (footer `flags` records their absence);
//! `save` rewrites the whole segment rather than appending incrementally. The
//! in-memory model is genuinely append-only — existing entries are never mutated.
//!
//! Large values: today every string lands inline in a block. A future value
//! type may divert long strings/blobs to a dedicated, more space-efficient
//! store; `intern` is the single choke point where that routing would hook in.

use std::collections::HashMap;
use std::io;
use std::path::Path;

/// Compact internal reference to an interned string.
pub type StrId = u64;

/// 128-bit content hash (xxh3-128), stored little-endian on disk.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub struct StrHash(pub [u8; 16]);

impl StrHash {
    fn from_u128(v: u128) -> Self {
        StrHash(v.to_le_bytes())
    }
    fn to_u128(self) -> u128 {
        u128::from_le_bytes(self.0)
    }
}

const HEADER_MAGIC: u32 = 0x5353_474D; // "MGSS" little-endian
const FOOTER_MAGIC: u32 = 0x4655_5353; // "SSUF" little-endian
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 8; // magic(4) + version(2) + flags(2)
const FOOTER_LEN: usize = 40;
const DEFAULT_BLOCK_TARGET: usize = 64 * 1024;

/// Location of one entry within the in-RAM block buffers.
#[derive(Clone, Copy)]
struct EntryLoc {
    block: u32,
    entry_off: u32,
    len: u32,
}

/// In-memory, append-only string store backed by a single segment file.
pub struct StringStore {
    /// Sealed and current block payloads (no trailers); last is the open block.
    blocks: Vec<Vec<u8>>,
    /// Entry start offsets within each block, parallel to `blocks`.
    block_offsets: Vec<Vec<u32>>,
    /// id -> location; index is the StrId.
    by_id: Vec<EntryLoc>,
    /// hash -> id, for dedup and `id_of`.
    by_hash: HashMap<u128, StrId>,
    /// Soft cap that triggers opening a fresh block.
    block_target: usize,
}

impl Default for StringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StringStore {
    /// Create an empty store with the default ~64 KiB block target.
    pub fn new() -> Self {
        Self::with_block_target(DEFAULT_BLOCK_TARGET)
    }

    fn with_block_target(block_target: usize) -> Self {
        StringStore {
            blocks: Vec::new(),
            block_offsets: Vec::new(),
            by_id: Vec::new(),
            by_hash: HashMap::new(),
            block_target,
        }
    }

    /// Number of unique strings stored.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True if no strings are stored.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Intern `s`, returning its hash and id. Idempotent: an already-present
    /// string yields its existing id and hash without storing a duplicate.
    pub fn intern(&mut self, s: &str) -> (StrHash, StrId) {
        let hash = xxhash_rust::xxh3::xxh3_128(s.as_bytes());
        if let Some(&id) = self.by_hash.get(&hash) {
            return (StrHash::from_u128(hash), id);
        }

        let bytes = s.as_bytes();
        let block_idx = self.ensure_block();
        let block = &mut self.blocks[block_idx];
        let entry_off = block.len() as u32;

        block.extend_from_slice(&hash.to_le_bytes());
        write_varint(block, bytes.len() as u64);
        block.extend_from_slice(bytes);
        let crc = crc32fast::hash(bytes);
        block.extend_from_slice(&crc.to_le_bytes());

        self.block_offsets[block_idx].push(entry_off);

        let id = self.by_id.len() as StrId;
        self.by_id.push(EntryLoc {
            block: block_idx as u32,
            entry_off,
            len: bytes.len() as u32,
        });
        self.by_hash.insert(hash, id);
        (StrHash::from_u128(hash), id)
    }

    /// Resolve a string by its id.
    pub fn resolve_id(&self, id: StrId) -> Option<&str> {
        let loc = self.by_id.get(id as usize)?;
        let block = &self.blocks[loc.block as usize];
        let str_off = loc.entry_off as usize + 16 + varint_len(loc.len as u64);
        let bytes = &block[str_off..str_off + loc.len as usize];
        std::str::from_utf8(bytes).ok()
    }

    /// Resolve a string by its content hash.
    pub fn resolve_hash(&self, h: StrHash) -> Option<&str> {
        let id = *self.by_hash.get(&h.to_u128())?;
        self.resolve_id(id)
    }

    /// Look up the id for a hash, if present.
    pub fn id_of(&self, h: StrHash) -> Option<StrId> {
        self.by_hash.get(&h.to_u128()).copied()
    }

    /// Read back the stored hash for an id.
    pub fn hash_of(&self, id: StrId) -> Option<StrHash> {
        let loc = self.by_id.get(id as usize)?;
        let block = &self.blocks[loc.block as usize];
        let off = loc.entry_off as usize;
        let mut h = [0u8; 16];
        h.copy_from_slice(&block[off..off + 16]);
        Some(StrHash(h))
    }

    /// Index of a block with room to append; opens a new one when needed.
    fn ensure_block(&mut self) -> usize {
        let need_new = match self.blocks.last() {
            None => true,
            Some(b) => b.len() >= self.block_target,
        };
        if need_new {
            self.blocks.push(Vec::new());
            self.block_offsets.push(Vec::new());
        }
        self.blocks.len() - 1
    }

    /// Serialize the whole store to a segment file at `path`.
    ///
    /// v1 writes via a temp file + rename so a crash mid-write cannot corrupt an
    /// existing segment.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut buf = Vec::new();

        // Header.
        buf.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // flags: no compression, no bloom

        // Blocks: payload then entry-offset trailer.
        let mut dir_entries: Vec<(u64, u32, u32)> = Vec::with_capacity(self.blocks.len());
        for (block, offsets) in self.blocks.iter().zip(&self.block_offsets) {
            let file_offset = buf.len() as u64;
            let payload_len = block.len() as u32;
            buf.extend_from_slice(block);
            for &off in offsets {
                buf.extend_from_slice(&off.to_le_bytes());
            }
            buf.extend_from_slice(&(offsets.len() as u32).to_le_bytes());
            dir_entries.push((file_offset, payload_len, offsets.len() as u32));
        }

        // Block directory.
        let dir_offset = buf.len() as u64;
        let mut dir = Vec::new();
        dir.extend_from_slice(&(dir_entries.len() as u32).to_le_bytes());
        for (off, plen, ecount) in &dir_entries {
            dir.extend_from_slice(&off.to_le_bytes());
            dir.extend_from_slice(&plen.to_le_bytes());
            dir.extend_from_slice(&ecount.to_le_bytes());
        }
        let dir_crc = crc32fast::hash(&dir);
        let dir_len = dir.len() as u32;
        buf.extend_from_slice(&dir);

        // Footer (fixed 40 bytes, read from end of file).
        buf.extend_from_slice(&dir_offset.to_le_bytes()); // 8
        buf.extend_from_slice(&dir_len.to_le_bytes()); // 4
        buf.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes()); // 4
        buf.extend_from_slice(&(self.by_id.len() as u64).to_le_bytes()); // 8
        buf.extend_from_slice(&0u32.to_le_bytes()); // 4 flags
        buf.extend_from_slice(&0u32.to_le_bytes()); // 4 reserved
        buf.extend_from_slice(&dir_crc.to_le_bytes()); // 4
        buf.extend_from_slice(&FOOTER_MAGIC.to_le_bytes()); // 4

        let path = path.as_ref();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load a store from a segment file, rebuilding the in-RAM indexes.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        if data.len() < HEADER_LEN + FOOTER_LEN {
            return Err(corrupt("file too small"));
        }
        if read_u32(&data, 0) != HEADER_MAGIC {
            return Err(corrupt("bad header magic"));
        }

        let f = data.len() - FOOTER_LEN;
        if read_u32(&data, f + 36) != FOOTER_MAGIC {
            return Err(corrupt("bad footer magic"));
        }
        let dir_offset = read_u64(&data, f) as usize;
        let dir_len = read_u32(&data, f + 8) as usize;
        let block_count = read_u32(&data, f + 12) as usize;
        let dir_crc = read_u32(&data, f + 32);

        if dir_offset + dir_len > data.len() {
            return Err(corrupt("directory out of range"));
        }
        let dir = &data[dir_offset..dir_offset + dir_len];
        if crc32fast::hash(dir) != dir_crc {
            return Err(corrupt("directory crc mismatch"));
        }
        if read_u32(dir, 0) as usize != block_count {
            return Err(corrupt("directory block count mismatch"));
        }

        let mut store = StringStore::with_block_target(DEFAULT_BLOCK_TARGET);
        let mut dp = 4;
        for _ in 0..block_count {
            let file_offset = read_u64(dir, dp) as usize;
            let payload_len = read_u32(dir, dp + 8) as usize;
            let entry_count = read_u32(dir, dp + 12) as usize;
            dp += 16;

            let payload = data[file_offset..file_offset + payload_len].to_vec();
            let trailer_start = file_offset + payload_len;
            let mut offsets = Vec::with_capacity(entry_count);
            for i in 0..entry_count {
                offsets.push(read_u32(&data, trailer_start + i * 4));
            }

            let block_idx = store.blocks.len() as u32;
            // Rebuild id/hash indexes from the entries in this block.
            for &entry_off in &offsets {
                let off = entry_off as usize;
                let hash = read_u128(&payload, off);
                let (len, vlen) = read_varint(&payload, off + 16);
                let str_off = off + 16 + vlen;
                let bytes = &payload[str_off..str_off + len as usize];
                let crc = read_u32(&payload, str_off + len as usize);
                if crc32fast::hash(bytes) != crc {
                    return Err(corrupt("entry crc mismatch"));
                }
                if std::str::from_utf8(bytes).is_err() {
                    return Err(corrupt("entry not valid utf-8"));
                }
                let id = store.by_id.len() as StrId;
                store.by_id.push(EntryLoc {
                    block: block_idx,
                    entry_off,
                    len: len as u32,
                });
                store.by_hash.insert(hash, id);
            }
            store.blocks.push(payload);
            store.block_offsets.push(offsets);
        }
        Ok(store)
    }
}

fn corrupt(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("string store: {msg}"))
}

/// LEB128 unsigned varint length of `n`.
fn varint_len(mut n: u64) -> usize {
    let mut len = 1;
    while n >= 0x80 {
        n >>= 7;
        len += 1;
    }
    len
}

fn write_varint(buf: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        buf.push((n as u8 & 0x7f) | 0x80);
        n >>= 7;
    }
    buf.push(n as u8);
}

/// Returns (value, bytes consumed).
fn read_varint(buf: &[u8], mut pos: usize) -> (u64, usize) {
    let mut result = 0u64;
    let mut shift = 0;
    let start = pos;
    loop {
        let byte = buf[pos];
        result |= ((byte & 0x7f) as u64) << shift;
        pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (result, pos - start)
}

fn read_u32(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap())
}

fn read_u128(buf: &[u8], pos: usize) -> u128 {
    u128::from_le_bytes(buf[pos..pos + 16].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_dedups_and_assigns_sequential_ids() {
        let mut s = StringStore::new();
        let (h1, id1) = s.intern("alpha");
        let (h2, id2) = s.intern("beta");
        let (h3, id3) = s.intern("alpha");

        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
        assert_eq!(id3, id1, "re-interning returns the same id");
        assert_eq!(h3, h1);
        assert_ne!(h1, h2);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn resolve_by_id_and_hash() {
        let mut s = StringStore::new();
        let (h, id) = s.intern("https://example.com/path");
        assert_eq!(s.resolve_id(id), Some("https://example.com/path"));
        assert_eq!(s.resolve_hash(h), Some("https://example.com/path"));
        assert_eq!(s.id_of(h), Some(id));
        assert_eq!(s.hash_of(id), Some(h));
    }

    #[test]
    fn missing_lookups_return_none() {
        let mut s = StringStore::new();
        s.intern("only");
        assert_eq!(s.resolve_id(99), None);
        assert_eq!(s.hash_of(99), None);
        assert_eq!(s.resolve_hash(StrHash([0u8; 16])), None);
        assert_eq!(s.id_of(StrHash([0u8; 16])), None);
    }

    #[test]
    fn empty_and_unicode_strings() {
        let mut s = StringStore::new();
        let (_, empty_id) = s.intern("");
        let (_, uni_id) = s.intern("héllo · 世界 · 🦀");
        assert_eq!(s.resolve_id(empty_id), Some(""));
        assert_eq!(s.resolve_id(uni_id), Some("héllo · 世界 · 🦀"));
    }

    #[test]
    fn roundtrip_save_open() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ss_{}.seg", std::process::id()));

        let inputs = ["one", "two", "three", "two", "", "🦀"];
        let mut ids = Vec::new();
        let mut src = StringStore::new();
        for x in inputs {
            ids.push(src.intern(x).1);
        }
        src.save(&path).unwrap();

        let loaded = StringStore::open(&path).unwrap();
        assert_eq!(loaded.len(), 5); // "two" deduped
        assert_eq!(loaded.resolve_id(ids[0]), Some("one"));
        assert_eq!(loaded.resolve_id(ids[2]), Some("three"));
        assert_eq!(loaded.resolve_id(ids[3]), Some("two"));
        assert_eq!(loaded.resolve_id(ids[5]), Some("🦀"));

        // Hash identity survives the roundtrip.
        let (h, _) = src.intern("three");
        assert_eq!(loaded.resolve_hash(h), Some("three"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn multi_block_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ss_mb_{}.seg", std::process::id()));

        // Tiny block target forces many blocks.
        let mut src = StringStore::with_block_target(64);
        let n = 500;
        let ids: Vec<_> = (0..n).map(|i| src.intern(&format!("value-{i}")).1).collect();
        assert!(src.blocks.len() > 1, "expected multiple blocks");
        src.save(&path).unwrap();

        let loaded = StringStore::open(&path).unwrap();
        assert_eq!(loaded.len(), n);
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(loaded.resolve_id(id), Some(format!("value-{i}").as_str()));
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_magic() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ss_bad_{}.seg", std::process::id()));
        let mut s = StringStore::new();
        s.intern("x");
        s.save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 0xff; // corrupt header magic
        std::fs::write(&path, &bytes).unwrap();

        assert!(StringStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
