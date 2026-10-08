//! Higher-level statistics on top of the triple-level diff: totals,
//! per-predicate and per-class (`rdf:type`) breakdowns, and the subjects
//! touched by the diff.

use crate::diff::DiffStats;
use anyhow::{Context, Result};
use oxrdf::vocab::rdf;
use oxrdf::{NamedNode, NamedOrBlankNode, Quad, Term};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

/// Number of most-changed subjects listed in [`DiffSummary::top_subjects`].
pub const TOP_SUBJECTS: usize = 50;

const IN_A: u8 = 1;
const IN_B: u8 = 2;

#[derive(Debug, Clone, Serialize)]
pub struct DiffSummary {
    pub totals: Totals,
    pub subjects: SubjectStats,
    /// Sorted by number of changed triples (descending), then IRI.
    pub predicates: Vec<PredicateStats>,
    /// Sorted by number of changed triples (descending), then IRI. Subjects
    /// without any `rdf:type` are grouped under `class: null`.
    pub classes: Vec<ClassStats>,
    /// The [`TOP_SUBJECTS`] subjects with the most changed triples.
    pub top_subjects: Vec<SubjectChange>,
    /// Affected subjects per class (`None` = untyped), used by the web viewer
    /// to filter the triple table on a class. Not part of the JSON output.
    #[serde(skip)]
    pub class_subjects: HashMap<Option<String>, Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    /// `None` when the summary was built from a saved diff file.
    pub a_total: Option<u64>,
    pub b_total: Option<u64>,
    pub common: Option<u64>,
    /// Triples present in B but not in A.
    pub added: u64,
    /// Triples present in A but not in B.
    pub removed: u64,
    pub a_skipped_bnodes: u64,
    pub b_skipped_bnodes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubjectStats {
    /// Distinct subjects in A / B. `None` for a saved diff file.
    pub a_total: Option<u64>,
    pub b_total: Option<u64>,
    /// Distinct subjects with at least one added or removed triple.
    pub affected: u64,
    /// Affected subjects that only occur (as subject) in B.
    /// `None` for a saved diff file.
    pub added: Option<u64>,
    /// Affected subjects that only occur (as subject) in A.
    pub removed: Option<u64>,
    /// Affected subjects that occur on both sides.
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PredicateStats {
    pub predicate: String,
    pub added: u64,
    pub removed: u64,
    /// `None` for a saved diff file.
    pub a_total: Option<u64>,
    pub b_total: Option<u64>,
    pub common: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassStats {
    /// Class IRI, or `None` for untyped subjects.
    pub class: Option<String>,
    /// `?s rdf:type <class>` triples added (new instances).
    pub instances_added: u64,
    /// `?s rdf:type <class>` triples removed (dropped instances).
    pub instances_removed: u64,
    /// Affected subjects typed with this class (in A or B).
    pub subjects_affected: u64,
    /// Added / removed triples whose subject is typed with this class. A
    /// subject with several classes counts towards each of them.
    pub triples_added: u64,
    pub triples_removed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectStatus {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubjectChange {
    pub subject: String,
    pub added: u64,
    pub removed: u64,
    /// `None` for a saved diff file.
    pub status: Option<SubjectStatus>,
}

/// Collects per-side information while the inputs are read, then folds it
/// together with the diff into a [`DiffSummary`].
#[derive(Debug, Default)]
pub struct SummaryBuilder {
    /// When false (saved diff file) the A/B totals are unknown.
    full: bool,
    pred_totals: HashMap<NamedNode, (u64, u64)>,
    /// Bit set of [`IN_A`] / [`IN_B`].
    subjects: HashMap<NamedOrBlankNode, u8>,
    types: HashMap<NamedOrBlankNode, HashSet<NamedNode>>,
}

impl SummaryBuilder {
    /// Builder for a diff computed from both source files. Every statement
    /// of A and B must be passed to [`observe_a`](Self::observe_a) /
    /// [`observe_b`](Self::observe_b).
    pub fn new() -> Self {
        Self {
            full: true,
            ..Self::default()
        }
    }

    /// Builder for a saved diff file, where only the changed triples are
    /// known. Classes are taken from the `rdf:type` triples in the diff.
    pub fn from_diff_only() -> Self {
        Self::default()
    }

    pub fn observe_a(&mut self, s: &NamedOrBlankNode, p: &NamedNode, o: &Term) {
        self.observe(s, p, o, IN_A);
    }

    pub fn observe_b(&mut self, s: &NamedOrBlankNode, p: &NamedNode, o: &Term) {
        self.observe(s, p, o, IN_B);
    }

    fn observe(&mut self, s: &NamedOrBlankNode, p: &NamedNode, o: &Term, side: u8) {
        let e = self.pred_totals.entry(p.clone()).or_default();
        if side == IN_A {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
        match self.subjects.get_mut(s) {
            Some(bits) => *bits |= side,
            None => {
                self.subjects.insert(s.clone(), side);
            }
        }
        self.observe_type(s, p, o);
    }

    fn observe_type(&mut self, s: &NamedOrBlankNode, p: &NamedNode, o: &Term) {
        if p.as_ref() == rdf::TYPE
            && let Term::NamedNode(c) = o
        {
            self.types.entry(s.clone()).or_default().insert(c.clone());
        }
    }

    pub fn finish(mut self, stats: &DiffStats, a_only: &[Quad], b_only: &[Quad]) -> DiffSummary {
        if !self.full {
            for q in a_only.iter().chain(b_only) {
                self.observe_type(&q.subject, &q.predicate, &q.object);
            }
        }

        // Per-predicate and per-subject change counts.
        let mut pred_changes: HashMap<&NamedNode, (u64, u64)> = HashMap::new();
        let mut subj_changes: HashMap<&NamedOrBlankNode, (u64, u64)> = HashMap::new();
        let mut instances: HashMap<&NamedNode, (u64, u64)> = HashMap::new();
        for (quads, added) in [(b_only, true), (a_only, false)] {
            for q in quads {
                let bump = |e: &mut (u64, u64)| {
                    if added { e.0 += 1 } else { e.1 += 1 }
                };
                bump(pred_changes.entry(&q.predicate).or_default());
                bump(subj_changes.entry(&q.subject).or_default());
                if q.predicate.as_ref() == rdf::TYPE
                    && let Term::NamedNode(c) = &q.object
                {
                    bump(instances.entry(c).or_default());
                }
            }
        }

        let status_of = |s: &NamedOrBlankNode| -> Option<SubjectStatus> {
            if !self.full {
                return None;
            }
            Some(match self.subjects.get(s).copied().unwrap_or(0) {
                IN_B => SubjectStatus::Added,
                IN_A => SubjectStatus::Removed,
                _ => SubjectStatus::Modified,
            })
        };

        // Subjects.
        let (mut s_added, mut s_removed, mut s_modified) = (0u64, 0u64, 0u64);
        let mut classes: HashMap<Option<&NamedNode>, ClassStats> = HashMap::new();
        let mut class_subjects: HashMap<Option<String>, Vec<String>> = HashMap::new();
        let mut subject_changes: Vec<SubjectChange> = Vec::with_capacity(subj_changes.len());
        for (s, &(added, removed)) in &subj_changes {
            let subject = term_str(s);
            let status = status_of(s);
            match status {
                Some(SubjectStatus::Added) => s_added += 1,
                Some(SubjectStatus::Removed) => s_removed += 1,
                Some(SubjectStatus::Modified) => s_modified += 1,
                None => {}
            }
            let types = self.types.get(*s);
            let keys: Vec<Option<&NamedNode>> = match types {
                Some(t) if !t.is_empty() => t.iter().map(Some).collect(),
                _ => vec![None],
            };
            for k in keys {
                let c = classes.entry(k).or_insert_with(|| empty_class(k));
                c.subjects_affected += 1;
                c.triples_added += added;
                c.triples_removed += removed;
                class_subjects
                    .entry(k.map(|c| c.as_str().to_string()))
                    .or_default()
                    .push(subject.clone());
            }
            subject_changes.push(SubjectChange {
                subject,
                added,
                removed,
                status,
            });
        }
        for (c, (added, removed)) in instances {
            let e = classes
                .entry(Some(c))
                .or_insert_with(|| empty_class(Some(c)));
            e.instances_added = added;
            e.instances_removed = removed;
        }

        subject_changes.sort_unstable_by(|a, b| {
            (b.added + b.removed)
                .cmp(&(a.added + a.removed))
                .then_with(|| a.subject.cmp(&b.subject))
        });
        subject_changes.truncate(TOP_SUBJECTS);

        let mut predicates: Vec<PredicateStats> = pred_changes
            .iter()
            .map(|(p, &(added, removed))| {
                let totals = self.full.then(|| {
                    let (a, b) = self.pred_totals.get(*p).copied().unwrap_or_default();
                    (a, b, a.saturating_sub(removed))
                });
                PredicateStats {
                    predicate: p.as_str().to_string(),
                    added,
                    removed,
                    a_total: totals.map(|t| t.0),
                    b_total: totals.map(|t| t.1),
                    common: totals.map(|t| t.2),
                }
            })
            .collect();
        predicates.sort_unstable_by(|a, b| {
            (b.added + b.removed)
                .cmp(&(a.added + a.removed))
                .then_with(|| a.predicate.cmp(&b.predicate))
        });

        let mut classes: Vec<ClassStats> = classes.into_values().collect();
        classes.sort_unstable_by(|a, b| {
            let wa = a.triples_added + a.triples_removed + a.instances_added + a.instances_removed;
            let wb = b.triples_added + b.triples_removed + b.instances_added + b.instances_removed;
            wb.cmp(&wa).then_with(|| a.class.cmp(&b.class))
        });

        let count_side = |side: u8| {
            self.full.then(|| {
                self.subjects
                    .values()
                    .filter(|&&bits| bits & side != 0)
                    .count() as u64
            })
        };

        DiffSummary {
            totals: Totals {
                a_total: self.full.then_some(stats.a_total),
                b_total: self.full.then_some(stats.b_total),
                common: self.full.then_some(stats.common),
                added: b_only.len() as u64,
                removed: a_only.len() as u64,
                a_skipped_bnodes: stats.a_skipped_bnodes,
                b_skipped_bnodes: stats.b_skipped_bnodes,
            },
            subjects: SubjectStats {
                a_total: count_side(IN_A),
                b_total: count_side(IN_B),
                affected: subj_changes.len() as u64,
                added: self.full.then_some(s_added),
                removed: self.full.then_some(s_removed),
                modified: self.full.then_some(s_modified),
            },
            predicates,
            classes,
            top_subjects: subject_changes,
            class_subjects,
        }
    }
}

fn empty_class(c: Option<&NamedNode>) -> ClassStats {
    ClassStats {
        class: c.map(|c| c.as_str().to_string()),
        instances_added: 0,
        instances_removed: 0,
        subjects_affected: 0,
        triples_added: 0,
        triples_removed: 0,
    }
}

fn term_str(s: &NamedOrBlankNode) -> String {
    match s {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
    }
}

/// Write `summary` as pretty-printed JSON to `path` (`-` for stdout).
pub fn write_summary_json(summary: &DiffSummary, path: &Path) -> Result<()> {
    let mut w: Box<dyn Write> = if path.as_os_str() == "-" {
        Box::new(std::io::stdout().lock())
    } else {
        Box::new(std::io::BufWriter::new(
            std::fs::File::create(path)
                .with_context(|| format!("failed to create {}", path.display()))?,
        ))
    };
    serde_json::to_writer_pretty(&mut w, summary).context("failed to serialize statistics")?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}
