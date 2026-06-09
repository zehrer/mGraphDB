//! Fixed-length node record store.
//!
//! Every node occupies a 12-byte record at a file offset derived directly from
//! its ID (`HEADER_LEN + id * RECORD_SIZE`), giving O(1) access by ID with no
//! index lookup.
//!
//! On-disk segment layout (little-endian throughout):
//! ```text
//! [Header  16 B]   magic(4) + version(2) + flags(2) + record_size(2) + id_bytes(1) + pad(5)
//! [Record  12 B]*  one per node, in ID order
//! [Footer  20 B]   count(8) + flags(4) + crc32(4) + magic(4)
//! ```
//!
//! Record layout:
//! ```text
//! offset 0  u8   flags      bit7=DEAD, bit6=HAS_IN, bit5=HAS_OUT, bit4=HAS_PROP, bits0-3=subtype
//! offset 1  u8   reserved
//! offset 2  u16  reserved
//! offset 4  u32  type_id    NodeId::MAX = no type
//! offset 8  u32  first_out  NodeId::MAX = no outgoing edge / property
//! ```
//!
//! `NodeId::MAX` (0xFFFF_FFFF) is the sentinel for "no reference"; it is never
//! a valid allocated node ID in the Small Profile (which caps out far below 4 B
//! nodes given the 12-byte record size and target file size).
//!
//! On `open`, the CRC32 of the entire record region is verified. A corrupt
//! or unrecognised header/footer is rejected with an error.

use std::io;
use std::path::Path;

/// Compact node identifier (u32; ~4 billion live IDs in the Small Profile).
pub type NodeId = u32;

/// Sentinel `NodeId` meaning "no reference" for `type_id` and `first_out`.
pub const NO_ID: NodeId = NodeId::MAX;

const HEADER_MAGIC: u32 = u32::from_le_bytes(*b"MGNS"); // mGraphDB Node Store
const FOOTER_MAGIC: u32 = u32::from_le_bytes(*b"MGNF"); // mGraphDB Node Footer
const FORMAT_VERSION: u16 = 1;
const RECORD_SIZE: usize = 12;
const ID_BYTES: u8 = 4;
const HEADER_LEN: usize = 16;
const FOOTER_LEN: usize = 20;

// ── NodeFlags ────────────────────────────────────────────────────────────────

/// Status flags and subtype packed into one byte.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NodeFlags(pub u8);

impl NodeFlags {
    pub const DEAD:         u8 = 0b1000_0000; // tombstoned
    pub const HAS_IN:       u8 = 0b0100_0000; // has at least one incoming edge
    pub const HAS_OUT:      u8 = 0b0010_0000; // has at least one outgoing edge
    pub const HAS_PROP:     u8 = 0b0001_0000; // has at least one property
    pub const SUBTYPE_MASK: u8 = 0b0000_1111; // 16 subtypes (0 = plain node)

    pub fn is_dead(self) -> bool  { self.0 & Self::DEAD     != 0 }
    pub fn has_in(self) -> bool   { self.0 & Self::HAS_IN   != 0 }
    pub fn has_out(self) -> bool  { self.0 & Self::HAS_OUT  != 0 }
    pub fn has_prop(self) -> bool { self.0 & Self::HAS_PROP != 0 }
    pub fn subtype(self) -> u8    { self.0 & Self::SUBTYPE_MASK }

    /// Return a copy with the given bits set.
    pub fn set(self, bits: u8) -> Self   { NodeFlags(self.0 | bits) }
    /// Return a copy with the given bits cleared.
    pub fn clear(self, bits: u8) -> Self { NodeFlags(self.0 & !bits) }
    /// Return a copy with the subtype field replaced.
    pub fn with_subtype(self, t: u8) -> Self {
        NodeFlags((self.0 & !Self::SUBTYPE_MASK) | (t & Self::SUBTYPE_MASK))
    }
}

// ── NodeRecord ───────────────────────────────────────────────────────────────

/// A single fixed-length node record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeRecord {
    /// Status flags and subtype.
    pub flags: NodeFlags,
    /// Class node this node is typed as; `NO_ID` if untyped.
    pub type_id: NodeId,
    /// First edge or property record in the property store; `NO_ID` if none.
    pub first_out: NodeId,
}

impl Default for NodeRecord {
    fn default() -> Self { Self::new() }
}

impl NodeRecord {
    /// A blank live node — no type, no edges.
    pub fn new() -> Self {
        NodeRecord { flags: NodeFlags::default(), type_id: NO_ID, first_out: NO_ID }
    }

    /// Builder: set the type reference.
    pub fn with_type(mut self, type_id: NodeId) -> Self {
        self.type_id = type_id;
        self
    }

    /// Builder: set arbitrary flags.
    pub fn with_flags(mut self, flags: NodeFlags) -> Self {
        self.flags = flags;
        self
    }
}

// ── NodeStore ────────────────────────────────────────────────────────────────

/// In-memory, append-friendly node store backed by a single segment file.
pub struct NodeStore {
    records: Vec<NodeRecord>,
}

impl Default for NodeStore {
    fn default() -> Self { Self::new() }
}

impl NodeStore {
    /// Create an empty store.
    pub fn new() -> Self {
        NodeStore { records: Vec::new() }
    }

    /// Total number of records, including tombstoned nodes.
    pub fn len(&self) -> usize { self.records.len() }

    /// True if no records exist.
    pub fn is_empty(&self) -> bool { self.records.is_empty() }

    /// Number of live (non-tombstoned) nodes.
    pub fn live_count(&self) -> usize {
        self.records.iter().filter(|r| !r.flags.is_dead()).count()
    }

    /// Allocate the next `NodeId` and store a blank record. Returns the new ID.
    pub fn create(&mut self) -> NodeId {
        let id = self.records.len() as NodeId;
        self.records.push(NodeRecord::new());
        id
    }

    /// Allocate a node and set its type reference in one step.
    pub fn create_typed(&mut self, type_id: NodeId) -> NodeId {
        let id = self.records.len() as NodeId;
        self.records.push(NodeRecord::new().with_type(type_id));
        id
    }

    /// Read a record by ID. Returns `None` for out-of-range IDs.
    pub fn get(&self, id: NodeId) -> Option<NodeRecord> {
        self.records.get(id as usize).copied()
    }

    /// Overwrite a record in place. Returns `false` if `id` is out of range.
    pub fn update(&mut self, id: NodeId, record: NodeRecord) -> bool {
        match self.records.get_mut(id as usize) {
            Some(slot) => { *slot = record; true }
            None => false,
        }
    }

    /// Set the `DEAD` flag on a node (soft delete). Returns `false` if out of range.
    pub fn tombstone(&mut self, id: NodeId) -> bool {
        match self.records.get_mut(id as usize) {
            Some(r) => { r.flags = r.flags.set(NodeFlags::DEAD); true }
            None => false,
        }
    }

    /// Serialize the store to a segment file at `path`.
    ///
    /// Writes via a temp file + rename so a crash mid-write cannot corrupt an
    /// existing segment.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut buf =
            Vec::with_capacity(HEADER_LEN + self.records.len() * RECORD_SIZE + FOOTER_LEN);

        // Header
        buf.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());                  // flags
        buf.extend_from_slice(&(RECORD_SIZE as u16).to_le_bytes());  // record_size
        buf.push(ID_BYTES);
        buf.extend_from_slice(&[0u8; 5]);                            // reserved

        // Records
        let records_start = buf.len();
        for r in &self.records {
            buf.push(r.flags.0);
            buf.push(0u8);                                           // reserved
            buf.extend_from_slice(&0u16.to_le_bytes());             // reserved
            buf.extend_from_slice(&r.type_id.to_le_bytes());
            buf.extend_from_slice(&r.first_out.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf[records_start..]);

        // Footer
        buf.extend_from_slice(&(self.records.len() as u64).to_le_bytes()); // count
        buf.extend_from_slice(&0u32.to_le_bytes());                         // flags
        buf.extend_from_slice(&crc.to_le_bytes());
        buf.extend_from_slice(&FOOTER_MAGIC.to_le_bytes());

        let path = path.as_ref();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)
    }

    /// Load a store from a segment file, verifying all integrity checks.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        if data.len() < HEADER_LEN + FOOTER_LEN {
            return Err(bad("file too small"));
        }
        if read_u32(&data, 0) != HEADER_MAGIC {
            return Err(bad("bad header magic"));
        }
        let version = read_u16(&data, 4);
        if version != FORMAT_VERSION {
            return Err(bad("unsupported format version"));
        }
        let record_size = read_u16(&data, 8) as usize;
        if record_size != RECORD_SIZE {
            return Err(bad("unexpected record size"));
        }

        let f = data.len() - FOOTER_LEN;
        if read_u32(&data, f + 16) != FOOTER_MAGIC {
            return Err(bad("bad footer magic"));
        }
        let count     = read_u64(&data, f) as usize;
        let stored_crc = read_u32(&data, f + 12);

        let records_end = HEADER_LEN + count * RECORD_SIZE;
        if records_end > f {
            return Err(bad("record region overruns footer"));
        }
        let record_bytes = &data[HEADER_LEN..records_end];
        if crc32fast::hash(record_bytes) != stored_crc {
            return Err(bad("records crc mismatch"));
        }

        let mut records = Vec::with_capacity(count);
        for i in 0..count {
            let o = i * RECORD_SIZE;
            records.push(NodeRecord {
                flags:     NodeFlags(record_bytes[o]),
                type_id:   read_u32(record_bytes, o + 4),
                first_out: read_u32(record_bytes, o + 8),
            });
        }
        Ok(NodeStore { records })
    }
}

// ── Low-level helpers ────────────────────────────────────────────────────────

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("node store: {msg}"))
}

fn read_u16(buf: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap())
}

fn read_u32(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assigns_sequential_ids() {
        let mut s = NodeStore::new();
        assert_eq!(s.create(), 0);
        assert_eq!(s.create(), 1);
        assert_eq!(s.create(), 2);
        assert_eq!(s.len(), 3);
        assert_eq!(s.live_count(), 3);
    }

    #[test]
    fn create_typed() {
        let mut s = NodeStore::new();
        let class = s.create(); // ID 0 — the class node itself
        let node  = s.create_typed(class); // ID 1
        assert_eq!(s.get(node).unwrap().type_id, class);
        assert_eq!(s.get(node).unwrap().first_out, NO_ID);
    }

    #[test]
    fn get_out_of_range_returns_none() {
        let s = NodeStore::new();
        assert!(s.get(0).is_none());
        assert!(s.get(99).is_none());
    }

    #[test]
    fn update_record() {
        let mut s = NodeStore::new();
        let id = s.create();
        let updated = NodeRecord::new()
            .with_type(42)
            .with_flags(NodeFlags::default().set(NodeFlags::HAS_OUT));
        assert!(s.update(id, updated));
        let got = s.get(id).unwrap();
        assert_eq!(got.type_id, 42);
        assert!(got.flags.has_out());
        assert!(!s.update(99, updated)); // out-of-range
    }

    #[test]
    fn tombstone_sets_dead_flag() {
        let mut s = NodeStore::new();
        let id = s.create();
        assert!(!s.get(id).unwrap().flags.is_dead());
        assert!(s.tombstone(id));
        assert!(s.get(id).unwrap().flags.is_dead());
        assert_eq!(s.live_count(), 0);
        assert_eq!(s.len(), 1); // still counted in total
        assert!(!s.tombstone(99)); // out-of-range
    }

    #[test]
    fn flags_set_clear_subtype() {
        let f = NodeFlags::default()
            .set(NodeFlags::HAS_IN)
            .set(NodeFlags::HAS_OUT)
            .with_subtype(3);
        assert!(f.has_in());
        assert!(f.has_out());
        assert!(!f.has_prop());
        assert_eq!(f.subtype(), 3);

        let f2 = f.clear(NodeFlags::HAS_IN);
        assert!(!f2.has_in());
        assert!(f2.has_out());
        assert_eq!(f2.subtype(), 3);
    }

    #[test]
    fn roundtrip_save_open() {
        let dir  = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ns_{}.seg", std::process::id()));

        let mut src = NodeStore::new();
        let class_id = src.create();
        let n1 = src.create_typed(class_id);
        let n2 = src.create();
        src.update(n2, NodeRecord {
            flags:     NodeFlags::default().set(NodeFlags::HAS_OUT).with_subtype(2),
            type_id:   NO_ID,
            first_out: 42,
        });
        src.tombstone(n1);
        src.save(&path).unwrap();

        let loaded = NodeStore::open(&path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.get(class_id).unwrap(), NodeRecord::new());
        assert!(loaded.get(n1).unwrap().flags.is_dead());
        assert_eq!(loaded.get(n1).unwrap().type_id, class_id);
        let r2 = loaded.get(n2).unwrap();
        assert!(r2.flags.has_out());
        assert_eq!(r2.flags.subtype(), 2);
        assert_eq!(r2.first_out, 42);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_header_magic() {
        let dir  = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ns_bad_{}.seg", std::process::id()));
        let mut s = NodeStore::new();
        s.create();
        s.save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        assert!(NodeStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_record_crc() {
        let dir  = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_ns_crc_{}.seg", std::process::id()));
        let mut s = NodeStore::new();
        s.create_typed(0);
        s.save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        // Flip a bit inside the record region (just after the header).
        bytes[HEADER_LEN] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        assert!(NodeStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn no_id_sentinel_never_allocated() {
        // Verify NO_ID cannot be reached by normal create() within any
        // reasonable store size.
        assert_eq!(NO_ID, NodeId::MAX);
        // A store of 3 nodes has IDs 0-2, never MAX.
        let mut s = NodeStore::new();
        for _ in 0..3 {
            let id = s.create();
            assert_ne!(id, NO_ID);
        }
    }
}
