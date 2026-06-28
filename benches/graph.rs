//! Graph API benchmarks: build, traverse, index, persist.
//!
//! Uses a deterministic LCG for reproducible edge wiring (no `rand` dependency).

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mgraphdb::graph::Graph;
use mgraphdb::prop_store::{PropValue, NO_KEY};

/// A handful of names: most inline (≤14 bytes), one long to exercise routing.
const NAMES: &[&str] = &[
    "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Heidi",
    "a considerably longer display name that overflows inline storage",
];

/// Tiny deterministic LCG (Numerical Recipes constants).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Build a graph with `n` nodes; each node gets a name property and
/// `edges_per_node` outgoing edges to pseudo-random targets.
fn build(n: u32, edges_per_node: u32) -> Graph {
    let mut g = Graph::new();
    for _ in 0..n {
        g.add_node();
    }
    let mut rng = Lcg(0x1234_5678);
    for node in 0..n {
        g.set_str(node, NO_KEY, NAMES[(node as usize) % NAMES.len()]).unwrap();
        if node % 7 == 0 {
            g.set_property(node, NO_KEY, &PropValue::I64(node as i64)).unwrap();
        }
        for _ in 0..edges_per_node {
            let target = (rng.next() as u32) % n;
            g.add_edge(node, NO_KEY, target).unwrap();
        }
    }
    g
}

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_build");
    for &n in &[1_000u32, 10_000] {
        let edges_per_node = 4;
        // Throughput = total elements written (nodes + ~props + edges).
        group.throughput(Throughput::Elements((n * (1 + edges_per_node)) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| build(n, edges_per_node));
        });
    }
    group.finish();
}

fn bench_traverse(c: &mut Criterion) {
    let n = 10_000u32;
    let g = build(n, 4);
    let mut group = c.benchmark_group("graph_traverse");
    group.throughput(Throughput::Elements(n as u64));
    group.bench_function("out_edges_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for node in 0..n {
                total += g.out_edges(node).unwrap().len();
            }
            total
        });
    });
    group.bench_function("degree_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for node in 0..n {
                total += g.degree(node);
            }
            total
        });
    });
    group.bench_function("in_edges_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for node in 0..n {
                total += g.in_degree(node); // precomputed reverse adjacency
            }
            total
        });
    });
    group.finish();
}

fn bench_build_index(c: &mut Criterion) {
    let n = 10_000u32;
    let g = build(n, 4);
    let mut group = c.benchmark_group("graph_index");
    group.throughput(Throughput::Elements(g.record_count() as u64));
    group.bench_function("build_index", |b| {
        b.iter(|| g.build_index().unwrap());
    });
    group.finish();
}

fn bench_find(c: &mut Criterion) {
    let n = 10_000u32;
    let g = build(n, 4);
    let mut group = c.benchmark_group("graph_find");
    // A find is a full scan over every owned record.
    group.throughput(Throughput::Elements(g.record_count() as u64));
    group.bench_function("find_by_str", |b| {
        b.iter(|| g.find_by_str(NO_KEY, "Alice").unwrap().len());
    });
    group.bench_function("find_by_property_i64", |b| {
        b.iter(|| g.find_by_property(NO_KEY, &PropValue::I64(42)).unwrap().len());
    });
    group.finish();
}

fn bench_save_open(c: &mut Criterion) {
    let n = 10_000u32;
    let g = build(n, 4);
    let dir = std::env::temp_dir().join("mgraphdb_bench_graph");

    let mut group = c.benchmark_group("graph_persist");
    group.bench_function("save", |b| {
        b.iter(|| g.save(&dir).unwrap());
    });
    g.save(&dir).unwrap();
    group.bench_function("open", |b| {
        b.iter(|| Graph::open(&dir).unwrap());
    });
    group.finish();

    std::fs::remove_dir_all(&dir).ok();
}

/// Benchmark on a real-world graph (SNAP ca-HepTh by default): a clustered,
/// power-law collaboration network — very different from the uniform-random
/// `build()` wiring above.
///
/// The dataset is not committed. Place the plain-text edge list at
/// `benches/data/ca-HepTh.txt` (or set `MGRAPHDB_GRAPH` to a path); if it is
/// absent this group is skipped. See `benches/data/README.md`.
fn bench_realworld(c: &mut Criterion) {
    let path = std::env::var("MGRAPHDB_GRAPH")
        .unwrap_or_else(|_| "benches/data/ca-HepTh.txt".to_string());
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => {
            eprintln!(
                "graph_realworld: skipped — dataset not found at {path} \
                 (set MGRAPHDB_GRAPH or see benches/data/README.md)"
            );
            return;
        }
    };
    let g = Graph::from_edge_list(std::io::BufReader::new(file)).unwrap();
    eprintln!(
        "graph_realworld: loaded {} nodes, {} edges from {path}",
        g.node_count(),
        g.record_count()
    );

    let n = g.node_count() as u32;
    let mut group = c.benchmark_group("graph_realworld");
    group.throughput(Throughput::Elements(g.record_count() as u64));
    group.bench_function("traverse_out_all", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for node in 0..n {
                total += g.out_edges(node).unwrap().len();
            }
            total
        });
    });
    group.bench_function("build_index", |b| b.iter(|| g.build_index().unwrap()));
    group.finish();
}

criterion_group!(
    benches,
    bench_build,
    bench_traverse,
    bench_build_index,
    bench_find,
    bench_realworld,
    bench_save_open
);
criterion_main!(benches);
