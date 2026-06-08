# mGraphDB

An embedded graph database published under the MIT License.

Designed to be small enough to embed (SQLite-like footprint) while exposing
several abstraction layers. The data model is uniform: **everything is a node** —
edges and properties are specialisations of a node.

**Status:** early development — String Store implemented, Persistent Layer in design.

## Project goals

- **Rust** primary implementation (performance, memory safety)
- **Swift** planned for direct iOS / macOS integration
- Minimal, embeddable footprint
- No `unsafe` in the project's own crates

Full design documentation lives in the [wiki](https://github.com/zehrer/mGraphDB/wiki).

## Architecture

```
Graph API            (planned)
    │
Persistent Layer     fixed-length node / edge / property records  (design)
    │
String Store         append-only deduplicated UTF-8 storage        ✓ implemented
```

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
| `cargo bench` | Criterion benchmarks (intern, resolve, save/open) |
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
