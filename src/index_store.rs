//! Compressed-sparse-row (CSR) adjacency index.
//!
//! The index is a **derived view** that maps each `NodeId` to the list of
//! property/edge records it owns in the [`PropStore`](crate::prop_store). The
//! storage layer itself stores no owner→record mapping (a `PropStore` record
//! spends all 14 payload bytes on its value, and the Small Profile keeps no
//! per-record `next` pointer), so the association is supplied explicitly via
//! [`IndexBuilder`] — in practice by the Graph API as it creates edges and
//! properties.
//!
//! Because it is derived, the index can be rebuilt from the stores at any time,
//! kept in memory only, or dropped to save space.
//!
//! ## CSR layout
//!
//! Two parallel arrays:
//! * `offsets` — `node_count + 1` entries. Node `n`'s records occupy
//!   `neighbors[offsets[n] .. offsets[n + 1]]`. The trailing entry equals the
//!   total record count, so every node (even the last) has a valid half-open
//!   range and no bounds special-casing is needed.
//! * `neighbors` — every owned `PropId`, grouped by owner, in the order the
//!   associations were added.
//!
//! Looking up a node's neighbours is therefore a contiguous slice — cache
//! friendly and allocation free.
//!
//! ## On-disk segment layout (little-endian throughout)
//! ```text
//! [Header   16 B]   magic(4) + version(2) + flags(2) + id_bytes(1) + pad(7)
//! [offsets  4 B]*   (node_count + 1) u32 entries
//! [neighbors 4 B]*  edge_count u32 entries (PropIds)
//! [Footer   24 B]   node_count(8) + edge_count(8) + crc32(4) + magic(4)
//! ```
//! On `open`, the CRC32 of the offsets+neighbors region is verified and the
//! header/footer magics are checked.

use crate::node_store::NodeId;
use crate::prop_store::PropId;
use std::io;
use std::path::Path;

const HEADER_MAGIC: u32 = u32::from_le_bytes(*b"MGIS"); // mGraphDB Index Store
const FOOTER_MAGIC: u32 = u32::from_le_bytes(*b"MGIF"); // mGraphDB Index Footer
const FORMAT_VERSION: u16 = 1;
const ID_BYTES: u8 = 4;
const HEADER_LEN: usize = 16;
const FOOTER_LEN: usize = 24;

// ── IndexBuilder ───────────────────────────────────────────────────────────────

/// Accumulates `(owner, record)` associations and builds an [`IndexStore`].
///
/// Add order is preserved within each owner's neighbour list.
#[derive(Default)]
pub struct IndexBuilder {
    pairs: Vec<(NodeId, PropId)>,
}

impl IndexBuilder {
    pub fn new() -> Self {
        IndexBuilder { pairs: Vec::new() }
    }

    /// Record that `owner` owns property/edge record `prop`.
    pub fn add(&mut self, owner: NodeId, prop: PropId) -> &mut Self {
        self.pairs.push((owner, prop));
        self
    }

    /// Number of associations added so far.
    pub fn len(&self) -> usize { self.pairs.len() }

    pub fn is_empty(&self) -> bool { self.pairs.is_empty() }

    /// Build the CSR index for a graph of `node_count` nodes (typically
    /// `node_store.len()`).
    ///
    /// Returns an error if any association references an owner `>= node_count`.
    pub fn build(&self, node_count: usize) -> io::Result<IndexStore> {
        // Degree per node.
        let mut offsets = vec![0u32; node_count + 1];
        for &(owner, _) in &self.pairs {
            let o = owner as usize;
            if o >= node_count {
                return Err(bad(&format!(
                    "owner {owner} out of range for node_count {node_count}"
                )));
            }
            offsets[o + 1] += 1;
        }

        // Prefix sum → start offset of each node's run.
        for i in 0..node_count {
            offsets[i + 1] += offsets[i];
        }

        // Scatter PropIds into their owner's run, preserving add order.
        let mut neighbors = vec![0u32; self.pairs.len()];
        let mut cursor: Vec<u32> = offsets[..node_count].to_vec();
        for &(owner, prop) in &self.pairs {
            let slot = cursor[owner as usize] as usize;
            neighbors[slot] = prop;
            cursor[owner as usize] += 1;
        }

        Ok(IndexStore { offsets, neighbors })
    }
}

// ── IndexStore ─────────────────────────────────────────────────────────────────

/// Immutable CSR adjacency index. Build with [`IndexBuilder`].
pub struct IndexStore {
    offsets: Vec<u32>,
    neighbors: Vec<PropId>,
}

impl IndexStore {
    /// Number of nodes the index was built for.
    pub fn node_count(&self) -> usize {
        // offsets always has node_count + 1 entries (≥ 1 after a build).
        self.offsets.len().saturating_sub(1)
    }

    /// Total number of owned records across all nodes.
    pub fn edge_count(&self) -> usize { self.neighbors.len() }

    /// The property/edge records owned by `node`, in insertion order.
    ///
    /// Returns an empty slice for an unknown node or one with no records.
    pub fn neighbors(&self, node: NodeId) -> &[PropId] {
        let n = node as usize;
        if n >= self.node_count() {
            return &[];
        }
        let start = self.offsets[n] as usize;
        let end = self.offsets[n + 1] as usize;
        &self.neighbors[start..end]
    }

    /// Number of records owned by `node`.
    pub fn degree(&self, node: NodeId) -> usize {
        self.neighbors(node).len()
    }

    /// Serialize to a segment file via temp + rename.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let body_len = (self.offsets.len() + self.neighbors.len()) * 4;
        let mut buf = Vec::with_capacity(HEADER_LEN + body_len + FOOTER_LEN);

        // Header
        buf.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // flags
        buf.push(ID_BYTES);
        buf.extend_from_slice(&[0u8; 7]); // reserved

        // Body
        let body_start = buf.len();
        for &o in &self.offsets {
            buf.extend_from_slice(&o.to_le_bytes());
        }
        for &p in &self.neighbors {
            buf.extend_from_slice(&p.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf[body_start..]);

        // Footer
        buf.extend_from_slice(&(self.node_count() as u64).to_le_bytes());
        buf.extend_from_slice(&(self.neighbors.len() as u64).to_le_bytes());
        buf.extend_from_slice(&crc.to_le_bytes());
        buf.extend_from_slice(&FOOTER_MAGIC.to_le_bytes());

        let path = path.as_ref();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)
    }

    /// Load from a segment file, verifying all integrity checks.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let data = std::fs::read(path)?;
        if data.len() < HEADER_LEN + FOOTER_LEN {
            return Err(bad("file too small"));
        }
        if read_u32(&data, 0) != HEADER_MAGIC {
            return Err(bad("bad header magic"));
        }
        if read_u16(&data, 4) != FORMAT_VERSION {
            return Err(bad("unsupported format version"));
        }

        let f = data.len() - FOOTER_LEN;
        if read_u32(&data, f + 20) != FOOTER_MAGIC {
            return Err(bad("bad footer magic"));
        }
        let node_count = read_u64(&data, f) as usize;
        let edge_count = read_u64(&data, f + 8) as usize;
        let stored_crc = read_u32(&data, f + 16);

        let offsets_len = node_count + 1;
        let body_len = (offsets_len + edge_count) * 4;
        let body_end = HEADER_LEN + body_len;
        if body_end > f {
            return Err(bad("body region overruns footer"));
        }
        let body = &data[HEADER_LEN..body_end];
        if crc32fast::hash(body) != stored_crc {
            return Err(bad("body crc mismatch"));
        }

        let mut offsets = Vec::with_capacity(offsets_len);
        for i in 0..offsets_len {
            offsets.push(read_u32(body, i * 4));
        }
        let mut neighbors = Vec::with_capacity(edge_count);
        let base = offsets_len * 4;
        for i in 0..edge_count {
            neighbors.push(read_u32(body, base + i * 4));
        }

        // Structural sanity: offsets must be non-decreasing and end at edge_count.
        if offsets[node_count] as usize != edge_count {
            return Err(bad("offsets tail does not match edge_count"));
        }
        for w in offsets.windows(2) {
            if w[1] < w[0] {
                return Err(bad("offsets are not monotonic"));
            }
        }

        Ok(IndexStore { offsets, neighbors })
    }
}

// ── Low-level helpers ───────────────────────────────────────────────────────────

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("index store: {msg}"))
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

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_groups_records_by_owner() {
        let mut b = IndexBuilder::new();
        // node 0 owns props 10, 11; node 2 owns prop 20; node 1 owns nothing.
        b.add(0, 10).add(2, 20).add(0, 11);
        let idx = b.build(3).unwrap();

        assert_eq!(idx.node_count(), 3);
        assert_eq!(idx.edge_count(), 3);
        assert_eq!(idx.neighbors(0), &[10, 11]); // add order preserved
        assert_eq!(idx.neighbors(1), &[] as &[PropId]);
        assert_eq!(idx.neighbors(2), &[20]);
        assert_eq!(idx.degree(0), 2);
        assert_eq!(idx.degree(1), 0);
    }

    #[test]
    fn unknown_node_returns_empty() {
        let idx = IndexBuilder::new().build(2).unwrap();
        assert_eq!(idx.neighbors(0), &[] as &[PropId]);
        assert_eq!(idx.neighbors(99), &[] as &[PropId]); // out of range
        assert_eq!(idx.degree(99), 0);
    }

    #[test]
    fn empty_graph() {
        let idx = IndexBuilder::new().build(0).unwrap();
        assert_eq!(idx.node_count(), 0);
        assert_eq!(idx.edge_count(), 0);
        assert_eq!(idx.neighbors(0), &[] as &[PropId]);
    }

    #[test]
    fn owner_out_of_range_is_error() {
        let mut b = IndexBuilder::new();
        b.add(5, 1);
        assert!(b.build(3).is_err());
    }

    #[test]
    fn roundtrip_save_open() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_idx_{}.seg", std::process::id()));

        let mut b = IndexBuilder::new();
        b.add(0, 100).add(0, 101).add(3, 7).add(1, 42);
        let src = b.build(4).unwrap();
        src.save(&path).unwrap();

        let loaded = IndexStore::open(&path).unwrap();
        assert_eq!(loaded.node_count(), 4);
        assert_eq!(loaded.edge_count(), 4);
        assert_eq!(loaded.neighbors(0), &[100, 101]);
        assert_eq!(loaded.neighbors(1), &[42]);
        assert_eq!(loaded.neighbors(2), &[] as &[PropId]);
        assert_eq!(loaded.neighbors(3), &[7]);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_header_magic() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_idx_bad_{}.seg", std::process::id()));
        let mut b = IndexBuilder::new();
        b.add(0, 1);
        b.build(1).unwrap().save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        assert!(IndexStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_rejects_corrupt_body_crc() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mgraphdb_idx_crc_{}.seg", std::process::id()));
        let mut b = IndexBuilder::new();
        b.add(0, 1).add(0, 2);
        b.build(1).unwrap().save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        // Flip a bit inside the body region (just after the header).
        bytes[HEADER_LEN] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        assert!(IndexStore::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
