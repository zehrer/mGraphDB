# mGraphDB

An embedded graph database published under the MIT License.

Designed to be small enough to embed (SQLite-like footprint) while exposing
several abstraction layers. The data model is uniform: **everything is a node** —
edges and properties are specialisations of a node.

**Status:** early development — Graph API, Persistent Layer (Node/Property/Edge
stores + CSR index), and String Store implemented.

## Project goals

- **Rust** primary implementation (performance, memory safety)
- **Swift** planned for direct iOS / macOS integration
- Minimal, embeddable footprint
- No `unsafe` in the project's own crates

Full design documentation lives in the [wiki](https://github.com/zehrer/mGraphDB/wiki).

## Architecture

```
Graph API            create / read / traverse nodes, props, edges   ✓ implemented
    │
Persistent Layer     fixed-length node/edge/property records + CSR index  ✓ implemented
    │
String Store         append-only deduplicated UTF-8 storage          ✓ implemented
```

## Graph API

`mgraphdb::graph::Graph` ties the layers together. Edges are just `Edge`-valued
properties — "everything is a node". Short strings are stored inline; longer
ones are routed into the String Store automatically.

```rust
use mgraphdb::graph::Graph;
use mgraphdb::prop_store::PropValue;

let mut g = Graph::new();

let person = g.add_node();              // a class node, used as a type
let alice  = g.add_typed_node(person);
let bob    = g.add_typed_node(person);

g.set_str(alice, "Alice")?;             // inline (≤ 14 bytes)
g.set_property(alice, &PropValue::I64(30))?;
g.set_str(bob, "Bob")?;
g.add_edge(alice, bob)?;                // Alice → Bob

assert_eq!(g.out_edges(alice)?, vec![bob]);
assert_eq!(g.properties(alice)?.len(), 2);   // name + age (edge excluded)

g.save("graph_dir")?;                   // nodes + props + strings + CSR index
let reloaded = Graph::open("graph_dir")?;
```

`cargo run` builds and traverses a small social graph end to end.

## String Store

Interns UTF-8 strings and addresses them two ways:
- `StrId` (u64) — compact internal reference
- `StrHash` (xxh3-128) — content hash for dedup and cross-segment identity

Block compression is selectable at creation time:

```rust
use mgraphdb::string_store::{Compression, StringStore};

// No compression (default)
let mut store = StringStore::new();

// LZ4 — pure Rust, fast
let mut store = StringStore::new().with_compression(Compression::Lz4);

// Zstd — better ratio, especially on URI-heavy data
let mut store = StringStore::new().with_compression(Compression::Zstd);

let (hash, id) = store.intern("https://schema.org/Person");
assert_eq!(store.resolve_id(id), Some("https://schema.org/Person"));

store.save("my.seg")?;
let loaded = StringStore::open("my.seg")?;
```

## Build

**Prerequisites:** Rust 1.94+ / Cargo (edition 2024)

| Command | Description |
|---|---|
| `cargo build` | Debug build |
| `cargo build --release` | Optimised build |
| `cargo test` | Run unit tests |
| `cargo clippy --all-targets -- -D warnings` | Lint gate (zero warnings) |
| `cargo run` | Run the Graph API demo |
| `cargo bench --bench string_store` | String Store benchmarks (intern, resolve, save/open) |
| `cargo bench --bench graph` | Graph benchmarks (build, traverse, index, persist) |
| `cargo bench -- compression_ratio` | Compression ratio table (LZ4 / Zstd / none) |

## Commit conventions

This repo uses [Conventional Commits](https://www.conventionalcommits.org/),
enforced by a `commit-msg` hook under `.githooks/`.

```
<type>(<scope>): <subject>
Types: feat | fix | docs | style | refactor | perf | test | build | ci | chore | revert
```

If hooks ever stop running: `chmod +x .githooks/*`

## License

MIT — see [LICENSE](LICENSE).
