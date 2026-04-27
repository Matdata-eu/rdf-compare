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
        file_a: fixtures(a),
        file_b: fixtures(b),
        format_a: None,
        format_b: None,
        output: Some(out),
        output_format: fmt,
        graph_a: None,
        graph_b: None,
        quiet: true,
        ci: false,
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
    let stats =
        run_diff(&args("a.nt", "a.nt", tmp.clone(), OutputFormat::Nq)).unwrap();
    assert_eq!(stats.a_only, 0);
    assert_eq!(stats.b_only, 0);
}
