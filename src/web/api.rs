//! HTTP routes for the rdf-compare web viewer.

use super::{AppState, assets};
use crate::diff::{
    DiffInputs, DiffResult, LoadDiffInputs, compute_diff, load_diff_file, stream_common_triples,
};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use oxrdf::{Quad, Term};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/assets/*path", get(asset))
        .route("/licenses", get(licenses))
        .route("/api/meta", get(meta))
        .route("/api/rows", get(rows))
        .route("/api/summary", get(summary))
        .route("/api/class-subjects", get(class_subjects))
        .route("/api/files", get(files))
        .with_state(state)
}

async fn root() -> Response {
    match assets::lookup("/assets/app/index.html") {
        Some(a) => Html(std::str::from_utf8(a.bytes).unwrap_or("")).into_response(),
        None => (StatusCode::NOT_FOUND, "missing").into_response(),
    }
}

/// Licenses of the third-party software bundled in the binary.
async fn licenses() -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        include_str!("../../THIRD_PARTY_NOTICES.md"),
    )
        .into_response()
}

async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let full = format!("/assets/{path}");
    match assets::lookup(&full) {
        Some(a) => {
            let mut resp = Response::new(Body::from(a.bytes));
            resp.headers_mut()
                .insert(header::CONTENT_TYPE, a.mime.parse().unwrap());
            resp
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[derive(Serialize)]
struct StatsDto {
    a_total: u64,
    b_total: u64,
    a_only: u64,
    b_only: u64,
    common: u64,
    a_skipped_bnodes: u64,
    b_skipped_bnodes: u64,
}

#[derive(Serialize)]
struct MetaDto {
    version: &'static str,
    loaded: bool,
    from_diff_file: bool,
    graph_a: Option<String>,
    graph_b: Option<String>,
    stats: Option<StatsDto>,
    prefixes: Vec<(String, String)>,
}

async fn meta(State(s): State<AppState>, Query(src): Query<SourceQuery>) -> Response {
    let data = match diff_for(&s, src).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match data.as_deref() {
        None => Json(MetaDto {
            version: env!("CARGO_PKG_VERSION"),
            loaded: false,
            from_diff_file: false,
            graph_a: None,
            graph_b: None,
            stats: None,
            prefixes: vec![],
        }),
        Some(d) => Json(MetaDto {
            version: env!("CARGO_PKG_VERSION"),
            loaded: true,
            from_diff_file: d.source_a.is_none() && d.source_b.is_none(),
            graph_a: Some(d.graph_a.as_str().to_string()),
            graph_b: Some(d.graph_b.as_str().to_string()),
            stats: Some(StatsDto {
                a_total: d.stats.a_total,
                b_total: d.stats.b_total,
                a_only: d.stats.a_only,
                b_only: d.stats.b_only,
                common: d.stats.common,
                a_skipped_bnodes: d.stats.a_skipped_bnodes,
                b_skipped_bnodes: d.stats.b_skipped_bnodes,
            }),
            prefixes: d.prefixes.clone(),
        }),
    }
    .into_response()
}

async fn summary(State(s): State<AppState>, Query(src): Query<SourceQuery>) -> Response {
    let data = match diff_for(&s, src).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match data.as_deref() {
        Some(d) => Json(&d.summary).into_response(),
        None => (StatusCode::CONFLICT, "no diff loaded").into_response(),
    }
}

#[derive(Deserialize)]
struct ClassQuery {
    /// Class IRI; omitted for untyped subjects.
    #[serde(default)]
    class: Option<String>,
}

/// Affected subjects of one class, for filtering the triple table.
async fn class_subjects(
    State(s): State<AppState>,
    Query(q): Query<ClassQuery>,
    Query(src): Query<SourceQuery>,
) -> Response {
    let data = match diff_for(&s, src).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match data.as_deref() {
        Some(d) => {
            let empty = Vec::new();
            let subjects = d.summary.class_subjects.get(&q.class).unwrap_or(&empty);
            Json(subjects).into_response()
        }
        None => (StatusCode::CONFLICT, "no diff loaded").into_response(),
    }
}

#[derive(Deserialize)]
struct RowsQuery {
    #[serde(default)]
    include: Option<String>,
}

#[derive(Serialize)]
struct ObjectDto<'a> {
    t: &'a str,
    v: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lng: Option<&'a str>,
}

#[derive(Serialize)]
struct RowDto<'a> {
    a: &'a str,
    s: String,
    p: &'a str,
    o: ObjectDto<'a>,
}

fn write_row<W: std::io::Write>(w: &mut W, action: &str, q: &Quad) -> std::io::Result<()> {
    let s = match &q.subject {
        oxrdf::NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        oxrdf::NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
    };
    let p = q.predicate.as_str();
    let obj = match &q.object {
        Term::NamedNode(n) => ObjectDto {
            t: "iri",
            v: n.as_str(),
            dt: None,
            lng: None,
        },
        Term::BlankNode(b) => ObjectDto {
            t: "bnode",
            v: b.as_str(),
            dt: None,
            lng: None,
        },
        Term::Literal(l) => {
            let lng = l.language();
            let dt = if lng.is_some() {
                None
            } else {
                Some(l.datatype().as_str())
            };
            ObjectDto {
                t: "lit",
                v: l.value(),
                dt,
                lng,
            }
        }
        #[allow(unreachable_patterns)]
        _ => ObjectDto {
            t: "iri",
            v: "",
            dt: None,
            lng: None,
        },
    };
    let row = RowDto {
        a: action,
        s,
        p,
        o: obj,
    };
    serde_json::to_writer(&mut *w, &row)?;
    w.write_all(b"\n")
}

/// Render NDJSON for the in-memory diff (added + deleted triples).
pub fn render_diff_ndjson(data: &DiffResult) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256 * 1024);
    for t in &data.b_only {
        let _ = write_row(&mut buf, "+", t);
    }
    for t in &data.a_only {
        let _ = write_row(&mut buf, "-", t);
    }
    buf
}

async fn rows(
    State(s): State<AppState>,
    Query(q): Query<RowsQuery>,
    Query(src): Query<SourceQuery>,
) -> Response {
    let include = q.include.as_deref().unwrap_or("diff").to_string();
    let data_arc = match diff_for(&s, src).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let Some(data) = data_arc else {
        return (StatusCode::CONFLICT, "no diff loaded").into_response();
    };

    let body: Body = match include.as_str() {
        "diff" => {
            let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(128);
            tokio::task::spawn_blocking(move || {
                for t in &data.b_only {
                    let mut buf = Vec::with_capacity(256);
                    let _ = write_row(&mut buf, "+", t);
                    if tx.blocking_send(buf).is_err() {
                        return;
                    }
                }
                for t in &data.a_only {
                    let mut buf = Vec::with_capacity(256);
                    let _ = write_row(&mut buf, "-", t);
                    if tx.blocking_send(buf).is_err() {
                        return;
                    }
                }
            });
            let stream = ReceiverStream::new(rx)
                .map(|b| Result::<_, std::io::Error>::Ok(axum::body::Bytes::from(b)));
            Body::from_stream(stream)
        }
        "common" => {
            if data.source_a.is_none() || data.source_b.is_none() {
                return (
                    StatusCode::CONFLICT,
                    "common triples unavailable: dataset was loaded from a diff file",
                )
                    .into_response();
            }
            let res = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
                let (Some(file_a), Some(file_b)) = (data.source_a.clone(), data.source_b.clone())
                else {
                    return Ok(Vec::new());
                };
                let fmt_a = data.format_a;
                let fmt_b = data.format_b;
                let mut buf = Vec::with_capacity(256 * 1024);
                stream_common_triples(&file_a, &file_b, fmt_a, fmt_b, |t| {
                    let q = Quad {
                        subject: t.subject.clone(),
                        predicate: t.predicate.clone(),
                        object: t.object.clone(),
                        graph_name: oxrdf::GraphName::DefaultGraph,
                    };
                    write_row(&mut buf, "=", &q).map_err(anyhow::Error::from)?;
                    Ok(())
                })?;
                Ok(buf)
            })
            .await;
            match res {
                Ok(Ok(b)) => Body::from(b),
                Ok(Err(e)) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
                }
                Err(_) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "task panic").into_response();
                }
            }
        }
        _ => return (StatusCode::BAD_REQUEST, "unknown include").into_response(),
    };

    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/x-ndjson".parse().unwrap(),
    );
    resp
}

/// Which diff a request is about, taken from the page URL's query string:
/// `a` + `b` (two source files) or `diff` (a saved diff file). With none of
/// them the diff preloaded from the command line is used.
#[derive(Debug, Default, Deserialize)]
pub struct SourceQuery {
    a: Option<PathBuf>,
    b: Option<PathBuf>,
    diff: Option<PathBuf>,
    graph_a: Option<String>,
    graph_b: Option<String>,
    #[serde(default)]
    ignore_blank_nodes: bool,
}

/// Identifies a file's content for cache purposes: path, size and mtime.
fn file_key(path: &Path) -> anyhow::Result<String> {
    let md = std::fs::metadata(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(format!("{}|{}|{}", path.display(), md.len(), mtime))
}

/// Resolve the diff named by `src`: `Ok(None)` when it names nothing and no
/// diff was preloaded, an error status for bad parameters or failed diffs.
async fn diff_for(
    s: &AppState,
    src: SourceQuery,
) -> Result<Option<Arc<DiffResult>>, (StatusCode, String)> {
    let bad = |e: anyhow::Error| (StatusCode::BAD_REQUEST, format!("{e:#}"));
    let root = s.data_dir.as_deref().map(PathBuf::as_path);
    let resolve = |p: &Path| resolve_in_data_dir(root, p).map_err(bad);
    let opts = format!(
        "{:?}|{:?}|{}",
        src.graph_a, src.graph_b, src.ignore_blank_nodes
    );
    let (key, compute): (
        String,
        Box<dyn FnOnce() -> anyhow::Result<DiffResult> + Send>,
    ) = match (src.a, src.b, src.diff) {
        (None, None, None) => return Ok(s.default.clone()),
        (None, None, Some(diff)) => {
            let diff = resolve(&diff)?;
            let key = format!("diff|{}|{opts}", file_key(&diff).map_err(bad)?);
            let inputs = LoadDiffInputs {
                diff,
                format: None,
                graph_a: src.graph_a,
                graph_b: src.graph_b,
            };
            (key, Box::new(move || load_diff_file(&inputs)))
        }
        (Some(a), Some(b), None) => {
            let (a, b) = (resolve(&a)?, resolve(&b)?);
            let key = format!(
                "files|{}|{}|{opts}",
                file_key(&a).map_err(bad)?,
                file_key(&b).map_err(bad)?
            );
            let inputs = DiffInputs {
                file_a: a,
                file_b: b,
                format_a: None,
                format_b: None,
                graph_a: src.graph_a,
                graph_b: src.graph_b,
                ignore_blank_nodes: src.ignore_blank_nodes,
            };
            (key, Box::new(move || compute_diff(&inputs)))
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "provide both `a` and `b`, or `diff` on its own".to_string(),
            ));
        }
    };
    s.cache
        .get_or_compute(&key, compute)
        .await
        .map(Some)
        .map_err(bad)
}

/// Resolve a user-supplied path against the data directory. Relative paths
/// are joined onto it; the canonical result must stay inside it so `..` and
/// symlinks cannot escape. Without a data directory the path is used as-is.
pub fn resolve_in_data_dir(root: Option<&Path>, path: &Path) -> anyhow::Result<PathBuf> {
    let Some(root) = root else {
        return Ok(path.to_path_buf());
    };
    let joined = root.join(path);
    let canonical = joined
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    if !canonical.starts_with(root) {
        anyhow::bail!("{} is outside the data directory", path.display());
    }
    Ok(canonical)
}

const MAX_LISTED_FILES: usize = 10_000;
const MAX_LIST_DEPTH: usize = 16;

/// Recursively collect RDF files (by extension) under `root`, as paths
/// relative to it, sorted.
pub fn list_rdf_files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_LIST_DEPTH || out.len() >= MAX_LISTED_FILES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_LISTED_FILES {
                return;
            }
            let path = entry.path();
            let name = entry.file_name();
            // Skip hidden entries such as Kubernetes' `..data` volume symlinks.
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                walk(root, &path, depth + 1, out);
            } else if crate::cli::detect_format(&path).is_ok()
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, 0, &mut out);
    out.sort();
    out
}

#[derive(Serialize)]
struct FilesDto {
    enabled: bool,
    files: Vec<String>,
}

async fn files(State(s): State<AppState>) -> Json<FilesDto> {
    let Some(root) = s.data_dir.clone() else {
        return Json(FilesDto {
            enabled: false,
            files: vec![],
        });
    };
    let files = tokio::task::spawn_blocking(move || list_rdf_files(&root))
        .await
        .unwrap_or_default();
    Json(FilesDto {
        enabled: true,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffInputs, compute_diff};
    use std::path::PathBuf;

    fn fixtures(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests");
        p.push("fixtures");
        p.push(name);
        p
    }

    #[test]
    fn data_dir_resolves_relative_and_rejects_escapes() {
        let root = fixtures("").canonicalize().unwrap();
        let ok = resolve_in_data_dir(Some(&root), Path::new("a.ttl")).unwrap();
        assert_eq!(ok, root.join("a.ttl"));
        let abs = resolve_in_data_dir(Some(&root), &root.join("b.ttl")).unwrap();
        assert_eq!(abs, root.join("b.ttl"));
        assert!(resolve_in_data_dir(Some(&root), Path::new("../cli.rs")).is_err());
        assert!(resolve_in_data_dir(Some(&root), Path::new("/etc/passwd")).is_err());
        assert!(resolve_in_data_dir(Some(&root), Path::new("missing.ttl")).is_err());
    }

    #[test]
    fn lists_rdf_files_under_data_dir() {
        let root = fixtures("").canonicalize().unwrap();
        let files = list_rdf_files(&root);
        assert!(files.contains(&"a.ttl".to_string()));
        assert!(files.contains(&"quads-a.nq".to_string()));
        assert!(files.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn ndjson_renders_one_line_per_triple() {
        let inputs = DiffInputs {
            file_a: fixtures("a.ttl"),
            file_b: fixtures("b.ttl"),
            format_a: None,
            format_b: None,
            graph_a: None,
            graph_b: None,
            ignore_blank_nodes: false,
        };
        let d = compute_diff(&inputs).unwrap();
        let bytes = render_diff_ndjson(&d);
        let s = std::str::from_utf8(&bytes).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len() as u64, d.stats.a_only + d.stats.b_only);
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(matches!(v["a"].as_str(), Some("+") | Some("-")));
            assert!(v["s"].is_string());
            assert!(v["p"].is_string());
            assert!(v["o"].is_object());
        }
    }

    #[test]
    fn class_subjects_lists_affected_subjects_per_class() {
        let inputs = DiffInputs {
            file_a: fixtures("summary-a.ttl"),
            file_b: fixtures("summary-b.ttl"),
            format_a: None,
            format_b: None,
            graph_a: None,
            graph_b: None,
            ignore_blank_nodes: false,
        };
        let d = compute_diff(&inputs).unwrap();
        let cs = &d.summary.class_subjects;
        let mut person = cs
            .get(&Some("http://example.org/Person".to_string()))
            .unwrap()
            .clone();
        person.sort();
        assert_eq!(
            person,
            ["http://example.org/alice", "http://example.org/carol"]
        );
        assert_eq!(cs.get(&None).unwrap(), &["http://example.org/gone"]);
        // Classes without changes have no entry.
        assert!(!cs.contains_key(&Some("http://example.org/Company".to_string())));
    }
}
