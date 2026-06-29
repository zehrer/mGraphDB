# Benchmark datasets

The `graph_realworld` benchmark (in `benches/graph.rs`) runs on a real-world
graph. The dataset itself is **not committed** — download it locally.

## Default: SNAP ca-HepTh

A High-Energy-Physics-Theory collaboration network: **9,877 nodes, 25,998
undirected edges**, with the clustered, power-law degree distribution typical of
real social/collaboration graphs. The file lists each undirected edge in both
directions, so `Graph::from_edge_list` loads it as a symmetric directed graph.

```sh
cd benches/data
curl -L -O https://snap.stanford.edu/data/ca-HepTh.txt.gz
gunzip ca-HepTh.txt.gz        # → ca-HepTh.txt
```

Then run:

```sh
cargo bench --bench graph -- graph_realworld
```

If the file is absent, the `graph_realworld` group is skipped (the rest of the
benchmarks still run).

## Using a different graph

`Graph::from_edge_list` accepts any SNAP-style edge list — one
`from<whitespace>to` integer pair per line, `#` comments skipped. Point the
benchmark at another file with:

```sh
MGRAPHDB_GRAPH=/path/to/edges.txt cargo bench --bench graph -- graph_realworld
```

Good alternatives from the [SNAP collection](https://snap.stanford.edu/data/):
`p2p-Gnutella04` (directed, ~10.9k/40k), `email-Eu-core`, `Wiki-Vote`.

Source: <https://snap.stanford.edu/data/ca-HepTh.html>
