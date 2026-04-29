use crate::cli::{Args, InputFormat, OutputFormat};
use crate::graph_iri::resolve_graph_iris;
use crate::input::{open_reader, parse_triples};
use anyhow::{Context, Result};
use oxrdf::{GraphName, NamedNode, Quad, Triple};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write, stdout};
use std::path::Path;

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
        // NQuadsSerializer::finish takes self and returns the wrapped writer.
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
                s = s.with_prefix(name, iri).with_context(|| {
                    format!("invalid prefix IRI for `{name}`: <{iri}>")
                })?;
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
/// Order is preserved: A's declarations first (in their original order), then
/// B's new declarations (in their original order).
fn merge_prefixes(
    a: Vec<(String, String)>,
    b: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut seen: HashSet<String> = HashSet::with_capacity(a.len() + b.len());
    let mut out: Vec<(String, String)> = Vec::with_capacity(a.len() + b.len());
    for (name, iri) in a.into_iter().chain(b.into_iter()) {
        if seen.insert(name.clone()) {
            out.push((name, iri));
        }
    }
    out
}

pub fn run_diff(args: &Args) -> Result<DiffStats> {
    let fmt_a = detect_or_override(&args.file_a, args.format_a)?;
    let fmt_b = detect_or_override(&args.file_b, args.format_b)?;

    let (graph_a, graph_b) = resolve_graph_iris(
        &args.file_a,
        &args.file_b,
        args.graph_a.as_deref(),
        args.graph_b.as_deref(),
    )?;

    // Phase 1: load A.
    let mut set: HashSet<Triple> = HashSet::new();
    let reader_a = open_reader(&args.file_a)?;
    let outcome_a = parse_triples(reader_a, fmt_a, |t| {
        set.insert(t);
        Ok(())
    })
    .with_context(|| format!("while parsing {}", args.file_a.display()))?;

    // Phase 2: stream B; buffer b-only triples so we can emit them after the
    // serializer (which needs all prefix declarations up front) is built.
    let mut b_only_triples: Vec<Triple> = Vec::new();
    let reader_b = open_reader(&args.file_b)?;
    let outcome_b = parse_triples(reader_b, fmt_b, |t| {
        if !set.remove(&t) {
            b_only_triples.push(t);
        }
        Ok(())
    })
    .with_context(|| format!("while parsing {}", args.file_b.display()))?;

    // Build the output sink with merged prefixes (A wins on conflicts).
    let prefixes = merge_prefixes(outcome_a.prefixes, outcome_b.prefixes);
    let writer = open_writer(args.output.as_deref())?;
    let mut sink = make_sink(args.output_format, writer, &prefixes)?;

    // Phase 3a: emit b-only quads.
    let graph_b_name = GraphName::NamedNode(graph_b.clone());
    let mut b_only: u64 = 0;
    for t in b_only_triples {
        let q = make_quad(t, &graph_b_name);
        sink.write(&q)?;
        b_only += 1;
    }

    // Phase 3b: drain remaining A entries as a-only quads.
    let graph_a_name = GraphName::NamedNode(graph_a);
    let mut a_only: u64 = 0;
    for t in set.drain() {
        let q = make_quad(t, &graph_a_name);
        sink.write(&q)?;
        a_only += 1;
    }

    sink.finish()?;

    let a_total = outcome_a.total;
    let b_total = outcome_b.total;
    let common = a_total.saturating_sub(a_only);
    Ok(DiffStats {
        a_total,
        b_total,
        common,
        a_only,
        b_only,
        a_skipped_bnodes: outcome_a.skipped,
        b_skipped_bnodes: outcome_b.skipped,
    })
}

fn make_quad(t: Triple, g: &GraphName) -> Quad {
    Quad {
        subject: t.subject,
        predicate: t.predicate,
        object: t.object,
        graph_name: g.clone(),
    }
}

// Re-export for type sugar if needed elsewhere.
#[allow(dead_code)]
pub type GraphIri = NamedNode;
