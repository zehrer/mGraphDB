use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mgraphdb::string_store::{Compression, StringStore};

// ── Realistic test data ──────────────────────────────────────────────────────

/// 200 common English words — representative of natural-language property values.
const ENGLISH_WORDS: &[&str] = &[
    "the", "be", "to", "of", "and", "a", "in", "that", "have", "it",
    "for", "not", "on", "with", "he", "as", "you", "do", "at", "this",
    "but", "his", "by", "from", "they", "we", "say", "her", "she", "or",
    "an", "will", "my", "one", "all", "would", "there", "their", "what",
    "so", "up", "out", "if", "about", "who", "get", "which", "go", "me",
    "when", "make", "can", "like", "time", "no", "just", "him", "know",
    "take", "people", "into", "year", "your", "good", "some", "could",
    "them", "see", "other", "than", "then", "now", "look", "only", "come",
    "its", "over", "think", "also", "back", "after", "use", "two", "how",
    "our", "work", "first", "well", "way", "even", "new", "want", "because",
    "any", "these", "give", "day", "most", "us", "great", "between", "need",
    "large", "often", "hand", "high", "place", "hold", "turn", "here",
    "why", "help", "put", "different", "away", "again", "off", "should",
    "house", "world", "still", "own", "old", "life", "while", "long", "down",
    "may", "change", "play", "spell", "air", "away", "animal", "house", "point",
    "page", "letter", "mother", "answer", "found", "study", "still", "learn",
    "plant", "cover", "food", "sun", "four", "between", "state", "keep",
    "never", "last", "let", "thought", "city", "tree", "cross", "farm", "hard",
    "start", "might", "story", "saw", "far", "sea", "draw", "left", "late",
    "run", "don't", "while", "press", "close", "night", "real", "life",
    "few", "north", "open", "seem", "together", "next", "white", "children",
    "begin", "got", "walk", "example", "ease", "paper", "group", "always",
    "music", "those", "both", "mark", "book", "carry", "took", "science",
    "eat", "room", "friend", "began", "idea", "fish", "mountain", "stop",
    "once", "base", "hear", "horse", "cut", "sure", "watch", "color",
    "face", "wood", "main", "enough", "plain", "girl", "usual", "young",
    "ready", "above", "ever", "red", "list", "though", "feel", "talk",
    "bird", "soon", "body", "dog", "family", "direct", "pose", "leave",
];

/// Common RDF/RDFS/OWL/XSD/FOAF/Dublin Core/Schema.org URIs.
/// Representative of semantic-web graph database content.
const ONTOLOGY_URIS: &[&str] = &[
    // RDF core
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Statement",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#subject",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#predicate",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#object",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#List",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Bag",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Seq",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Alt",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#value",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral",
    // RDFS
    "http://www.w3.org/2000/01/rdf-schema#Class",
    "http://www.w3.org/2000/01/rdf-schema#subClassOf",
    "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
    "http://www.w3.org/2000/01/rdf-schema#comment",
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2000/01/rdf-schema#domain",
    "http://www.w3.org/2000/01/rdf-schema#range",
    "http://www.w3.org/2000/01/rdf-schema#Resource",
    "http://www.w3.org/2000/01/rdf-schema#Literal",
    "http://www.w3.org/2000/01/rdf-schema#Datatype",
    "http://www.w3.org/2000/01/rdf-schema#Container",
    "http://www.w3.org/2000/01/rdf-schema#member",
    "http://www.w3.org/2000/01/rdf-schema#seeAlso",
    "http://www.w3.org/2000/01/rdf-schema#isDefinedBy",
    // OWL
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2002/07/owl#Thing",
    "http://www.w3.org/2002/07/owl#Nothing",
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
    "http://www.w3.org/2002/07/owl#AnnotationProperty",
    "http://www.w3.org/2002/07/owl#FunctionalProperty",
    "http://www.w3.org/2002/07/owl#InverseFunctionalProperty",
    "http://www.w3.org/2002/07/owl#TransitiveProperty",
    "http://www.w3.org/2002/07/owl#SymmetricProperty",
    "http://www.w3.org/2002/07/owl#AsymmetricProperty",
    "http://www.w3.org/2002/07/owl#ReflexiveProperty",
    "http://www.w3.org/2002/07/owl#IrreflexiveProperty",
    "http://www.w3.org/2002/07/owl#Restriction",
    "http://www.w3.org/2002/07/owl#onProperty",
    "http://www.w3.org/2002/07/owl#allValuesFrom",
    "http://www.w3.org/2002/07/owl#someValuesFrom",
    "http://www.w3.org/2002/07/owl#hasValue",
    "http://www.w3.org/2002/07/owl#equivalentClass",
    "http://www.w3.org/2002/07/owl#equivalentProperty",
    "http://www.w3.org/2002/07/owl#inverseOf",
    "http://www.w3.org/2002/07/owl#disjointWith",
    "http://www.w3.org/2002/07/owl#sameAs",
    "http://www.w3.org/2002/07/owl#differentFrom",
    "http://www.w3.org/2002/07/owl#Ontology",
    "http://www.w3.org/2002/07/owl#imports",
    "http://www.w3.org/2002/07/owl#versionInfo",
    // XSD types
    "http://www.w3.org/2001/XMLSchema#string",
    "http://www.w3.org/2001/XMLSchema#boolean",
    "http://www.w3.org/2001/XMLSchema#integer",
    "http://www.w3.org/2001/XMLSchema#long",
    "http://www.w3.org/2001/XMLSchema#int",
    "http://www.w3.org/2001/XMLSchema#short",
    "http://www.w3.org/2001/XMLSchema#byte",
    "http://www.w3.org/2001/XMLSchema#decimal",
    "http://www.w3.org/2001/XMLSchema#float",
    "http://www.w3.org/2001/XMLSchema#double",
    "http://www.w3.org/2001/XMLSchema#date",
    "http://www.w3.org/2001/XMLSchema#time",
    "http://www.w3.org/2001/XMLSchema#dateTime",
    "http://www.w3.org/2001/XMLSchema#dateTimeStamp",
    "http://www.w3.org/2001/XMLSchema#duration",
    "http://www.w3.org/2001/XMLSchema#anyURI",
    "http://www.w3.org/2001/XMLSchema#hexBinary",
    "http://www.w3.org/2001/XMLSchema#base64Binary",
    "http://www.w3.org/2001/XMLSchema#language",
    "http://www.w3.org/2001/XMLSchema#normalizedString",
    // FOAF
    "http://xmlns.com/foaf/0.1/Person",
    "http://xmlns.com/foaf/0.1/Agent",
    "http://xmlns.com/foaf/0.1/Organization",
    "http://xmlns.com/foaf/0.1/Project",
    "http://xmlns.com/foaf/0.1/name",
    "http://xmlns.com/foaf/0.1/firstName",
    "http://xmlns.com/foaf/0.1/lastName",
    "http://xmlns.com/foaf/0.1/title",
    "http://xmlns.com/foaf/0.1/nick",
    "http://xmlns.com/foaf/0.1/mbox",
    "http://xmlns.com/foaf/0.1/homepage",
    "http://xmlns.com/foaf/0.1/knows",
    "http://xmlns.com/foaf/0.1/member",
    "http://xmlns.com/foaf/0.1/depiction",
    "http://xmlns.com/foaf/0.1/age",
    "http://xmlns.com/foaf/0.1/gender",
    "http://xmlns.com/foaf/0.1/based_near",
    "http://xmlns.com/foaf/0.1/Document",
    "http://xmlns.com/foaf/0.1/Image",
    "http://xmlns.com/foaf/0.1/topic",
    // Dublin Core
    "http://purl.org/dc/elements/1.1/title",
    "http://purl.org/dc/elements/1.1/creator",
    "http://purl.org/dc/elements/1.1/subject",
    "http://purl.org/dc/elements/1.1/description",
    "http://purl.org/dc/elements/1.1/publisher",
    "http://purl.org/dc/elements/1.1/contributor",
    "http://purl.org/dc/elements/1.1/date",
    "http://purl.org/dc/elements/1.1/type",
    "http://purl.org/dc/elements/1.1/format",
    "http://purl.org/dc/elements/1.1/identifier",
    "http://purl.org/dc/elements/1.1/source",
    "http://purl.org/dc/elements/1.1/language",
    "http://purl.org/dc/elements/1.1/relation",
    "http://purl.org/dc/elements/1.1/coverage",
    "http://purl.org/dc/elements/1.1/rights",
    // Schema.org (common types)
    "https://schema.org/Person",
    "https://schema.org/Organization",
    "https://schema.org/Place",
    "https://schema.org/Event",
    "https://schema.org/Product",
    "https://schema.org/CreativeWork",
    "https://schema.org/Article",
    "https://schema.org/WebPage",
    "https://schema.org/name",
    "https://schema.org/description",
    "https://schema.org/url",
    "https://schema.org/identifier",
    "https://schema.org/image",
    "https://schema.org/dateCreated",
    "https://schema.org/dateModified",
    "https://schema.org/author",
    "https://schema.org/email",
    "https://schema.org/telephone",
    "https://schema.org/address",
    "https://schema.org/geo",
];

fn make_mixed() -> Vec<&'static str> {
    let mut v: Vec<&str> = Vec::with_capacity(ENGLISH_WORDS.len() + ONTOLOGY_URIS.len());
    v.extend_from_slice(ENGLISH_WORDS);
    v.extend_from_slice(ONTOLOGY_URIS);
    v
}

// ── Intern throughput ────────────────────────────────────────────────────────

fn bench_intern(c: &mut Criterion) {
    let mixed = make_mixed();
    let datasets: &[(&str, &[&str])] = &[
        ("english_words", ENGLISH_WORDS),
        ("ontology_uris", ONTOLOGY_URIS),
        ("mixed", mixed.as_slice()),
    ];

    let mut group = c.benchmark_group("intern");
    for (name, data) in datasets {
        group.throughput(Throughput::Elements(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("unique", name), data, |b, data| {
            b.iter(|| {
                let mut store = StringStore::new();
                for &s in *data {
                    store.intern(s);
                }
                store
            });
        });
        // Intern the same set twice: tests the fast dedup path.
        group.bench_with_input(BenchmarkId::new("dedup_hit", name), data, |b, data| {
            b.iter(|| {
                let mut store = StringStore::new();
                for &s in *data {
                    store.intern(s);
                }
                for &s in *data {
                    store.intern(s);
                }
                store
            });
        });
    }
    group.finish();
}

// ── Resolve throughput ───────────────────────────────────────────────────────

fn bench_resolve(c: &mut Criterion) {
    let mixed = make_mixed();
    let datasets: &[(&str, &[&str])] = &[
        ("english_words", ENGLISH_WORDS),
        ("ontology_uris", ONTOLOGY_URIS),
        ("mixed", mixed.as_slice()),
    ];

    let mut group = c.benchmark_group("resolve_id");
    for (name, data) in datasets {
        let mut store = StringStore::new();
        let ids: Vec<_> = data.iter().map(|s| store.intern(s).1).collect();

        group.throughput(Throughput::Elements(ids.len() as u64));
        group.bench_with_input(BenchmarkId::new("sequential", name), &ids, |b, ids| {
            b.iter(|| {
                ids.iter()
                    .map(|&id| store.resolve_id(id))
                    .collect::<Vec<_>>()
            });
        });
    }
    group.finish();
}

// ── Save / open roundtrip ────────────────────────────────────────────────────

fn bench_save_open(c: &mut Criterion) {
    let mixed = make_mixed();
    let tmp = std::env::temp_dir();

    let algos = [
        ("none", Compression::None),
        ("lz4", Compression::Lz4),
        ("zstd", Compression::Zstd),
    ];

    let mut save_group = c.benchmark_group("save");
    for (algo_name, algo) in &algos {
        let path = tmp.join(format!("bench_ss_{algo_name}.seg"));
        save_group.throughput(Throughput::Elements(mixed.len() as u64));
        save_group.bench_function(BenchmarkId::new("mixed", algo_name), |b| {
            b.iter(|| {
                let mut store = StringStore::new().with_compression(*algo);
                for &s in &mixed {
                    store.intern(s);
                }
                store.save(&path).unwrap();
            });
        });
    }
    save_group.finish();

    let mut open_group = c.benchmark_group("open");
    for (algo_name, algo) in &algos {
        let path = tmp.join(format!("bench_ss_open_{algo_name}.seg"));
        // Write the file once so we can benchmark just the open.
        let mut store = StringStore::new().with_compression(*algo);
        for &s in &mixed {
            store.intern(s);
        }
        store.save(&path).unwrap();

        open_group.throughput(Throughput::Elements(mixed.len() as u64));
        open_group.bench_function(BenchmarkId::new("mixed", algo_name), |b| {
            b.iter(|| StringStore::open(&path).unwrap());
        });
    }
    open_group.finish();
}

// ── Compression ratio (not a timing benchmark — printed once) ────────────────

fn bench_compression_ratio(c: &mut Criterion) {
    let mixed = make_mixed();
    let tmp = std::env::temp_dir();

    // Single measurement group used only to trigger the ratio report once.
    let mut group = c.benchmark_group("compression_ratio");
    group.bench_function("report", |b| {
        b.iter(|| {
            let algos = [
                ("none", Compression::None),
                ("lz4", Compression::Lz4),
                ("zstd", Compression::Zstd),
            ];
            let mut sizes = Vec::new();
            for (name, algo) in algos {
                let path = tmp.join(format!("bench_ratio_{name}.seg"));
                let mut store = StringStore::new().with_compression(algo);
                for &s in &mixed {
                    store.intern(s);
                }
                store.save(&path).unwrap();
                let size = std::fs::metadata(&path).unwrap().len();
                sizes.push((name, size));
                std::fs::remove_file(&path).ok();
            }
            sizes
        });
    });
    group.finish();

    // Print ratio table to stderr so it appears in `cargo bench` output.
    let algos = [
        ("none", Compression::None),
        ("lz4", Compression::Lz4),
        ("zstd", Compression::Zstd),
    ];
    let mut rows: Vec<(&str, u64, f64)> = Vec::new();
    let mut base_size = 0u64;
    for (name, algo) in algos {
        let path = tmp.join(format!("bench_ratio_final_{name}.seg"));
        let mut store = StringStore::new().with_compression(algo);
        for &s in &mixed {
            store.intern(s);
        }
        store.save(&path).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        if name == "none" {
            base_size = size;
        }
        rows.push((name, size, 0.0));
        std::fs::remove_file(&path).ok();
    }
    eprintln!("\n── Compression ratio ({} unique strings) ──", mixed.len());
    eprintln!("{:<8} {:>10}  {:>8}", "algo", "bytes", "ratio");
    for (name, size, _) in &rows {
        let ratio = if base_size > 0 {
            base_size as f64 / *size as f64
        } else {
            1.0
        };
        eprintln!("{:<8} {:>10}  {:>7.2}×", name, size, ratio);
    }
    eprintln!();
}

criterion_group!(
    benches,
    bench_intern,
    bench_resolve,
    bench_save_open,
    bench_compression_ratio,
);
criterion_main!(benches);
