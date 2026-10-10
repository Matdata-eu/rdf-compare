use rdf_compare::cli::{Args, OutputFormat};
use rdf_compare::diff::run_diff;
use std::path::PathBuf;

fn fixtures(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push(name);
    p
}

fn args(a: &str, b: &str, out: PathBuf, fmt: OutputFormat) -> Args {
    Args {
        file_a: Some(fixtures(a)),
        file_b: Some(fixtures(b)),
        format_a: None,
        format_b: None,
        output: Some(out),
        output_format: fmt,
        graph_a: None,
        graph_b: None,
        stats: None,
        quiet: true,
        ci: false,
        view: false,
        no_open: false,
        bind: "127.0.0.1:0".to_string(),
        ignore_blank_nodes: false,
        normalize_literals: false,
        wkt_precision: None,
    }
}

#[test]
fn nt_vs_nt_basic_diff_nq_output() {
    let tmp = std::env::temp_dir().join("rdf-compare-nt-nt.nq");
    let _ = std::fs::remove_file(&tmp);
    let stats = run_diff(&args("a.nt", "b.nt", tmp.clone(), OutputFormat::Nq)).unwrap();

    // a.nt has 3 ground triples; b.nt has 4. Common: s1 and s2 (2). a-only: s3=vA. b-only: s3=vB and s4=v4.
    assert_eq!(stats.a_total, 3);
    assert_eq!(stats.b_total, 4);
    assert_eq!(stats.a_only, 1);
    assert_eq!(stats.b_only, 2);
    assert_eq!(stats.common, 2);
    assert_eq!(stats.a_skipped_bnodes, 0);
    assert_eq!(stats.b_skipped_bnodes, 0);

    let body = std::fs::read_to_string(&tmp).unwrap();
    assert!(body.contains("urn:rdf-compare:source:a"));
    assert!(body.contains("urn:rdf-compare:source:b"));
    assert!(body.contains("\"vA\""));
    assert!(body.contains("\"vB\""));
    assert!(body.contains("\"v4\""));
    // Common triples must NOT appear.
    assert!(!body.contains("\"v1\""));
    assert!(!body.contains("\"v2\""));
}

#[test]
fn turtle_vs_ntriples_cross_format() {
    let tmp = std::env::temp_dir().join("rdf-compare-ttl-nt.trig");
    let _ = std::fs::remove_file(&tmp);
    let stats = run_diff(&args("a.ttl", "b.nt", tmp.clone(), OutputFormat::Trig)).unwrap();
    assert_eq!(stats.a_total, 3);
    assert_eq!(stats.b_total, 4);
    assert_eq!(stats.a_only, 1);
    assert_eq!(stats.b_only, 2);
    let body = std::fs::read_to_string(&tmp).unwrap();
    assert!(body.contains("urn:rdf-compare:source:a"));
    assert!(body.contains("urn:rdf-compare:source:b"));
}

#[test]
fn identical_inputs_produce_no_diff() {
    let tmp = std::env::temp_dir().join("rdf-compare-identical.nq");
    let _ = std::fs::remove_file(&tmp);
    let mut a = args("a.nt", "a.nt", tmp.clone(), OutputFormat::Nq);
    // Avoid graph IRI collision — provide explicit overrides.
    a.graph_a = Some("urn:test:left".to_string());
    a.graph_b = Some("urn:test:right".to_string());
    let stats = run_diff(&a).unwrap();
    assert_eq!(stats.a_only, 0);
    assert_eq!(stats.b_only, 0);
    assert_eq!(stats.common, 3);
    assert!(!stats.has_differences());
    let body = std::fs::read_to_string(&tmp).unwrap();
    assert!(body.trim().is_empty());
}

#[test]
fn collision_basenames_get_disambiguated() {
    // a.nt vs a.nt with auto-derived graph IRIs → suffixed :1 / :2
    let tmp = std::env::temp_dir().join("rdf-compare-collision.nq");
    let _ = std::fs::remove_file(&tmp);
    let stats = run_diff(&args("a.nt", "a.nt", tmp.clone(), OutputFormat::Nq)).unwrap();
    assert_eq!(stats.a_only, 0);
    assert_eq!(stats.b_only, 0);
}

#[test]
fn trig_output_preserves_prefixes_from_inputs() {
    // a.ttl declares `ex:`; b.ttl declares `ex:` and a new `foo:`.
    // Output is TriG, so prefix declarations from A must be kept and any
    // new prefix from B (here `foo:`) must be appended without overriding A.
    let tmp = std::env::temp_dir().join("rdf-compare-prefixes.trig");
    let _ = std::fs::remove_file(&tmp);
    let stats = run_diff(&args("a.ttl", "b.ttl", tmp.clone(), OutputFormat::Trig)).unwrap();
    assert!(stats.has_differences());

    let body = std::fs::read_to_string(&tmp).unwrap();

    // Prefixes from A and the new one from B must appear as declarations.
    assert!(
        body.contains("@prefix ex: <http://example.org/>"),
        "missing ex: prefix declaration in:\n{body}"
    );
    assert!(
        body.contains("@prefix foo: <http://foo.example/>"),
        "missing foo: prefix declaration in:\n{body}"
    );

    // The serializer should actually use those prefixes for terms it can shorten,
    // rather than emitting the full IRIs.
    assert!(
        body.contains("ex:s3") || body.contains("ex:s4") || body.contains("ex:s5"),
        "expected ex:-prefixed term in output:\n{body}"
    );
    assert!(
        body.contains("foo:vC"),
        "expected foo:-prefixed term in output:\n{body}"
    );
}

#[test]
fn first_file_prefix_wins_over_second() {
    // Both files declare `ex:` but with different IRIs. The first file's
    // declaration must win — the output's `ex:` must point at A's IRI.
    let dir = std::env::temp_dir().join("rdf-compare-prefix-precedence");
    let _ = std::fs::create_dir_all(&dir);
    let a_path = dir.join("a.ttl");
    let b_path = dir.join("b.ttl");
    let out = dir.join("out.trig");
    let _ = std::fs::remove_file(&out);

    std::fs::write(
        &a_path,
        "@prefix ex: <http://a.example/> .\n\
         <http://a.example/s> <http://a.example/p> \"vA\" .\n",
    )
    .unwrap();
    std::fs::write(
        &b_path,
        "@prefix ex: <http://b.example/> .\n\
         <http://b.example/s> <http://b.example/p> \"vB\" .\n",
    )
    .unwrap();

    let mut a = Args {
        file_a: Some(a_path),
        file_b: Some(b_path),
        format_a: None,
        format_b: None,
        output: Some(out.clone()),
        output_format: OutputFormat::Trig,
        graph_a: None,
        graph_b: None,
        stats: None,
        quiet: true,
        ci: false,
        view: false,
        no_open: false,
        bind: "127.0.0.1:0".to_string(),
        ignore_blank_nodes: false,
        normalize_literals: false,
        wkt_precision: None,
    };
    a.graph_a = Some("urn:test:left".to_string());
    a.graph_b = Some("urn:test:right".to_string());

    run_diff(&a).unwrap();
    let body = std::fs::read_to_string(&out).unwrap();

    assert!(
        body.contains("@prefix ex: <http://a.example/>"),
        "A's ex: prefix should win, got:\n{body}"
    );
    assert!(
        !body.contains("@prefix ex: <http://b.example/>"),
        "B's ex: prefix must not override A's, got:\n{body}"
    );
}

#[test]
fn isomorphic_blank_nodes_diff_to_zero() {
    // Same shape, different bnode labels. RDFC-1.0 must canonicalise both
    // sides to the same labels so the set-diff is empty.
    let tmp = std::env::temp_dir().join("rdf-compare-bnode-iso.trig");
    let _ = std::fs::remove_file(&tmp);
    let stats = run_diff(&args(
        "bnode-iso-a.ttl",
        "bnode-iso-b.ttl",
        tmp.clone(),
        OutputFormat::Trig,
    ))
    .unwrap();
    assert_eq!(stats.a_only, 0, "stats: {stats:?}");
    assert_eq!(stats.b_only, 0, "stats: {stats:?}");
    assert!(!stats.has_differences());
    // Default mode does NOT skip bnodes anymore.
    assert_eq!(stats.a_skipped_bnodes, 0);
    assert_eq!(stats.b_skipped_bnodes, 0);
}

#[test]
fn structurally_different_blank_nodes_diff() {
    let tmp = std::env::temp_dir().join("rdf-compare-bnode-diff.trig");
    let _ = std::fs::remove_file(&tmp);
    let stats = run_diff(&args(
        "bnode-diff-a.ttl",
        "bnode-diff-b.ttl",
        tmp.clone(),
        OutputFormat::Trig,
    ))
    .unwrap();
    // The `ex:knows _:bnode` edge canonicalises identically on both sides
    // (same shape) and cancels. The `ex:name` literal differs, leaving one
    // unique edge per side.
    assert_eq!(stats.a_only, 1, "stats: {stats:?}");
    assert_eq!(stats.b_only, 1, "stats: {stats:?}");
    let body = std::fs::read_to_string(&tmp).unwrap();
    assert!(body.contains("\"Bob\""));
    assert!(body.contains("\"Carol\""));
}

#[test]
fn ignore_blank_nodes_falls_back_to_skipping() {
    let tmp = std::env::temp_dir().join("rdf-compare-bnode-skip.trig");
    let _ = std::fs::remove_file(&tmp);
    let mut a = args(
        "bnode-iso-a.ttl",
        "bnode-iso-b.ttl",
        tmp.clone(),
        OutputFormat::Trig,
    );
    a.ignore_blank_nodes = true;
    let stats = run_diff(&a).unwrap();
    // Both bnode-bearing statements on each side are skipped.
    assert_eq!(stats.a_total, 0);
    assert_eq!(stats.b_total, 0);
    assert_eq!(stats.a_skipped_bnodes, 2);
    assert_eq!(stats.b_skipped_bnodes, 2);
    assert_eq!(stats.a_only, 0);
    assert_eq!(stats.b_only, 0);
}

#[test]
fn nq_inputs_emit_dual_output_files() {
    let dir = std::env::temp_dir().join("rdf-compare-nq-dual");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("out.nq");
    let path_a = dir.join("out-a.nq");
    let path_b = dir.join("out-b.nq");
    let _ = std::fs::remove_file(&path_a);
    let _ = std::fs::remove_file(&path_b);
    let _ = std::fs::remove_file(&out);

    let stats = run_diff(&args(
        "quads-a.nq",
        "quads-b.nq",
        out.clone(),
        OutputFormat::Nq,
    ))
    .unwrap();
    assert_eq!(stats.a_only, 1, "stats: {stats:?}");
    assert_eq!(stats.b_only, 2, "stats: {stats:?}");

    // The merged single output file must NOT exist; instead, two per-side
    // files preserving the original named graphs must be written.
    assert!(
        !out.exists(),
        "single-file output must not exist for quad inputs"
    );
    assert!(path_a.exists(), "missing per-side A file at {path_a:?}");
    assert!(path_b.exists(), "missing per-side B file at {path_b:?}");

    let body_a = std::fs::read_to_string(&path_a).unwrap();
    let body_b = std::fs::read_to_string(&path_b).unwrap();
    // Original graph names must be preserved (no wrapper graph IRIs).
    assert!(body_a.contains("<http://example.org/g2>"));
    assert!(body_b.contains("<http://example.org/g2>"));
    assert!(!body_a.contains("urn:rdf-compare:source:"));
    assert!(!body_b.contains("urn:rdf-compare:source:"));
    // A-only payload: "vA"; B-only payload: "vB" and "v4".
    assert!(body_a.contains("\"vA\""));
    assert!(body_b.contains("\"vB\""));
    assert!(body_b.contains("\"v4\""));
}

#[test]
fn stats_json_reports_predicates_classes_and_subjects() {
    let dir = std::env::temp_dir().join("rdf-compare-stats");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("out.trig");
    let stats_path = dir.join("stats.json");
    let _ = std::fs::remove_file(&stats_path);

    let mut a = args("summary-a.ttl", "summary-b.ttl", out, OutputFormat::Trig);
    a.stats = Some(stats_path.clone());
    run_diff(&a).unwrap();

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&stats_path).unwrap()).unwrap();

    // ex:alice age 30→31, ex:carol is new (type + name), ex:gone is dropped.
    assert_eq!(v["totals"]["added"], 3);
    assert_eq!(v["totals"]["removed"], 2);
    assert_eq!(v["totals"]["common"], 7);

    assert_eq!(v["subjects"]["affected"], 3);
    assert_eq!(v["subjects"]["added"], 1);
    assert_eq!(v["subjects"]["removed"], 1);
    assert_eq!(v["subjects"]["modified"], 1);
    assert_eq!(v["subjects"]["a_total"], 5);
    assert_eq!(v["subjects"]["b_total"], 5);

    let preds = v["predicates"].as_array().unwrap();
    let pred = |iri: &str| {
        preds
            .iter()
            .find(|p| p["predicate"] == iri)
            .unwrap_or_else(|| panic!("missing predicate {iri} in {preds:?}"))
    };
    let age = pred("http://example.org/age");
    assert_eq!(age["added"], 1);
    assert_eq!(age["removed"], 1);
    assert_eq!(age["common"], 0);
    let name = pred("http://example.org/name");
    assert_eq!(name["added"], 1);
    assert_eq!(name["removed"], 0);
    assert_eq!(name["a_total"], 3);
    assert_eq!(name["b_total"], 4);
    assert_eq!(name["common"], 3);
    // Unchanged predicates are not listed.
    assert_eq!(preds.len(), 4);

    let classes = v["classes"].as_array().unwrap();
    let person = classes
        .iter()
        .find(|c| c["class"] == "http://example.org/Person")
        .unwrap();
    assert_eq!(person["instances_added"], 1);
    assert_eq!(person["instances_removed"], 0);
    assert_eq!(person["subjects_affected"], 2);
    assert_eq!(person["triples_added"], 3);
    assert_eq!(person["triples_removed"], 1);
    let untyped = classes.iter().find(|c| c["class"].is_null()).unwrap();
    assert_eq!(untyped["subjects_affected"], 1);
    assert_eq!(untyped["triples_removed"], 1);
    assert!(
        !classes
            .iter()
            .any(|c| c["class"] == "http://example.org/Company"),
        "unchanged classes must not be listed"
    );

    let top = v["top_subjects"].as_array().unwrap();
    assert_eq!(top.len(), 3);
    assert_eq!(top[0]["status"], "modified");

    // ex:alice age 30→31 is one changed value; ex:carol's name is not.
    assert_eq!(v["totals"]["changed"], 1);
    assert_eq!(age["changed"], 1);
    assert_eq!(name["changed"], 0);
}

fn diff_body(a: &Args) -> (rdf_compare::diff::DiffStats, String) {
    let stats = run_diff(a).unwrap();
    let body = std::fs::read_to_string(a.output.as_ref().unwrap()).unwrap();
    (stats, body)
}

#[test]
fn normalize_literals_cancels_equivalent_values() {
    let out = std::env::temp_dir().join("rdf-compare-norm-off.nq");
    let a = args("norm-a.ttl", "norm-b.ttl", out, OutputFormat::Nq);
    let (stats, _) = diff_body(&a);
    // Without normalisation every literal differs except the language tag
    // (the Turtle parser already lower-cases it).
    assert_eq!(stats.a_only, 7);
    assert_eq!(stats.b_only, 7);

    let out = std::env::temp_dir().join("rdf-compare-norm-on.nq");
    let mut a = args("norm-a.ttl", "norm-b.ttl", out, OutputFormat::Nq);
    a.normalize_literals = true;
    let (stats, body) = diff_body(&a);
    // Left: the coarse WKT point and the real name change.
    assert_eq!(stats.a_only, 2, "{body}");
    assert_eq!(stats.b_only, 2, "{body}");
    assert_eq!(stats.changed, 2);
    assert!(body.contains("\"Old name\""));
    assert!(body.contains("POINT(4.3517103 50.8503396)"));

    let out = std::env::temp_dir().join("rdf-compare-norm-wkt.nq");
    let mut a = args("norm-a.ttl", "norm-b.ttl", out, OutputFormat::Nq);
    a.normalize_literals = true;
    a.wkt_precision = Some(4);
    let (stats, body) = diff_body(&a);
    assert_eq!(stats.a_only, 1, "{body}");
    assert_eq!(stats.b_only, 1, "{body}");
}
