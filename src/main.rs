use mgraphdb::string_store::StringStore;

fn main() {
    let mut store = StringStore::new();
    let (hash, id) = store.intern("https://example.com");
    println!("interned id={id} hash={hash:?}");
    println!("resolve_id({id}) = {:?}", store.resolve_id(id));
    println!("unique strings: {}", store.len());
}
