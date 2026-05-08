use crate::cli::InputFormat;
use anyhow::{Context, Result};
use flate2::read::MultiGzDecoder;
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term, Triple};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Returns true for triple-shaped formats (no graph component in the input).
pub fn is_quad_format(f: InputFormat) -> bool {
    matches!(f, InputFormat::Trig | InputFormat::Nq)
}

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

/// Outcome of parsing one input file.
#[derive(Debug, Default, Clone)]
pub struct ParseOutcome {
    pub total: u64,
    pub skipped: u64,
    /// Number of triples/quads with at least one blank node (subject or object).
    pub bnode_count: u64,
    /// Prefix declarations encountered in the source, in iteration order.
    /// Empty for formats that do not carry prefixes (N-Triples, N-Quads, RDF/XML).
    pub prefixes: Vec<(String, String)>,
}

/// Stream-parse `reader` according to `format`. For each triple (after dropping
/// any graph context for quad formats), invoke `on_triple`. Triples involving a
/// blank node are skipped and increment the skipped counter.
///
/// Returns a [`ParseOutcome`] containing totals and any prefix declarations
/// seen in the source.
pub fn parse_triples<R: BufRead, F: FnMut(Triple) -> Result<()>>(
    reader: R,
    format: InputFormat,
    mut on_triple: F,
) -> Result<ParseOutcome> {
    let mut total: u64 = 0;
    let mut skipped: u64 = 0;
    let mut prefixes: Vec<(String, String)> = Vec::new();

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
            let mut parser = oxttl::TurtleParser::new().for_reader(reader);
            for tri in parser.by_ref() {
                let t = tri.context("Turtle parse error")?;
                handle_triple!(t);
            }
            prefixes.extend(
                parser
                    .prefixes()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            );
        }
        InputFormat::Rdf => {
            let parser = oxrdfxml::RdfXmlParser::new().for_reader(reader);
            for tri in parser {
                let t = tri.context("RDF/XML parse error")?;
                handle_triple!(t);
            }
        }
        InputFormat::Trig => {
            let mut parser = oxttl::TriGParser::new().for_reader(reader);
            for q in parser.by_ref() {
                let q = q.context("TriG parse error")?;
                handle_triple!(Triple::new(q.subject, q.predicate, q.object));
            }
            prefixes.extend(
                parser
                    .prefixes()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            );
        }
        InputFormat::Nq => {
            let parser = oxttl::NQuadsParser::new().for_reader(reader);
            for q in parser {
                let q = q.context("N-Quads parse error")?;
                handle_triple!(Triple::new(q.subject, q.predicate, q.object));
            }
        }
    }

    Ok(ParseOutcome {
        total,
        skipped,
        bnode_count: 0,
        prefixes,
    })
}

/// Stream-parse `reader` according to `format` and emit one [`Quad`] per
/// statement. For triple-shaped formats, the graph component is set to
/// [`GraphName::DefaultGraph`]; for quad-shaped formats, the input's named
/// graph is preserved. Blank nodes are NOT skipped — the caller is expected to
/// handle them via canonicalisation. Counts of statements and of those that
/// touch a blank node are returned in the outcome.
pub fn parse_quads<R: BufRead, F: FnMut(Quad) -> Result<()>>(
    reader: R,
    format: InputFormat,
    mut on_quad: F,
) -> Result<ParseOutcome> {
    let mut total: u64 = 0;
    let mut bnode_count: u64 = 0;
    let mut prefixes: Vec<(String, String)> = Vec::new();

    macro_rules! handle {
        ($q:expr) => {{
            let q: Quad = $q;
            total += 1;
            if matches!(q.subject, NamedOrBlankNode::BlankNode(_))
                || matches!(q.object, Term::BlankNode(_))
            {
                bnode_count += 1;
            }
            on_quad(q)?;
        }};
    }

    match format {
        InputFormat::Nt => {
            let parser = oxttl::NTriplesParser::new().for_reader(reader);
            for tri in parser {
                let t = tri.context("N-Triples parse error")?;
                handle!(Quad {
                    subject: t.subject,
                    predicate: t.predicate,
                    object: t.object,
                    graph_name: GraphName::DefaultGraph,
                });
            }
        }
        InputFormat::Ttl => {
            let mut parser = oxttl::TurtleParser::new().for_reader(reader);
            for tri in parser.by_ref() {
                let t = tri.context("Turtle parse error")?;
                handle!(Quad {
                    subject: t.subject,
                    predicate: t.predicate,
                    object: t.object,
                    graph_name: GraphName::DefaultGraph,
                });
            }
            prefixes.extend(
                parser
                    .prefixes()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            );
        }
        InputFormat::Rdf => {
            let parser = oxrdfxml::RdfXmlParser::new().for_reader(reader);
            for tri in parser {
                let t = tri.context("RDF/XML parse error")?;
                handle!(Quad {
                    subject: t.subject,
                    predicate: t.predicate,
                    object: t.object,
                    graph_name: GraphName::DefaultGraph,
                });
            }
        }
        InputFormat::Trig => {
            let mut parser = oxttl::TriGParser::new().for_reader(reader);
            for q in parser.by_ref() {
                let q = q.context("TriG parse error")?;
                handle!(q);
            }
            prefixes.extend(
                parser
                    .prefixes()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            );
        }
        InputFormat::Nq => {
            let parser = oxttl::NQuadsParser::new().for_reader(reader);
            for q in parser {
                let q = q.context("N-Quads parse error")?;
                handle!(q);
            }
        }
    }

    Ok(ParseOutcome {
        total,
        skipped: 0,
        bnode_count,
        prefixes,
    })
}

/// Convenience: produces a triple from a quad by dropping the graph component.
pub fn quad_to_triple(q: &Quad) -> Triple {
    Triple {
        subject: q.subject.clone(),
        predicate: q.predicate.clone(),
        object: q.object.clone(),
    }
}
