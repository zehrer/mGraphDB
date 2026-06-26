//! Graph API — the public surface that ties the storage layers together.
//!
//! A [`Graph`] owns the four lower layers and presents a small, ergonomic API
//! for creating nodes, attaching properties, and linking nodes with edges:
//!
//! * [`NodeStore`](crate::node_store) — one fixed 12-byte record per node.
//! * [`PropStore`](crate::prop_store) — one fixed 16-byte record per property
//!   or edge.
//! * [`StringStore`](crate::string_store) — deduplicated backing for strings
//!   too long to inline (REQ-22 / REQ-37 routing happens here).
//! * A live **adjacency** (`Vec<Vec<PropId>>`) giving O(1) edge/property
//!   insertion and iteration while the graph is mutated. It is exported to an
//!   immutable [`IndexStore`](crate::index_store) (CSR) for persistence via
//!   [`Graph::build_index`].
//!
//! "Everything is a node": an edge is just a property whose value is
//! [`PropValue::Edge`], owned by its start node and pointing at its end node.

use crate::index_store::{IndexBuilder, IndexStore};
use crate::node_store::{NodeFlags, NodeId, NodeStore};
use crate::prop_store::{PropId, PropStore, PropValue, INLINE_STR_MAX};
use crate::string_store::{Compression, StringStore};
use std::io;
use std::path::Path;

/// The in-memory graph: nodes, properties/edges, strings, and live adjacency.
pub struct Graph {
    nodes: NodeStore,
    props: PropStore,
    strings: StringStore,
    /// `adjacency[node]` = the PropIds owned by `node`, in insertion order.
    adjacency: Vec<Vec<PropId>>,
}

impl Default for Graph {
    fn default() -> Self { Self::new() }
}

impl Graph {
    /// Create an empty graph (strings uncompressed).
    pub fn new() -> Self {
        Graph {
            nodes: NodeStore::new(),
            props: PropStore::new(),
            strings: StringStore::new(),
            adjacency: Vec::new(),
        }
    }

    /// Create an empty graph whose String Store uses `compression`.
    pub fn with_compression(compression: Compression) -> Self {
        Graph {
            nodes: NodeStore::new(),
            props: PropStore::new(),
            strings: StringStore::new().with_compression(compression),
            adjacency: Vec::new(),
        }
    }

    // ── Nodes ──────────────────────────────────────────────────────────────

    /// Number of nodes (including tombstoned).
    pub fn node_count(&self) -> usize { self.nodes.len() }

    /// Number of property/edge records.
    pub fn record_count(&self) -> usize { self.props.len() }

    /// Create a plain node and return its id.
    pub fn add_node(&mut self) -> NodeId {
        let id = self.nodes.create();
        self.adjacency.push(Vec::new());
        id
    }

    /// Create a node typed as the class node `type_id`.
    pub fn add_typed_node(&mut self, type_id: NodeId) -> NodeId {
        let id = self.nodes.create_typed(type_id);
        self.adjacency.push(Vec::new());
        id
    }

    // ── Properties ─────────────────────────────────────────────────────────

    /// Attach a typed property `value` to `node`. Returns the new `PropId`.
    pub fn set_property(&mut self, node: NodeId, value: &PropValue) -> io::Result<PropId> {
        self.check_node(node)?;
        let pid = self.props.create(value)?;
        self.adjacency[node as usize].push(pid);
        self.set_flag(node, NodeFlags::HAS_PROP)?;
        Ok(pid)
    }

    /// Attach a string property, **auto-routing** by length: strings up to
    /// [`INLINE_STR_MAX`] bytes are stored inline; longer strings are interned
    /// into the String Store and referenced by `StrId` (REQ-22 / REQ-37).
    pub fn set_str(&mut self, node: NodeId, s: &str) -> io::Result<PropId> {
        let value = if s.len() <= INLINE_STR_MAX {
            PropValue::InlineStr(s.to_owned())
        } else {
            let (_, id) = self.strings.intern(s);
            PropValue::StringRef(id)
        };
        self.set_property(node, &value)
    }

    // ── Edges ──────────────────────────────────────────────────────────────

    /// Add a directed edge `from → to`. Returns the edge's `PropId`.
    ///
    /// The edge is owned by `from`; `from` gains `HAS_OUT` and `to` gains
    /// `HAS_IN`.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId) -> io::Result<PropId> {
        self.check_node(from)?;
        self.check_node(to)?;
        let pid = self.props.create(&PropValue::Edge { end_node: to })?;
        self.adjacency[from as usize].push(pid);
        self.set_flag(from, NodeFlags::HAS_OUT)?;
        self.set_flag(to, NodeFlags::HAS_IN)?;
        Ok(pid)
    }

    // ── Reads / traversal ──────────────────────────────────────────────────

    /// The property/edge records owned by `node`, in insertion order.
    pub fn neighbors(&self, node: NodeId) -> &[PropId] {
        match self.adjacency.get(node as usize) {
            Some(v) => v,
            None => &[],
        }
    }

    /// Number of records (properties + edges) owned by `node`.
    pub fn degree(&self, node: NodeId) -> usize {
        self.neighbors(node).len()
    }

    /// Decode the value of property/edge record `pid`.
    pub fn value(&self, pid: PropId) -> io::Result<Option<PropValue>> {
        self.props.get(pid)
    }

    /// Resolve a string-valued property to an owned `String`, transparently
    /// handling inline storage and String Store references. Returns `None` if
    /// the record is not a string type.
    pub fn string_value(&self, pid: PropId) -> io::Result<Option<String>> {
        Ok(match self.props.get(pid)? {
            Some(PropValue::InlineStr(s)) => Some(s),
            Some(PropValue::StringRef(id)) | Some(PropValue::UrlRef(id)) => {
                self.strings.resolve_id(id).map(str::to_owned)
            }
            _ => None,
        })
    }

    /// The end nodes of all outgoing edges of `node`, in insertion order.
    pub fn out_edges(&self, node: NodeId) -> io::Result<Vec<NodeId>> {
        let mut out = Vec::new();
        for &pid in self.neighbors(node) {
            if let Some(PropValue::Edge { end_node }) = self.props.get(pid)? {
                out.push(end_node);
            }
        }
        Ok(out)
    }

    /// All non-edge properties of `node` as `(PropId, value)` pairs.
    pub fn properties(&self, node: NodeId) -> io::Result<Vec<(PropId, PropValue)>> {
        let mut out = Vec::new();
        for &pid in self.neighbors(node) {
            match self.props.get(pid)? {
                Some(PropValue::Edge { .. }) | None => {}
                Some(v) => out.push((pid, v)),
            }
        }
        Ok(out)
    }

    // ── Index / persistence ────────────────────────────────────────────────

    /// Export the live adjacency as an immutable CSR [`IndexStore`].
    pub fn build_index(&self) -> io::Result<IndexStore> {
        let mut b = IndexBuilder::new();
        for (node, records) in self.adjacency.iter().enumerate() {
            for &pid in records {
                b.add(node as NodeId, pid);
            }
        }
        b.build(self.nodes.len())
    }

    /// Persist all four segments into `dir`: `nodes.seg`, `props.seg`,
    /// `strings.seg`, and `graph.idx`.
    pub fn save(&self, dir: impl AsRef<Path>) -> io::Result<()> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        self.nodes.save(dir.join("nodes.seg"))?;
        self.props.save(dir.join("props.seg"))?;
        self.strings.save(dir.join("strings.seg"))?;
        self.build_index()?.save(dir.join("graph.idx"))?;
        Ok(())
    }

    /// Load a graph previously written by [`Graph::save`], rebuilding the live
    /// adjacency from the persisted CSR index.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref();
        let nodes = NodeStore::open(dir.join("nodes.seg"))?;
        let props = PropStore::open(dir.join("props.seg"))?;
        let strings = StringStore::open(dir.join("strings.seg"))?;
        let index = IndexStore::open(dir.join("graph.idx"))?;

        let mut adjacency = vec![Vec::new(); nodes.len()];
        for (node, slot) in adjacency.iter_mut().enumerate() {
            slot.extend_from_slice(index.neighbors(node as NodeId));
        }
        Ok(Graph { nodes, props, strings, adjacency })
    }

    // ── Internals ──────────────────────────────────────────────────────────

    fn check_node(&self, node: NodeId) -> io::Result<()> {
        if (node as usize) < self.nodes.len() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("graph: node {node} out of range (count {})", self.nodes.len()),
            ))
        }
    }

    fn set_flag(&mut self, node: NodeId, bit: u8) -> io::Result<()> {
        let mut rec = self.nodes.get(node).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("graph: node {node} missing"))
        })?;
        rec.flags = rec.flags.set(bit);
        self.nodes.update(node, rec);
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_a_small_graph() {
        let mut g = Graph::new();
        let person = g.add_node();           // class node
        let alice = g.add_typed_node(person);
        let bob = g.add_typed_node(person);

        g.set_str(alice, "Alice").unwrap();
        g.set_property(alice, &PropValue::I64(30)).unwrap();
        g.set_str(bob, "Bob").unwrap();
        let _knows = g.add_edge(alice, bob).unwrap();

        assert_eq!(g.node_count(), 3);
        // alice owns: name, age, edge → degree 3
        assert_eq!(g.degree(alice), 3);
        assert_eq!(g.out_edges(alice).unwrap(), vec![bob]);
        assert_eq!(g.out_edges(bob).unwrap(), vec![]);

        let props = g.properties(alice).unwrap();
        assert_eq!(props.len(), 2); // name + age, edge excluded
    }

    #[test]
    fn long_string_routes_to_string_store() {
        let mut g = Graph::new();
        let n = g.add_node();
        let short = "hi";
        let long = "this is definitely longer than fourteen bytes";

        let p_short = g.set_str(n, short).unwrap();
        let p_long = g.set_str(n, long).unwrap();

        // short stays inline, long becomes a StringRef
        assert!(matches!(g.value(p_short).unwrap(), Some(PropValue::InlineStr(_))));
        assert!(matches!(g.value(p_long).unwrap(), Some(PropValue::StringRef(_))));

        // both resolve back to the original text transparently
        assert_eq!(g.string_value(p_short).unwrap().as_deref(), Some(short));
        assert_eq!(g.string_value(p_long).unwrap().as_deref(), Some(long));
    }

    #[test]
    fn edge_sets_in_out_flags() {
        let mut g = Graph::new();
        let a = g.add_node();
        let b = g.add_node();
        g.add_edge(a, b).unwrap();
        assert!(g.nodes.get(a).unwrap().flags.has_out());
        assert!(g.nodes.get(b).unwrap().flags.has_in());
        assert!(!g.nodes.get(a).unwrap().flags.has_in());
    }

    #[test]
    fn operations_on_unknown_node_error() {
        let mut g = Graph::new();
        assert!(g.set_property(5, &PropValue::None).is_err());
        assert!(g.add_edge(0, 1).is_err());
    }

    #[test]
    fn build_index_matches_adjacency() {
        let mut g = Graph::new();
        let a = g.add_node();
        let b = g.add_node();
        g.set_str(a, "x").unwrap();
        g.add_edge(a, b).unwrap();

        let idx = g.build_index().unwrap();
        assert_eq!(idx.node_count(), 2);
        assert_eq!(idx.neighbors(a), g.neighbors(a));
        assert_eq!(idx.degree(b), 0);
    }

    #[test]
    fn save_open_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mgraphdb_graph_{}", std::process::id()));

        let mut g = Graph::new();
        let person = g.add_node();
        let alice = g.add_typed_node(person);
        let bob = g.add_typed_node(person);
        g.set_str(alice, "Alice").unwrap();
        g.set_str(bob, "a name far longer than fourteen bytes for routing").unwrap();
        g.add_edge(alice, bob).unwrap();
        g.save(&dir).unwrap();

        let loaded = Graph::open(&dir).unwrap();
        assert_eq!(loaded.node_count(), 3);
        assert_eq!(loaded.degree(alice), 2); // name + edge
        assert_eq!(loaded.out_edges(alice).unwrap(), vec![bob]);
        assert_eq!(loaded.string_value(loaded.neighbors(alice)[0]).unwrap().as_deref(), Some("Alice"));
        assert_eq!(
            loaded.string_value(loaded.neighbors(bob)[0]).unwrap().as_deref(),
            Some("a name far longer than fourteen bytes for routing")
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
