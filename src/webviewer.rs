use crate::cli::{Args, InputFormat};
use crate::input::{open_reader, parse_triples};
use anyhow::{Context, Result};
use oxrdf::{NamedOrBlankNode, Term, Triple};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

const GEO_WKT_LITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";

#[derive(Debug, Serialize, Clone)]
struct Row {
    subject: String,
    predicate: String,
    object: String,
    prefix: String,
    common: bool,
    wkt: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct SubjectDetail {
    common: Vec<Row>,
    left_distinct: Vec<Row>,
    right_distinct: Vec<Row>,
}

#[derive(Debug, Serialize)]
struct WebData {
    left: Vec<Row>,
    right: Vec<Row>,
    details: BTreeMap<String, SubjectDetail>,
}

pub fn run_webviewer(args: &Args) -> Result<()> {
    let fmt_a = detect_or_override(&args.file_a, args.format_a)?;
    let fmt_b = detect_or_override(&args.file_b, args.format_b)?;

    let mut a: HashSet<Triple> = HashSet::new();
    let mut b: HashSet<Triple> = HashSet::new();

    let out_a = parse_triples(open_reader(&args.file_a)?, fmt_a, |t| {
        a.insert(t);
        Ok(())
    })
    .with_context(|| format!("while parsing {}", args.file_a.display()))?;
    let out_b = parse_triples(open_reader(&args.file_b)?, fmt_b, |t| {
        b.insert(t);
        Ok(())
    })
    .with_context(|| format!("while parsing {}", args.file_b.display()))?;

    let mut prefixes = out_a.prefixes;
    let mut seen: HashSet<String> = prefixes.iter().map(|(k, _)| k.clone()).collect();
    for (k, v) in out_b.prefixes {
        if seen.insert(k.clone()) {
            prefixes.push((k, v));
        }
    }

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut details: BTreeMap<String, SubjectDetail> = BTreeMap::new();

    for t in &a {
        let common = b.contains(t);
        let row = row_from_triple(t, common, &prefixes);
        details
            .entry(row.subject.clone())
            .or_default()
            .push(row.clone(), true, common);
        left.push(row);
    }
    for t in &b {
        let common = a.contains(t);
        let row = row_from_triple(t, common, &prefixes);
        details
            .entry(row.subject.clone())
            .or_default()
            .push(row.clone(), false, common);
        right.push(row);
    }

    let data = WebData {
        left,
        right,
        details,
    };
    let data_json = escape_script_json(&serde_json::to_string(&data)?);
    let html = build_html(&data_json, args);
    let bind_addr = format!("{}:{}", args.webviewer_host, args.webviewer_port);
    let listener = TcpListener::bind(&bind_addr)
        .with_context(|| format!("failed to bind webviewer to {bind_addr}"))?;
    let local = listener
        .local_addr()
        .context("failed to read bound webviewer address")?;

    eprintln!(
        "webviewer listening on http://{}/ (left={}, right={}, common={})",
        local,
        out_a.total,
        out_b.total,
        a.intersection(&b).count()
    );
    eprintln!("Press Ctrl+C to stop.");

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(err) => {
                eprintln!("webviewer: failed to accept connection: {err}");
                continue;
            }
        };
        if let Err(err) = handle_connection(stream, &html) {
            eprintln!("webviewer: request handling error: {err}");
        }
    }
    Ok(())
}

fn detect_or_override(path: &Path, over: Option<InputFormat>) -> Result<InputFormat> {
    match over {
        Some(f) => Ok(f),
        None => crate::cli::detect_format(path),
    }
}

fn row_from_triple(t: &Triple, common: bool, prefixes: &[(String, String)]) -> Row {
    let subject = t.subject.to_string();
    let predicate = t.predicate.to_string();
    let object = t.object.to_string();
    Row {
        prefix: infer_prefix(t, prefixes),
        subject,
        predicate,
        object,
        common,
        wkt: extract_wkt(&t.object),
    }
}

fn infer_prefix(t: &Triple, prefixes: &[(String, String)]) -> String {
    let mut matched = Vec::new();
    let subject = match &t.subject {
        NamedOrBlankNode::NamedNode(n) => Some(n.as_str()),
        NamedOrBlankNode::BlankNode(_) => None,
    };
    let predicate = Some(t.predicate.as_str());
    let object = match &t.object {
        Term::NamedNode(n) => Some(n.as_str()),
        _ => None,
    };

    for (name, iri) in prefixes {
        let hit = subject.is_some_and(|s| s.starts_with(iri))
            || predicate.is_some_and(|p| p.starts_with(iri))
            || object.is_some_and(|o| o.starts_with(iri));
        if hit {
            matched.push(name.as_str());
        }
    }
    matched.join(",")
}

fn extract_wkt(object: &Term) -> Option<String> {
    let lit = match object {
        Term::Literal(l) => l,
        _ => return None,
    };
    if lit.datatype().as_str() != GEO_WKT_LITERAL {
        return None;
    }
    Some(lit.value().to_string())
}

impl SubjectDetail {
    fn push(&mut self, row: Row, left: bool, common: bool) {
        if common {
            self.common.push(row);
        } else if left {
            self.left_distinct.push(row);
        } else {
            self.right_distinct.push(row);
        }
    }
}

fn handle_connection(mut stream: TcpStream, body: &str) -> Result<()> {
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader
            .read_line(&mut request_line)
            .context("failed to read request line")?;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next();
    let path = parts.next();
    let version = parts.next();

    let valid_get = matches!(method, Some("GET")) && version.is_some();
    if valid_get && path == Some("/") {
        let bytes = body.as_bytes();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )?;
        stream.write_all(bytes)?;
    } else {
        write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found"
        )?;
    }
    Ok(())
}

fn build_html(data_json: &str, args: &Args) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>rdf-compare webviewer</title>
  <style>
    body {{ font-family: system-ui, sans-serif; margin: 0; }}
    .bar {{ padding: 12px; border-bottom: 1px solid #ddd; display: grid; grid-template-columns: repeat(5, minmax(120px, 1fr)); gap: 8px; }}
    .content {{ display: grid; grid-template-columns: 1fr 1fr; gap: 12px; padding: 12px; }}
    .panel {{ border: 1px solid #ddd; border-radius: 4px; overflow: hidden; }}
    .panel h2 {{ margin: 0; padding: 8px 10px; background: #f7f7f7; font-size: 14px; }}
    table {{ width: 100%; border-collapse: collapse; font-size: 12px; }}
    th, td {{ border-top: 1px solid #eee; padding: 6px; text-align: left; vertical-align: top; }}
    th {{ background: #fafafa; position: sticky; top: 0; }}
    .scroller {{ max-height: 45vh; overflow: auto; }}
    .common {{ background: #f2fff2; }}
    .detail {{ padding: 12px; border-top: 1px solid #ddd; }}
    .chip {{ display: inline-block; font-size: 11px; border: 1px solid #ddd; border-radius: 999px; padding: 2px 7px; margin-right: 4px; }}
    .map {{ width: 100%; height: 260px; border: 1px solid #ddd; background: linear-gradient(#f7fbff, #edf5ff); }}
    code {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }}
  </style>
</head>
<body>
  <div class="bar">
    <input id="subjectFilter" placeholder="Filter subject…" />
    <input id="predicateFilter" placeholder="Filter predicate…" />
    <input id="prefixFilter" placeholder="Filter prefix…" />
    <select id="sortBy"><option value="subject">Sort by subject</option><option value="predicate">Sort by predicate</option></select>
    <label><input id="showCommon" type="checkbox" /> Show common triples</label>
  </div>
  <div class="content">
    <section class="panel">
      <h2>Left ({})</h2>
      <div class="scroller"><table><thead><tr><th>Subject</th><th>Predicate</th><th>Object</th><th>Prefix</th></tr></thead><tbody id="leftBody"></tbody></table></div>
    </section>
    <section class="panel">
      <h2>Right ({})</h2>
      <div class="scroller"><table><thead><tr><th>Subject</th><th>Predicate</th><th>Object</th><th>Prefix</th></tr></thead><tbody id="rightBody"></tbody></table></div>
    </section>
  </div>
  <section class="detail">
    <div><strong>Selected subject:</strong> <code id="detailSubject">(none)</code></div>
    <div id="detailContent">Click a subject row while "Show common triples" is disabled to inspect distinct/common triples for that subject.</div>
    <h3>WKT map preview</h3>
    <svg class="map" id="wktMap" viewBox="0 0 360 180" preserveAspectRatio="none"></svg>
    <div id="wktLabel"></div>
  </section>
  <script>
    const DATA = {data_json};
    const DETAILS = DATA.details || {{}};
    const state = {{
      subject: '',
      predicate: '',
      prefix: '',
      sortBy: 'subject',
      showCommon: false,
      selectedSubject: ''
    }};

    const leftBody = document.getElementById('leftBody');
    const rightBody = document.getElementById('rightBody');
    const detailSubject = document.getElementById('detailSubject');
    const detailContent = document.getElementById('detailContent');
    const wktMap = document.getElementById('wktMap');
    const wktLabel = document.getElementById('wktLabel');

    function drawMapGrid() {{
      wktMap.innerHTML = '';
      for (let x = 0; x <= 360; x += 60) {{
        const line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
        line.setAttribute('x1', x); line.setAttribute('y1', 0);
        line.setAttribute('x2', x); line.setAttribute('y2', 180);
        line.setAttribute('stroke', '#cfe0f5'); line.setAttribute('stroke-width', '0.8');
        wktMap.appendChild(line);
      }}
      for (let y = 0; y <= 180; y += 30) {{
        const line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
        line.setAttribute('x1', 0); line.setAttribute('y1', y);
        line.setAttribute('x2', 360); line.setAttribute('y2', y);
        line.setAttribute('stroke', '#cfe0f5'); line.setAttribute('stroke-width', '0.8');
        wktMap.appendChild(line);
      }}
    }}

    function plotWkt(wkt) {{
      drawMapGrid();
      if (!wkt) {{
        wktLabel.textContent = 'No WKT literal selected.';
        return;
      }}
      const pointMatch = wkt.match(/POINT\\s*\\(\\s*(-?\\d+(?:\\.\\d+)?)\\s+(-?\\d+(?:\\.\\d+)?)\\s*\\)/i);
      if (!pointMatch) {{
        wktLabel.textContent = `WKT selected (currently visualized for POINT): ${{wkt}}`;
        return;
      }}
      const lon = Number(pointMatch[1]);
      const lat = Number(pointMatch[2]);
      const x = lon + 180;
      const y = 90 - lat;
      const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
      circle.setAttribute('cx', x.toString());
      circle.setAttribute('cy', y.toString());
      circle.setAttribute('r', '3');
      circle.setAttribute('fill', '#d00');
      wktMap.appendChild(circle);
      wktLabel.textContent = `WKT POINT on map: lon=${{lon}}, lat=${{lat}}`;
    }}

    function applyFilters(rows) {{
      return rows
        .filter(r => state.showCommon || !r.common)
        .filter(r => !state.subject || r.subject.toLowerCase().includes(state.subject))
        .filter(r => !state.predicate || r.predicate.toLowerCase().includes(state.predicate))
        .filter(r => !state.prefix || (r.prefix || '').toLowerCase().includes(state.prefix))
        .sort((a, b) => (a[state.sortBy] || '').localeCompare(b[state.sortBy] || ''));
    }}

    function esc(value) {{
      return String(value ?? '')
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;')
        .replaceAll('"', '&quot;')
        .replaceAll("'", '&#39;');
    }}

    function renderRows(target, rows) {{
      target.innerHTML = '';
      for (const row of applyFilters(rows)) {{
        const tr = document.createElement('tr');
        if (row.common) tr.classList.add('common');
        tr.innerHTML = `<td><button data-subj="${{esc(row.subject)}}" style="all:unset; cursor:pointer; color:#0366d6">${{esc(row.subject)}}</button></td><td>${{esc(row.predicate)}}</td><td>${{esc(row.object)}}</td><td>${{esc(row.prefix || '')}}</td>`;
        tr.querySelector('button').addEventListener('click', () => {{
          if (state.showCommon) return;
          state.selectedSubject = row.subject;
          renderDetail();
          if (row.wkt) plotWkt(row.wkt);
        }});
        target.appendChild(tr);
      }}
    }}

    function rowsToList(rows) {{
      if (!rows || rows.length === 0) return '<em>none</em>';
      return rows.map(r => `<div><span class="chip">${{esc(r.predicate)}}</span>${{esc(r.object)}}</div>`).join('');
    }}

    function renderDetail() {{
      const key = state.selectedSubject;
      detailSubject.textContent = key || '(none)';
      if (!key || !DETAILS[key]) {{
        detailContent.innerHTML = 'Click a subject row while "Show common triples" is disabled to inspect distinct/common triples for that subject.';
        return;
      }}
      const d = DETAILS[key];
      detailContent.innerHTML = `
        <h4>Distinct triples in left</h4>
        ${{rowsToList(d.left_distinct)}}
        <h4>Distinct triples in right</h4>
        ${{rowsToList(d.right_distinct)}}
        <h4>Common triples</h4>
        ${{rowsToList(d.common)}}
      `;
    }}

    function render() {{
      renderRows(leftBody, DATA.left || []);
      renderRows(rightBody, DATA.right || []);
      renderDetail();
    }}

    document.getElementById('subjectFilter').addEventListener('input', (e) => {{
      state.subject = e.target.value.toLowerCase();
      render();
    }});
    document.getElementById('predicateFilter').addEventListener('input', (e) => {{
      state.predicate = e.target.value.toLowerCase();
      render();
    }});
    document.getElementById('prefixFilter').addEventListener('input', (e) => {{
      state.prefix = e.target.value.toLowerCase();
      render();
    }});
    document.getElementById('sortBy').addEventListener('change', (e) => {{
      state.sortBy = e.target.value;
      render();
    }});
    document.getElementById('showCommon').addEventListener('change', (e) => {{
      state.showCommon = e.target.checked;
      render();
    }});

    drawMapGrid();
    render();
  </script>
</body>
</html>"#,
        args.file_a.display(),
        args.file_b.display()
    )
}

fn escape_script_json(input: &str) -> String {
    input
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode};

    #[test]
    fn detects_geo_wkt_literal() {
        let t = Term::Literal(Literal::new_typed_literal(
            "POINT(4.35 50.85)",
            NamedNode::new(GEO_WKT_LITERAL).unwrap(),
        ));
        assert_eq!(extract_wkt(&t).as_deref(), Some("POINT(4.35 50.85)"));
    }

    #[test]
    fn escapes_script_end_tag_case_insensitive() {
        let escaped = escape_script_json(r#"{"x":"</ScRiPt>","y":"a&b"}"#);
        assert_eq!(escaped, r#"{"x":"\u003c/ScRiPt\u003e","y":"a\u0026b"}"#);
    }
}
