use anyhow::{Context, Result};
use oxrdf::NamedNode;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::path::Path;

/// Characters that are *not* safe in the trailing component of our URN graph IRIs.
/// We keep IRI-friendly chars and percent-encode everything else (incl. spaces,
/// non-ASCII, and reserved IRI delimiters).
const URN_NSS: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'&')
    .add(b'=');

const PREFIX: &str = "urn:rdf-compare:source:";

fn basename_stem(path: &Path) -> String {
    // Strip a single `.gz` first, then the final extension.
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed");
    let lower = name.to_ascii_lowercase();
    let trimmed = if lower.ends_with(".gz") {
        &name[..name.len() - 3]
    } else {
        name
    };
    match trimmed.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => stem.to_string(),
        _ => trimmed.to_string(),
    }
}

fn iri_for(stem: &str) -> Result<NamedNode> {
    let encoded: String = utf8_percent_encode(stem, URN_NSS).collect();
    let iri = format!("{}{}", PREFIX, encoded);
    NamedNode::new(&iri).with_context(|| format!("invalid generated graph IRI: {}", iri))
}

/// Resolve graph IRIs for both files, honoring optional CLI overrides and
/// disambiguating the auto-derived case when both basenames collide.
pub fn resolve_graph_iris(
    path_a: &Path,
    path_b: &Path,
    override_a: Option<&str>,
    override_b: Option<&str>,
) -> Result<(NamedNode, NamedNode)> {
    let a = match override_a {
        Some(s) => NamedNode::new(s).with_context(|| format!("invalid --graph-a IRI: {}", s))?,
        None => iri_for(&basename_stem(path_a))?,
    };
    let b = match override_b {
        Some(s) => NamedNode::new(s).with_context(|| format!("invalid --graph-b IRI: {}", s))?,
        None => iri_for(&basename_stem(path_b))?,
    };

    // Disambiguate auto-derived collisions.
    if override_a.is_none() && override_b.is_none() && a == b {
        let stem_a = basename_stem(path_a);
        let stem_b = basename_stem(path_b);
        let a2 = iri_for(&format!("{}:1", stem_a))?;
        let b2 = iri_for(&format!("{}:2", stem_b))?;
        return Ok((a2, b2));
    }
    Ok((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn derives_basic_iri() {
        let (a, b) = resolve_graph_iris(
            &PathBuf::from("foo.ttl"),
            &PathBuf::from("bar.nt"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(a.as_str(), "urn:rdf-compare:source:foo");
        assert_eq!(b.as_str(), "urn:rdf-compare:source:bar");
    }

    #[test]
    fn collision_gets_suffix() {
        let (a, b) = resolve_graph_iris(
            &PathBuf::from("dir1/data.ttl"),
            &PathBuf::from("dir2/data.ttl"),
            None,
            None,
        )
        .unwrap();
        assert_ne!(a, b);
        assert!(a.as_str().ends_with(":1"));
        assert!(b.as_str().ends_with(":2"));
    }

    #[test]
    fn override_used_verbatim() {
        let (a, b) = resolve_graph_iris(
            &PathBuf::from("a.ttl"),
            &PathBuf::from("b.ttl"),
            Some("https://example.com/A"),
            None,
        )
        .unwrap();
        assert_eq!(a.as_str(), "https://example.com/A");
        assert_eq!(b.as_str(), "urn:rdf-compare:source:b");
    }

    #[test]
    fn handles_gz_double_ext() {
        let (a, _) = resolve_graph_iris(
            &PathBuf::from("foo.ttl.gz"),
            &PathBuf::from("b.ttl"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(a.as_str(), "urn:rdf-compare:source:foo");
    }
}
