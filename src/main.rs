//! Small demo: build a tiny social graph with the keyed Graph API and traverse it.

use mgraphdb::graph::Graph;
use mgraphdb::prop_store::PropValue;

fn main() -> std::io::Result<()> {
    let mut g = Graph::new();

    // Key / predicate nodes — "everything is a node".
    let person = g.add_node(); // class node, used as a type
    let name = g.add_node();
    let age = g.add_node();
    let bio = g.add_node();
    let knows = g.add_node(); // edge predicate

    // Three typed instances.
    let alice = g.add_typed_node(person);
    let bob = g.add_typed_node(person);
    let carol = g.add_typed_node(person);

    // Keyed properties (short strings inline; long ones route to the String Store).
    g.set_str(alice, name, "Alice")?;
    g.set_property(alice, age, &PropValue::I64(30))?;
    g.set_str(alice, bio, "Alice lives in a city with a rather long descriptive name")?;
    g.set_str(bob, name, "Bob")?;
    g.set_str(carol, name, "Carol")?;

    // Labelled edges: Alice KNOWS Bob, Alice KNOWS Carol, Bob KNOWS Carol.
    g.add_edge(alice, knows, bob)?;
    g.add_edge(alice, knows, carol)?;
    g.add_edge(bob, knows, carol)?;

    let name_of = |g: &Graph, n| g.get_str(n, name).unwrap().unwrap();

    println!("nodes: {}  records: {}", g.node_count(), g.record_count());
    for &(node, label) in &[(alice, "alice"), (bob, "bob"), (carol, "carol")] {
        let outs: Vec<String> = g.edges_of_type(node, knows)?.iter().map(|&t| name_of(&g, t)).collect();
        println!("{label:>5} ({}) knows {:?}", name_of(&g, node), outs);
    }

    // Keyed lookups.
    println!("alice age: {:?}", g.get_property(alice, age)?.map(|(_, v)| v));
    println!("alice bio: {:?}", g.get_str(alice, bio)?); // long → routed, resolved transparently

    // Incoming edges: who knows Carol?
    let into_carol: Vec<String> = g.in_neighbors(carol).iter().map(|&s| name_of(&g, s)).collect();
    println!("carol in-degree={} ← known by {:?}", g.in_degree(carol), into_carol);

    // Delete Bob: his edges are detached in both directions.
    g.delete_node(bob)?;
    println!(
        "after deleting bob: live nodes={}  carol known by {:?}",
        g.live_node_count(),
        g.in_neighbors(carol).iter().map(|&s| name_of(&g, s)).collect::<Vec<_>>(),
    );

    // Export the CSR index that backs traversal.
    let idx = g.build_index()?;
    println!("index: {} nodes, {} edges/props", idx.node_count(), idx.edge_count());

    Ok(())
}
