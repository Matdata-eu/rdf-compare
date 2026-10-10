use crate::cli::{InputFormat, OutputFormat};
use crate::graph_iri::resolve_graph_iris;
use crate::input::{is_quad_format, open_reader, parse_quads, parse_triples, quad_to_triple};
use crate::normalize::Normalization;
use crate::stats::{DiffSummary, SummaryBuilder};
use anyhow::{Context, Result, bail};
use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::{Dataset, GraphName, NamedNode, NamedOrBlankNode, Quad, Term, Triple};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
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
    /// Removed/added pairs that share subject and predicate (see
    /// [`find_changes`]). Each pair is also counted in `a_only` and `b_only`.
    pub changed: u64,
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
///
/// `a_only` / `b_only` carry [`Quad`]s. In **triple mode** (`quad_mode = false`)
/// each quad's graph component is the wrapper graph IRI (`graph_a` / `graph_b`)
/// and the diff is written as a single TriG/N-Quads document containing two
/// named graphs. In **quad mode** (at least one input is N-Quads or TriG)
/// the original graph names from the source are preserved and the diff is
/// emitted as two separate files (one per side).
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub a_only: Vec<Quad>,
    pub b_only: Vec<Quad>,
    /// Changed values: `(index into a_only, index into b_only)` pairs, as
    /// found by [`find_changes`]. Recomputed by [`DiffResult::sort_rows`].
    pub changes: Vec<(usize, usize)>,
    /// Literal normalisation applied to both inputs, reused when the viewer
    /// streams the common triples.
    pub normalization: Normalization,
    /// Merged prefix declarations from A and B. A wins on conflicts.
    pub prefixes: Vec<(String, String)>,
    pub graph_a: NamedNode,
    pub graph_b: NamedNode,
    pub stats: DiffStats,
    /// Higher-level statistics (per predicate, per class, subjects).
    pub summary: DiffSummary,
    /// Source file paths, when known (used by the web viewer to lazily
    /// recompute the set of common triples).
    pub source_a: Option<PathBuf>,
    pub source_b: Option<PathBuf>,
    pub format_a: Option<InputFormat>,
    pub format_b: Option<InputFormat>,
    pub quad_mode: bool,
}

fn graph_name_str(g: &GraphName) -> &str {
    match g {
        GraphName::NamedNode(n) => n.as_str(),
        GraphName::BlankNode(b) => b.as_str(),
        GraphName::DefaultGraph => "",
    }
}

fn quad_order(a: &Quad, b: &Quad) -> std::cmp::Ordering {
    let sa = match &a.subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str(),
        NamedOrBlankNode::BlankNode(bn) => bn.as_str(),
    };
    let sb = match &b.subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str(),
        NamedOrBlankNode::BlankNode(bn) => bn.as_str(),
    };
    graph_name_str(&a.graph_name)
        .cmp(graph_name_str(&b.graph_name))
        .then_with(|| sa.cmp(sb))
        .then_with(|| a.predicate.as_str().cmp(b.predicate.as_str()))
}

impl DiffResult {
    pub fn sort_rows(&mut self) {
        self.a_only.sort_unstable_by(quad_order);
        self.b_only.sort_unstable_by(quad_order);
        self.changes = find_changes(&self.a_only, &self.b_only, self.quad_mode);
    }

    pub fn a_only_triples(&self) -> impl Iterator<Item = Triple> + '_ {
        self.a_only.iter().map(quad_to_triple)
    }

    pub fn b_only_triples(&self) -> impl Iterator<Item = Triple> + '_ {
        self.b_only.iter().map(quad_to_triple)
    }
}

#[derive(Debug, Clone)]
pub struct DiffInputs {
    pub file_a: PathBuf,
    pub file_b: PathBuf,
    pub format_a: Option<InputFormat>,
    pub format_b: Option<InputFormat>,
    pub graph_a: Option<String>,
    pub graph_b: Option<String>,
    /// When true, blank-node-bearing statements are skipped instead of
    /// canonicalised via RDFC-1.0.
    pub ignore_blank_nodes: bool,
    pub normalization: Normalization,
}

#[derive(Debug, Clone)]
pub struct LoadDiffInputs {
    pub diff: PathBuf,
    pub format: Option<InputFormat>,
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

/// Canonicalise `quads` in place using RDFC-1.0 (oxrdf, `rdfc-10` feature).
/// The blank-node identifiers in the returned vector are stable canonical
/// labels (`_:c14n…`) determined by the graph's structure.
fn canonicalize_quads(quads: Vec<Quad>) -> Vec<Quad> {
    let mut dataset: Dataset = quads.into_iter().collect();
    dataset.canonicalize(CanonicalizationAlgorithm::Unstable);
    dataset.iter().map(Quad::from).collect()
}

/// Pair removed and added statements that describe a new value for the same
/// property: same subject and predicate (and named graph in quad mode), and
/// for language-tagged objects the same language. A pair is only formed when
/// exactly one statement was removed and one added for that key, so
/// multi-valued properties with several changes stay as separate rows.
///
/// Returns `(index into a_only, index into b_only)` pairs, ordered by the
/// `a_only` index.
pub fn find_changes(a_only: &[Quad], b_only: &[Quad], quad_mode: bool) -> Vec<(usize, usize)> {
    type Key<'a> = (
        Option<&'a GraphName>,
        &'a NamedOrBlankNode,
        &'a NamedNode,
        Option<&'a str>,
    );
    fn key(q: &Quad, quad_mode: bool) -> Key<'_> {
        let lang = match &q.object {
            Term::Literal(l) => l.language(),
            _ => None,
        };
        (
            quad_mode.then_some(&q.graph_name),
            &q.subject,
            &q.predicate,
            lang,
        )
    }
    // (count, last index) per side.
    let mut groups: HashMap<Key<'_>, [(u32, usize); 2]> = HashMap::new();
    for (side, quads) in [a_only, b_only].into_iter().enumerate() {
        for (i, q) in quads.iter().enumerate() {
            let e = &mut groups.entry(key(q, quad_mode)).or_default()[side];
            e.0 += 1;
            e.1 = i;
        }
    }
    let mut pairs: Vec<(usize, usize)> = groups
        .into_values()
        .filter(|[a, b]| a.0 == 1 && b.0 == 1)
        .map(|[a, b]| (a.1, b.1))
        .collect();
    pairs.sort_unstable();
    pairs
}

pub fn compute_diff(inputs: &DiffInputs) -> Result<DiffResult> {
    let fmt_a = detect_or_override(&inputs.file_a, inputs.format_a)?;
    let fmt_b = detect_or_override(&inputs.file_b, inputs.format_b)?;

    let (graph_a, graph_b) = resolve_graph_iris(
        &inputs.file_a,
        &inputs.file_b,
        inputs.graph_a.as_deref(),
        inputs.graph_b.as_deref(),
    )?;
    let quad_mode = is_quad_format(fmt_a) || is_quad_format(fmt_b);

    if inputs.ignore_blank_nodes {
        return compute_diff_skip_bnodes(inputs, fmt_a, fmt_b, &graph_a, &graph_b, quad_mode);
    }

    let mut quads_a: Vec<Quad> = Vec::new();
    let reader_a = open_reader(&inputs.file_a)?;
    let norm = inputs.normalization;
    let outcome_a = parse_quads(reader_a, fmt_a, |q| {
        quads_a.push(norm.quad(q));
        Ok(())
    })
    .with_context(|| format!("while parsing {}", inputs.file_a.display()))?;

    let mut quads_b: Vec<Quad> = Vec::new();
    let reader_b = open_reader(&inputs.file_b)?;
    let outcome_b = parse_quads(reader_b, fmt_b, |q| {
        quads_b.push(norm.quad(q));
        Ok(())
    })
    .with_context(|| format!("while parsing {}", inputs.file_b.display()))?;

    if outcome_a.bnode_count > 0 || outcome_b.bnode_count > 0 {
        // Per W3C RDFC-1.0: canonicalise each side independently, then
        // perform a syntactic set-diff on the resulting quads. Identical
        // sub-graphs receive identical canonical bnode labels and therefore
        // compare equal across sides.
        quads_a = canonicalize_quads(quads_a);
        quads_b = canonicalize_quads(quads_b);
    }

    let mut summary = SummaryBuilder::new();
    for q in &quads_a {
        summary.observe_a(&q.subject, &q.predicate, &q.object);
    }
    for q in &quads_b {
        summary.observe_b(&q.subject, &q.predicate, &q.object);
    }

    let mut set: HashSet<Quad> = quads_a.into_iter().collect();
    let mut b_only: Vec<Quad> = Vec::new();
    for q in quads_b {
        if !set.remove(&q) {
            b_only.push(q);
        }
    }
    let mut a_only: Vec<Quad> = set.into_iter().collect();

    // In triple mode, statements parsed from triple inputs all carry
    // `DefaultGraph`. Tag survivors with the per-side wrapper graph IRI
    // *after* the set-diff so equal triples on both sides cancel.
    if !quad_mode {
        let g_a = GraphName::NamedNode(graph_a.clone());
        let g_b = GraphName::NamedNode(graph_b.clone());
        for q in &mut a_only {
            if matches!(q.graph_name, GraphName::DefaultGraph) {
                q.graph_name = g_a.clone();
            }
        }
        for q in &mut b_only {
            if matches!(q.graph_name, GraphName::DefaultGraph) {
                q.graph_name = g_b.clone();
            }
        }
    }

    let prefixes = merge_prefixes(outcome_a.prefixes, outcome_b.prefixes);
    let a_total = outcome_a.total;
    let b_total = outcome_b.total;
    let a_only_count = a_only.len() as u64;
    let b_only_count = b_only.len() as u64;
    let common = a_total.saturating_sub(a_only_count);
    let changes = find_changes(&a_only, &b_only, quad_mode);

    let stats = DiffStats {
        a_total,
        b_total,
        common,
        a_only: a_only_count,
        b_only: b_only_count,
        changed: changes.len() as u64,
        a_skipped_bnodes: 0,
        b_skipped_bnodes: 0,
    };
    let summary = summary.finish(&stats, &a_only, &b_only, &changes);

    Ok(DiffResult {
        a_only,
        b_only,
        changes,
        normalization: norm,
        prefixes,
        graph_a,
        graph_b,
        stats,
        summary,
        source_a: Some(inputs.file_a.clone()),
        source_b: Some(inputs.file_b.clone()),
        format_a: Some(fmt_a),
        format_b: Some(fmt_b),
        quad_mode,
    })
}

fn compute_diff_skip_bnodes(
    inputs: &DiffInputs,
    fmt_a: InputFormat,
    fmt_b: InputFormat,
    graph_a: &NamedNode,
    graph_b: &NamedNode,
    quad_mode: bool,
) -> Result<DiffResult> {
    let mut summary = SummaryBuilder::new();
    let mut set: HashSet<Triple> = HashSet::new();
    let norm = inputs.normalization;
    let reader_a = open_reader(&inputs.file_a)?;
    let outcome_a = parse_triples(reader_a, fmt_a, |t| {
        let t = norm.triple(t);
        summary.observe_a(&t.subject, &t.predicate, &t.object);
        set.insert(t);
        Ok(())
    })
    .with_context(|| format!("while parsing {}", inputs.file_a.display()))?;

    let mut b_only_triples: Vec<Triple> = Vec::new();
    let reader_b = open_reader(&inputs.file_b)?;
    let outcome_b = parse_triples(reader_b, fmt_b, |t| {
        let t = norm.triple(t);
        summary.observe_b(&t.subject, &t.predicate, &t.object);
        if !set.remove(&t) {
            b_only_triples.push(t);
        }
        Ok(())
    })
    .with_context(|| format!("while parsing {}", inputs.file_b.display()))?;

    let a_only_triples: Vec<Triple> = set.into_iter().collect();
    let prefixes = merge_prefixes(outcome_a.prefixes, outcome_b.prefixes);

    let a_only_count = a_only_triples.len() as u64;
    let b_only_count = b_only_triples.len() as u64;
    let common = outcome_a.total.saturating_sub(a_only_count);

    let g_a = GraphName::NamedNode(graph_a.clone());
    let g_b = GraphName::NamedNode(graph_b.clone());
    let a_only: Vec<Quad> = a_only_triples
        .into_iter()
        .map(|t| make_quad(t, &g_a))
        .collect();
    let b_only: Vec<Quad> = b_only_triples
        .into_iter()
        .map(|t| make_quad(t, &g_b))
        .collect();
    let changes = find_changes(&a_only, &b_only, quad_mode);

    let stats = DiffStats {
        a_total: outcome_a.total,
        b_total: outcome_b.total,
        common,
        a_only: a_only_count,
        b_only: b_only_count,
        changed: changes.len() as u64,
        a_skipped_bnodes: outcome_a.skipped,
        b_skipped_bnodes: outcome_b.skipped,
    };
    let summary = summary.finish(&stats, &a_only, &b_only, &changes);

    Ok(DiffResult {
        a_only,
        b_only,
        changes,
        normalization: norm,
        prefixes,
        graph_a: graph_a.clone(),
        graph_b: graph_b.clone(),
        stats,
        summary,
        source_a: Some(inputs.file_a.clone()),
        source_b: Some(inputs.file_b.clone()),
        format_a: Some(fmt_a),
        format_b: Some(fmt_b),
        quad_mode,
    })
}

/// Derive per-side output paths for a quad-mode diff. Given `out =
/// "diff.trig"`, returns `("diff-a.trig", "diff-b.trig")`.
fn split_output_paths(out: &Path) -> (PathBuf, PathBuf) {
    let stem = out
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("diff")
        .to_string();
    let ext = out
        .extension()
        .and_then(OsStr::to_str)
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let parent = out.parent();
    let make = |suffix: &str| {
        let name = format!("{stem}-{suffix}{ext}");
        match parent {
            Some(p) if !p.as_os_str().is_empty() => p.join(name),
            _ => PathBuf::from(name),
        }
    };
    (make("a"), make("b"))
}

pub fn write_diff(result: &DiffResult, out: Option<&Path>, format: OutputFormat) -> Result<()> {
    if result.quad_mode {
        let Some(out_path) = out else {
            bail!(
                "quad-shaped inputs (N-Quads/TriG) require --output: two files \
                 are written (one per side) to preserve the source named graphs"
            );
        };
        let (path_a, path_b) = split_output_paths(out_path);

        let writer_a = open_writer(Some(&path_a))?;
        let mut sink_a = make_sink(format, writer_a, &result.prefixes)?;
        for q in &result.a_only {
            sink_a.write(q)?;
        }
        sink_a.finish()?;

        let writer_b = open_writer(Some(&path_b))?;
        let mut sink_b = make_sink(format, writer_b, &result.prefixes)?;
        for q in &result.b_only {
            sink_b.write(q)?;
        }
        sink_b.finish()?;
        return Ok(());
    }

    let writer = open_writer(out)?;
    let mut sink = make_sink(format, writer, &result.prefixes)?;
    for q in &result.b_only {
        sink.write(q)?;
    }
    for q in &result.a_only {
        sink.write(q)?;
    }
    sink.finish()?;
    Ok(())
}

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
        ignore_blank_nodes: args.ignore_blank_nodes,
        normalization: args.normalization(),
    };
    let result = compute_diff(&inputs)?;
    write_diff(&result, args.output.as_deref(), args.output_format)?;
    if let Some(path) = &args.stats {
        crate::stats::write_summary_json(&result.summary, path)?;
    }
    Ok(result.stats)
}

/// Stream-iterate the bnode-free common triples of two RDF files.
pub fn stream_common_triples<F: FnMut(&Triple) -> Result<()>>(
    file_a: &Path,
    file_b: &Path,
    format_a: Option<InputFormat>,
    format_b: Option<InputFormat>,
    normalization: Normalization,
    mut on_triple: F,
) -> Result<()> {
    let fmt_a = detect_or_override(file_a, format_a)?;
    let fmt_b = detect_or_override(file_b, format_b)?;

    let mut set: HashSet<Triple> = HashSet::new();
    let reader_a = open_reader(file_a)?;
    parse_triples(reader_a, fmt_a, |t| {
        set.insert(normalization.triple(t));
        Ok(())
    })
    .with_context(|| format!("while parsing {}", file_a.display()))?;

    let reader_b = open_reader(file_b)?;
    parse_triples(reader_b, fmt_b, |t| {
        let t = normalization.triple(t);
        if set.contains(&t) {
            on_triple(&t)?;
        }
        Ok(())
    })
    .with_context(|| format!("while parsing {}", file_b.display()))?;
    Ok(())
}

/// Load a previously-written diff file (TriG or N-Quads). Expects the
/// triple-mode topology: a single document with two named graphs.
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

    let mut a_only: Vec<Quad> = Vec::new();
    let mut b_only: Vec<Quad> = Vec::new();
    for q in quads {
        match &q.graph_name {
            GraphName::NamedNode(g) if g == &graph_a => a_only.push(q),
            GraphName::NamedNode(g) if g == &graph_b => b_only.push(q),
            _ => {}
        }
    }

    let changes = find_changes(&a_only, &b_only, false);
    let stats = DiffStats {
        a_total: a_only.len() as u64,
        b_total: b_only.len() as u64,
        common: 0,
        a_only: a_only.len() as u64,
        b_only: b_only.len() as u64,
        changed: changes.len() as u64,
        a_skipped_bnodes: 0,
        b_skipped_bnodes: 0,
    };
    let summary = SummaryBuilder::from_diff_only().finish(&stats, &a_only, &b_only, &changes);

    Ok(DiffResult {
        a_only,
        b_only,
        changes,
        normalization: Normalization::default(),
        prefixes,
        graph_a,
        graph_b,
        stats,
        summary,
        source_a: None,
        source_b: None,
        format_a: None,
        format_b: None,
        quad_mode: false,
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
            ignore_blank_nodes: false,
            normalization: Normalization::default(),
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

        let computed_a: HashSet<Triple> = computed.a_only_triples().collect();
        let computed_b: HashSet<Triple> = computed.b_only_triples().collect();
        let loaded_a: HashSet<Triple> = loaded.a_only_triples().collect();
        let loaded_b: HashSet<Triple> = loaded.b_only_triples().collect();
        assert_eq!(computed_a, loaded_a);
        assert_eq!(computed_b, loaded_b);
    }

    fn q(s: &str, p: &str, o: Term) -> Quad {
        Quad::new(
            NamedNode::new_unchecked(s),
            NamedNode::new_unchecked(p),
            o,
            GraphName::DefaultGraph,
        )
    }

    #[test]
    fn find_changes_pairs_single_value_changes_only() {
        use oxrdf::Literal;
        let lit = |v: &str| Term::Literal(Literal::new_simple_literal(v));
        let lang =
            |v: &str, l: &str| Term::Literal(Literal::new_language_tagged_literal_unchecked(v, l));
        let a_only = vec![
            q("ex:s", "ex:name", lit("old")),
            q("ex:s", "ex:tag", lit("t1")),
            q("ex:s", "ex:tag", lit("t2")),
            q("ex:s", "ex:label", lang("Hallo", "nl")),
            q("ex:s", "ex:label", lang("Hello", "en")),
            q("ex:gone", "ex:name", lit("x")),
        ];
        let b_only = vec![
            q("ex:s", "ex:label", lang("Hi", "en")),
            q("ex:s", "ex:tag", lit("t3")),
            q("ex:s", "ex:name", lit("new")),
            q("ex:s", "ex:label", lang("Dag", "nl")),
        ];
        // name: 1→1 pairs; tag: 2 removed, 1 added does not; labels pair
        // per language; ex:gone has no added counterpart.
        assert_eq!(
            find_changes(&a_only, &b_only, false),
            vec![(0, 2), (3, 3), (4, 0)]
        );
    }
}
