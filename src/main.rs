//! Small demo: build a tiny social graph with the Graph API and traverse it.

use mgraphdb::graph::Graph;
use mgraphdb::prop_store::PropValue;

fn main() -> std::io::Result<()> {
    let mut g = Graph::new();

    // A class node used as a type, then three typed instances.
    let person = g.add_node();
    let alice = g.add_typed_node(person);
    let bob = g.add_typed_node(person);
    let carol = g.add_typed_node(person);

    // Properties (short strings inline; long ones route to the String Store).
    g.set_str(alice, "Alice")?;
    g.set_property(alice, &PropValue::I64(30))?;
    g.set_str(bob, "Bob")?;
    g.set_str(carol, "Carol")?;
    g.set_str(alice, "Alice lives in a city with a rather long descriptive name")?;

    // Edges: Alice → Bob, Alice → Carol, Bob → Carol.
    g.add_edge(alice, bob)?;
    g.add_edge(alice, carol)?;
    g.add_edge(bob, carol)?;

    let names = |g: &Graph, n| g.string_value(g.neighbors(n)[0]).unwrap().unwrap();

    println!("nodes: {}  records: {}", g.node_count(), g.record_count());
    for &(node, label) in &[(alice, "alice"), (bob, "bob"), (carol, "carol")] {
        let outs: Vec<String> = g
            .out_edges(node)?
            .iter()
            .map(|&t| names(&g, t))
            .collect();
        println!(
            "{label:>5} ({}) degree={} → knows {:?}",
            names(&g, node),
            g.degree(node),
            outs
        );
    }

    // Incoming edges: who knows Carol?
    let into_carol: Vec<String> = g.in_neighbors(carol).iter().map(|&s| names(&g, s)).collect();
    println!("carol in-degree={} ← known by {:?}", g.in_degree(carol), into_carol);

    // Show the auto-routed long string resolves transparently.
    let long_pid = g.neighbors(alice)[2]; // 3rd record on alice (the long string)
    println!("alice long prop: {:?}", g.string_value(long_pid)?);

    // Export the CSR index that backs traversal.
    let idx = g.build_index()?;
    println!("index: {} nodes, {} edges/props", idx.node_count(), idx.edge_count());

    Ok(())
}
