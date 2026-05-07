use crate::cli::{InputFormat, OutputFormat};
use crate::graph_iri::resolve_graph_iris;
use crate::input::{open_reader, parse_triples};
use anyhow::{Context, Result, bail};
use oxrdf::{GraphName, NamedNode, NamedOrBlankNode, Quad, Triple};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write, stdout};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy)]
pub struct DiffStats {
    pub a_total: u64,
    pub b_total: u64,
    pub common: u64,
    pub a_only: u64,
    pub b_only: u64,
    pub a_skipped_bnodes: u64,
    pub b_skipped_bnodes: u64,
}

impl DiffStats {
    pub fn has_differences(&self) -> bool {
        self.a_only > 0 || self.b_only > 0
    }
}

/// Full result of a diff computation, kept in memory so it can be either
/// serialized to disk or served from the web viewer.
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub a_only: Vec<Triple>,
    pub b_only: Vec<Triple>,
    /// Merged prefix declarations from A and B. A wins on conflicts.
    pub prefixes: Vec<(String, String)>,
    pub graph_a: NamedNode,
    pub graph_b: NamedNode,
    pub stats: DiffStats,
    /// Source file paths, when known (used by the web viewer to lazily
    /// recompute the set of common triples).
    pub source_a: Option<PathBuf>,
    pub source_b: Option<PathBuf>,
    pub format_a: Option<InputFormat>,
    pub format_b: Option<InputFormat>,
}

fn triple_order(a: &Triple, b: &Triple) -> std::cmp::Ordering {
    let sa = match &a.subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str(),
        NamedOrBlankNode::BlankNode(bn) => bn.as_str(),
    };
    let sb = match &b.subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str(),
        NamedOrBlankNode::BlankNode(bn) => bn.as_str(),
    };
    sa.cmp(sb)
        .then_with(|| a.predicate.as_str().cmp(b.predicate.as_str()))
}

impl DiffResult {
    /// Sort `a_only` and `b_only` by (subject, predicate) so the web viewer
    /// receives rows in a deterministic order without client-side sorting.
    pub fn sort_rows(&mut self) {
        self.a_only.sort_unstable_by(triple_order);
        self.b_only.sort_unstable_by(triple_order);
    }
}

/// Inputs for [`compute_diff`].
#[derive(Debug, Clone)]
pub struct DiffInputs {
    pub file_a: PathBuf,
    pub file_b: PathBuf,
    pub format_a: Option<InputFormat>,
    pub format_b: Option<InputFormat>,
    pub graph_a: Option<String>,
    pub graph_b: Option<String>,
}

/// Inputs for [`load_diff_file`].
#[derive(Debug, Clone)]
pub struct LoadDiffInputs {
    pub diff: PathBuf,
    /// Override format (auto-detected when `None`). Only TriG and N-Quads
    /// carry the named-graph information needed to recover side membership.
    pub format: Option<InputFormat>,
    /// Optional explicit graph IRIs. If unset, they are inferred from the
    /// two distinct named graphs encountered in the diff file.
    pub graph_a: Option<String>,
    pub graph_b: Option<String>,
}

fn detect_or_override(path: &Path, over: Option<InputFormat>) -> Result<InputFormat> {
    match over {
        Some(f) => Ok(f),
        None => crate::cli::detect_format(path),
    }
}

fn open_writer(out: Option<&Path>) -> Result<Box<dyn Write>> {
    Ok(match out {
        Some(p) => Box::new(BufWriter::new(
            File::create(p).with_context(|| format!("failed to create {}", p.display()))?,
        )),
        None => Box::new(BufWriter::new(stdout().lock())),
    })
}

/// Trait-object-friendly quad sink so we can pick the serializer at runtime.
trait QuadSink {
    fn write(&mut self, quad: &Quad) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
}

struct TrigSink<W: Write> {
    inner: oxttl::trig::WriterTriGSerializer<W>,
}
impl<W: Write> QuadSink for TrigSink<W> {
    fn write(&mut self, quad: &Quad) -> Result<()> {
        self.inner
            .serialize_quad(quad)
            .context("failed to serialize quad to TriG")
    }
    fn finish(self: Box<Self>) -> Result<()> {
        self.inner
            .finish()
            .context("failed to finalize TriG output")?;
        Ok(())
    }
}

struct NqSink<W: Write> {
    inner: oxttl::nquads::WriterNQuadsSerializer<W>,
}
impl<W: Write> QuadSink for NqSink<W> {
    fn write(&mut self, quad: &Quad) -> Result<()> {
        self.inner
            .serialize_quad(quad)
            .context("failed to serialize quad to N-Quads")
    }
    fn finish(self: Box<Self>) -> Result<()> {
        let _ = self.inner.finish();
        Ok(())
    }
}

fn make_sink(
    format: OutputFormat,
    w: Box<dyn Write>,
    prefixes: &[(String, String)],
) -> Result<Box<dyn QuadSink>> {
    Ok(match format {
        OutputFormat::Trig => {
            let mut s = oxttl::TriGSerializer::new();
            for (name, iri) in prefixes {
                s = s
                    .with_prefix(name, iri)
                    .with_context(|| format!("invalid prefix IRI for `{name}`: <{iri}>"))?;
            }
            Box::new(TrigSink {
                inner: s.for_writer(w),
            })
        }
        OutputFormat::Nq => Box::new(NqSink {
            inner: oxttl::NQuadsSerializer::new().for_writer(w),
        }),
    })
}

/// Merge prefix lists from file A and file B. Prefixes from A are kept as-is;
/// any prefix name from B that is not already declared by A is appended.
fn merge_prefixes(a: Vec<(String, String)>, b: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen: HashSet<String> = HashSet::with_capacity(a.len() + b.len());
    let mut out: Vec<(String, String)> = Vec::with_capacity(a.len() + b.len());
    for (name, iri) in a.into_iter().chain(b) {
        if seen.insert(name.clone()) {
            out.push((name, iri));
        }
    }
    out
}

fn make_quad(t: Triple, g: &GraphName) -> Quad {
    Quad {
        subject: t.subject,
        predicate: t.predicate,
        object: t.object,
        graph_name: g.clone(),
    }
}

/// Compute the diff between two RDF files into a [`DiffResult`].
pub fn compute_diff(inputs: &DiffInputs) -> Result<DiffResult> {
    let fmt_a = detect_or_override(&inputs.file_a, inputs.format_a)?;
    let fmt_b = detect_or_override(&inputs.file_b, inputs.format_b)?;

    let (graph_a, graph_b) = resolve_graph_iris(
        &inputs.file_a,
        &inputs.file_b,
        inputs.graph_a.as_deref(),
        inputs.graph_b.as_deref(),
    )?;

    let mut set: HashSet<Triple> = HashSet::new();
    let reader_a = open_reader(&inputs.file_a)?;
    let outcome_a = parse_triples(reader_a, fmt_a, |t| {
        set.insert(t);
        Ok(())
    })
    .with_context(|| format!("while parsing {}", inputs.file_a.display()))?;

    let mut b_only_triples: Vec<Triple> = Vec::new();
    let reader_b = open_reader(&inputs.file_b)?;
    let outcome_b = parse_triples(reader_b, fmt_b, |t| {
        if !set.remove(&t) {
            b_only_triples.push(t);
        }
        Ok(())
    })
    .with_context(|| format!("while parsing {}", inputs.file_b.display()))?;

    let a_only_triples: Vec<Triple> = set.into_iter().collect();
    let prefixes = merge_prefixes(outcome_a.prefixes, outcome_b.prefixes);

    let a_total = outcome_a.total;
    let b_total = outcome_b.total;
    let a_only_count = a_only_triples.len() as u64;
    let b_only_count = b_only_triples.len() as u64;
    let common = a_total.saturating_sub(a_only_count);

    let stats = DiffStats {
        a_total,
        b_total,
        common,
        a_only: a_only_count,
        b_only: b_only_count,
        a_skipped_bnodes: outcome_a.skipped,
        b_skipped_bnodes: outcome_b.skipped,
    };

    Ok(DiffResult {
        a_only: a_only_triples,
        b_only: b_only_triples,
        prefixes,
        graph_a,
        graph_b,
        stats,
        source_a: Some(inputs.file_a.clone()),
        source_b: Some(inputs.file_b.clone()),
        format_a: Some(fmt_a),
        format_b: Some(fmt_b),
    })
}

/// Serialize a [`DiffResult`] to the given destination using the chosen format.
pub fn write_diff(result: &DiffResult, out: Option<&Path>, format: OutputFormat) -> Result<()> {
    let writer = open_writer(out)?;
    let mut sink = make_sink(format, writer, &result.prefixes)?;

    let graph_b_name = GraphName::NamedNode(result.graph_b.clone());
    for t in &result.b_only {
        sink.write(&make_quad(t.clone(), &graph_b_name))?;
    }
    let graph_a_name = GraphName::NamedNode(result.graph_a.clone());
    for t in &result.a_only {
        sink.write(&make_quad(t.clone(), &graph_a_name))?;
    }
    sink.finish()?;
    Ok(())
}

/// Backwards-compatible end-to-end runner: parse, compute diff, write output.
pub fn run_diff(args: &crate::cli::Args) -> Result<DiffStats> {
    let inputs = DiffInputs {
        file_a: args
            .file_a
            .clone()
            .ok_or_else(|| anyhow::anyhow!("<FILE_A> is required"))?,
        file_b: args
            .file_b
            .clone()
            .ok_or_else(|| anyhow::anyhow!("<FILE_B> is required"))?,

        format_a: args.format_a,
        format_b: args.format_b,
        graph_a: args.graph_a.clone(),
        graph_b: args.graph_b.clone(),
    };
    let result = compute_diff(&inputs)?;
    write_diff(&result, args.output.as_deref(), args.output_format)?;
    Ok(result.stats)
}

/// Stream-iterate the common triples of two RDF files. Memory cost ≈ |A|.
pub fn stream_common_triples<F: FnMut(&Triple) -> Result<()>>(
    file_a: &Path,
    file_b: &Path,
    format_a: Option<InputFormat>,
    format_b: Option<InputFormat>,
    mut on_triple: F,
) -> Result<()> {
    let fmt_a = detect_or_override(file_a, format_a)?;
    let fmt_b = detect_or_override(file_b, format_b)?;

    let mut set: HashSet<Triple> = HashSet::new();
    let reader_a = open_reader(file_a)?;
    parse_triples(reader_a, fmt_a, |t| {
        set.insert(t);
        Ok(())
    })
    .with_context(|| format!("while parsing {}", file_a.display()))?;

    let reader_b = open_reader(file_b)?;
    parse_triples(reader_b, fmt_b, |t| {
        if set.contains(&t) {
            on_triple(&t)?;
        }
        Ok(())
    })
    .with_context(|| format!("while parsing {}", file_b.display()))?;
    Ok(())
}

/// Load a previously-written diff file (TriG or N-Quads).
pub fn load_diff_file(inputs: &LoadDiffInputs) -> Result<DiffResult> {
    let fmt = detect_or_override(&inputs.diff, inputs.format)?;
    let mut prefixes: Vec<(String, String)> = Vec::new();

    let mut quads: Vec<Quad> = Vec::new();
    match fmt {
        InputFormat::Trig => {
            let reader = open_reader(&inputs.diff)?;
            let mut parser = oxttl::TriGParser::new().for_reader(reader);
            for q in parser.by_ref() {
                quads.push(q.context("TriG parse error")?);
            }
            prefixes.extend(
                parser
                    .prefixes()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            );
        }
        InputFormat::Nq => {
            let reader = open_reader(&inputs.diff)?;
            let parser = oxttl::NQuadsParser::new().for_reader(reader);
            for q in parser {
                quads.push(q.context("N-Quads parse error")?);
            }
        }
        other => bail!(
            "diff file format {:?} does not carry named graphs; use TriG or N-Quads",
            other
        ),
    }

    let (graph_a, graph_b) = match (inputs.graph_a.as_deref(), inputs.graph_b.as_deref()) {
        (Some(a), Some(b)) => (
            NamedNode::new(a).with_context(|| format!("invalid --graph-a IRI: {a}"))?,
            NamedNode::new(b).with_context(|| format!("invalid --graph-b IRI: {b}"))?,
        ),
        _ => {
            let mut seen: Vec<NamedNode> = Vec::new();
            for q in &quads {
                if let GraphName::NamedNode(n) = &q.graph_name
                    && !seen.iter().any(|s| s == n)
                {
                    seen.push(n.clone());
                    if seen.len() == 2 {
                        break;
                    }
                }
            }
            match seen.as_slice() {
                [a, b] => (a.clone(), b.clone()),
                [single] => (single.clone(), single.clone()),
                _ => bail!("diff file contains no named graphs; cannot determine A/B sides"),
            }
        }
    };

    let mut a_only: Vec<Triple> = Vec::new();
    let mut b_only: Vec<Triple> = Vec::new();
    let mut a_skipped: u64 = 0;
    let mut b_skipped: u64 = 0;
    for q in quads {
        let t = Triple::new(q.subject, q.predicate, q.object);
        let bnode = matches!(t.subject, oxrdf::NamedOrBlankNode::BlankNode(_))
            || matches!(t.object, oxrdf::Term::BlankNode(_));
        match &q.graph_name {
            GraphName::NamedNode(g) if g == &graph_a => {
                if bnode {
                    a_skipped += 1;
                } else {
                    a_only.push(t);
                }
            }
            GraphName::NamedNode(g) if g == &graph_b => {
                if bnode {
                    b_skipped += 1;
                } else {
                    b_only.push(t);
                }
            }
            _ => {}
        }
    }

    let stats = DiffStats {
        a_total: a_only.len() as u64,
        b_total: b_only.len() as u64,
        common: 0,
        a_only: a_only.len() as u64,
        b_only: b_only.len() as u64,
        a_skipped_bnodes: a_skipped,
        b_skipped_bnodes: b_skipped,
    };

    Ok(DiffResult {
        a_only,
        b_only,
        prefixes,
        graph_a,
        graph_b,
        stats,
        source_a: None,
        source_b: None,
        format_a: None,
        format_b: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OutputFormat;
    use std::path::PathBuf;

    fn fixtures(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests");
        p.push("fixtures");
        p.push(name);
        p
    }

    #[test]
    fn compute_then_load_round_trip_trig() {
        let inputs = DiffInputs {
            file_a: fixtures("a.ttl"),
            file_b: fixtures("b.ttl"),
            format_a: None,
            format_b: None,
            graph_a: None,
            graph_b: None,
        };
        let computed = compute_diff(&inputs).unwrap();

        let tmp = std::env::temp_dir().join("rdf-compare-roundtrip.trig");
        let _ = std::fs::remove_file(&tmp);
        write_diff(&computed, Some(&tmp), OutputFormat::Trig).unwrap();

        let loaded = load_diff_file(&LoadDiffInputs {
            diff: tmp,
            format: None,
            graph_a: Some(computed.graph_a.as_str().to_string()),
            graph_b: Some(computed.graph_b.as_str().to_string()),
        })
        .unwrap();

        let computed_a: HashSet<&Triple> = computed.a_only.iter().collect();
        let computed_b: HashSet<&Triple> = computed.b_only.iter().collect();
        let loaded_a: HashSet<&Triple> = loaded.a_only.iter().collect();
        let loaded_b: HashSet<&Triple> = loaded.b_only.iter().collect();
        assert_eq!(computed_a, loaded_a);
        assert_eq!(computed_b, loaded_b);
    }
}
