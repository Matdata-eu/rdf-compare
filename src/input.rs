use crate::cli::InputFormat;
use anyhow::{Context, Result};
use flate2::read::MultiGzDecoder;
use oxrdf::{NamedOrBlankNode, Term, Triple};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Open `path` for reading, transparently decompressing if it ends with `.gz`.
pub fn open_reader(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let is_gz = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase().ends_with(".gz"))
        .unwrap_or(false);

    let raw: Box<dyn Read> = if is_gz {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };
    Ok(Box::new(BufReader::new(raw)))
}

/// Returns true if subject or object is a blank node.
fn has_blank_node(t: &Triple) -> bool {
    matches!(t.subject, NamedOrBlankNode::BlankNode(_)) || matches!(t.object, Term::BlankNode(_))
}

/// Stream-parse `reader` according to `format`. For each triple (after dropping
/// any graph context for quad formats), invoke `on_triple`. Triples involving a
/// blank node are skipped and increment `skipped`.
///
/// Returns `(total_triples_seen, skipped_blank_node_count)` where
/// `total_triples_seen` counts every parsed (non-blank-node) triple delivered
/// to the callback.
pub fn parse_triples<R: BufRead, F: FnMut(Triple) -> Result<()>>(
    reader: R,
    format: InputFormat,
    mut on_triple: F,
) -> Result<(u64, u64)> {
    let mut total: u64 = 0;
    let mut skipped: u64 = 0;

    macro_rules! handle_triple {
        ($t:expr) => {{
            let t: Triple = $t;
            if has_blank_node(&t) {
                skipped += 1;
            } else {
                total += 1;
                on_triple(t)?;
            }
        }};
    }

    match format {
        InputFormat::Nt => {
            let parser = oxttl::NTriplesParser::new().for_reader(reader);
            for tri in parser {
                let t = tri.context("N-Triples parse error")?;
                handle_triple!(t);
            }
        }
        InputFormat::Ttl => {
            let parser = oxttl::TurtleParser::new().for_reader(reader);
            for tri in parser {
                let t = tri.context("Turtle parse error")?;
                handle_triple!(t);
            }
        }
        InputFormat::Rdf => {
            let parser = oxrdfxml::RdfXmlParser::new().for_reader(reader);
            for tri in parser {
                let t = tri.context("RDF/XML parse error")?;
                handle_triple!(t);
            }
        }
        InputFormat::Trig => {
            let parser = oxttl::TriGParser::new().for_reader(reader);
            for q in parser {
                let q = q.context("TriG parse error")?;
                handle_triple!(Triple::new(q.subject, q.predicate, q.object));
            }
        }
        InputFormat::Nq => {
            let parser = oxttl::NQuadsParser::new().for_reader(reader);
            for q in parser {
                let q = q.context("N-Quads parse error")?;
                handle_triple!(Triple::new(q.subject, q.predicate, q.object));
            }
        }
    }

    Ok((total, skipped))
}
