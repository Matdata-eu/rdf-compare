// rdf-compare web viewer — Tabulator-driven diff browser.
(function () {
  const state = {
    prefixes: [], // sorted by length desc for longest-prefix-wins shortening
    table: null,
    meta: null,
    diffLoaded: false,
    commonLoaded: false,
    wktSelection: new Set(), // wkt literal strings currently shown on the map
    classFilter: null, // { key, subjects: Set } when the table is filtered on a class
  };

  const els = {
    meta: document.getElementById("meta"),
    table: document.getElementById("table"),
    empty: document.getElementById("empty-state"),
    loader: document.getElementById("loader"),
    tripleView: document.getElementById("triple-view"),
    openLoad: document.getElementById("open-load"),
    doLoad: document.getElementById("do-load"),
    pathA: document.getElementById("path-a"),
    pathB: document.getElementById("path-b"),
    pathDiff: document.getElementById("path-diff"),
    loaderMsg: document.getElementById("loader-msg"),
    overlay: document.getElementById("loading-overlay"),
    overlayMsg: document.getElementById("loading-msg"),
    commonError: document.getElementById("common-error"),
    summary: document.getElementById("summary"),
    summaryToggle: document.getElementById("toggle-summary"),
    summaryCards: document.getElementById("summary-cards"),
    summaryPredicates: document.getElementById("summary-predicates"),
    summaryClasses: document.getElementById("summary-classes"),
    summarySubjects: document.getElementById("summary-subjects"),
  };

  // The diff shown is named by the page URL (?a=…&b=… or ?diff=…), so a
  // link opens the same diff for anyone and each tab can show its own.
  const SOURCE_PARAMS = ["a", "b", "diff", "graph_a", "graph_b", "ignore_blank_nodes"];
  const pageParams = new URLSearchParams(window.location.search);
  const sourceParams = new URLSearchParams();
  for (const k of SOURCE_PARAMS) {
    if (pageParams.has(k)) sourceParams.set(k, pageParams.get(k));
  }

  function apiUrl(path, extra) {
    const q = new URLSearchParams(sourceParams);
    for (const [k, v] of Object.entries(extra || {})) q.set(k, v);
    const qs = q.toString();
    return qs ? `${path}?${qs}` : path;
  }

  function showLoading(msg) {
    els.overlayMsg.textContent = msg || "Loading rows\u2026";
    els.overlay.classList.remove("hidden");
  }

  function hideLoading() {
    els.overlay.classList.add("hidden");
  }

  function shortenIri(iri) {
    for (const [name, base] of state.prefixes) {
      if (iri.startsWith(base)) {
        const local = iri.slice(base.length);
        if (/^[A-Za-z_][\w.\-]*$/.test(local)) return `${name}:${local}`;
      }
    }
    return null;
  }

  function escapeHtml(s) {
    return s
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function renderIri(iri) {
    const short = shortenIri(iri);
    const text = short || `<${iri}>`;
    const safeIri = escapeHtml(iri);
    const safeText = escapeHtml(text);
    return `<a class="iri-link" href="${safeIri}" target="_blank" rel="noopener noreferrer" title="${safeIri}">${safeText}</a>`;
  }

  const WKT_DATATYPE = "http://www.opengis.net/ont/geosparql#wktLiteral";

  function renderObject(o) {
    if (!o) return "";
    if (o.t === "iri") return renderIri(o.v);
    const v = escapeHtml(o.v);
    if (o.lng) return `"${v}"<span class="lit-dt">@${escapeHtml(o.lng)}</span>`;
    if (o.dt && o.dt !== "http://www.w3.org/2001/XMLSchema#string") {
      const dt = renderIri(o.dt);
      if (o.dt === WKT_DATATYPE) {
        return `<button class="wkt-map-btn" data-wkt="${v}" title="Show on map">\u{1F4CD}</button>"${v}"<span class="lit-dt">^^${dt}</span>`;
      }
      return `"${v}"<span class="lit-dt">^^${dt}</span>`;
    }
    return `"${v}"`;
  }

  function rowClass(row) {
    const a = row.getData().a;
    if (a === "+") return "row-added";
    if (a === "-") return "row-deleted";
    return "row-common";
  }

  function actionFormatter(cell) {
    const v = cell.getValue();
    const nameA = escapeHtml((state.meta && state.meta.graph_a) || "A");
    const nameB = escapeHtml((state.meta && state.meta.graph_b) || "B");
    if (v === "+") return `<span class="badge added" title="Triple present in ${nameB} but not in ${nameA}">+</span>`;
    if (v === "-") return `<span class="badge deleted" title="Triple present in ${nameA} but not in ${nameB}">−</span>`;
    return '<span class="badge common" title="Present in both">=</span>';
  }

  function iriFormatter(cell) {
    return renderIri(cell.getValue());
  }
  function objectFormatter(cell) {
    const o = cell.getValue();
    if (window.MapWidget && window.MapWidget.isWkt(o)) {
      const el = cell.getElement();
      el.classList.add("wkt-cell");
      if (state.wktSelection.has(o.v)) {
        el.classList.add("wkt-cell--active");
        el.title = "Click to remove from map";
      } else {
        el.classList.remove("wkt-cell--active");
        el.title = "Click to add to map";
      }
    }
    return renderObject(o);
  }

  function objectSorter(a, b) {
    const av = a && a.v ? a.v : "";
    const bv = b && b.v ? b.v : "";
    return av.localeCompare(bv);
  }

  function iriFilter(headerValue, rowValue) {
    if (!headerValue) return true;
    if (!rowValue) return false;
    const lc = headerValue.toLowerCase();
    if (rowValue.toLowerCase().includes(lc)) return true;
    const short = shortenIri(rowValue);
    return short ? short.toLowerCase().includes(lc) : false;
  }

  function objectFilter(headerValue, _rowValue, rowData) {
    if (!headerValue) return true;
    const o = rowData.o;
    if (!o) return false;
    if (o.v.toLowerCase().includes(headerValue.toLowerCase())) return true;
    if (o.t === "iri") {
      const short = shortenIri(o.v);
      if (short && short.toLowerCase().includes(headerValue.toLowerCase())) return true;
    }
    return false;
  }

  function buildTable() {
    state.table = new Tabulator(els.table, {
      height: "100%",
      layout: "fitColumns",
      virtualDom: true,
      virtualDomBuffer: 600,
      placeholder: "No rows",
      initialSort: [
        { column: "o", dir: "asc" },
        { column: "p", dir: "asc" },
        { column: "s", dir: "asc" },
      ],
      rowFormatter: function (row) {
        row.getElement().classList.remove("row-added", "row-deleted", "row-common");
        row.getElement().classList.add(rowClass(row));
      },
      columns: [
        {
          title: "Action",
          field: "a",
          width: 90,
          headerFilter: "list",
          headerFilterParams: { values: { "": "All", "+": "+ Added", "-": "− Deleted", "=": "= Common" } },
          formatter: actionFormatter,
        },
        { title: "Subject", field: "s", headerFilter: "input", headerFilterFunc: iriFilter, formatter: iriFormatter },
        { title: "Predicate", field: "p", headerFilter: "input", headerFilterFunc: iriFilter, formatter: iriFormatter },
        {
          title: "Object",
          field: "o",
          headerFilter: "input",
          headerFilterFunc: objectFilter,
          sorter: objectSorter,
          formatter: objectFormatter,
        },
      ],
    });

    els.table.addEventListener("click", function (e) {
      const cellEl = e.target.closest(".tabulator-cell");
      if (!cellEl || !cellEl.classList.contains("wkt-cell")) return;
      const rowEl = cellEl.closest(".tabulator-row");
      if (!rowEl) return;
      try {
        const row = state.table.getRow(rowEl);
        if (!row) return;
        const o = row.getData().o;
        if (!window.MapWidget || !window.MapWidget.isWkt(o)) return;
        e.preventDefault();
        if (state.wktSelection.has(o.v)) {
          state.wktSelection.delete(o.v);
          cellEl.classList.remove("wkt-cell--active");
          cellEl.title = "Click to add to map";
        } else {
          state.wktSelection.add(o.v);
          cellEl.classList.add("wkt-cell--active");
          cellEl.title = "Click to remove from map";
        }
        window.MapWidget.showWkts([...state.wktSelection]);
      } catch (_) {}
    });
  }

  async function streamRows(url, action) {
    const resp = await fetch(url);
    if (!resp.ok) throw new Error(`${url} → ${resp.status}`);
    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    let rows = [];

    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf("\n")) !== -1) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line) continue;
        try {
          const row = JSON.parse(line);
          if (action) row.a = action;
          rows.push(row);
          if (rows.length % 10000 === 0) {
            els.overlayMsg.textContent = `Received ${rows.length.toLocaleString()} rows\u2026`;
          }
        } catch (e) {
          console.warn("bad ndjson line", e);
        }
      }
    }
    if (buf.trim()) {
      try {
        const row = JSON.parse(buf);
        if (action) row.a = action;
        rows.push(row);
      } catch (e) {
        // ignore
      }
    }
    return rows;
  }

  async function loadMeta() {
    const resp = await fetch(apiUrl("/api/meta"));
    if (!resp.ok) throw new Error((await resp.text()) || `/api/meta → ${resp.status}`);
    const meta = await resp.json();
    state.meta = meta;
    state.prefixes = (meta.prefixes || [])
      .slice()
      .sort((a, b) => b[1].length - a[1].length);
    return meta;
  }

  function renderMeta() {
    if (!state.meta || !state.meta.loaded) {
      els.meta.textContent = "no diff loaded";
      return;
    }
    const s = state.meta.stats || {};
    const nameA = escapeHtml(state.meta.graph_a || "");
    const nameB = escapeHtml(state.meta.graph_b || "");
    els.meta.innerHTML =
      `A=${nameA} (${s.a_total ?? "?"} triples)` +
      ` · B=${nameB} (${s.b_total ?? "?"})` +
      ` · <span class="count-added">+${s.b_only ?? 0}</span>` +
      ` <span class="count-removed">−${s.a_only ?? 0}</span>`;
    const legendAdded = document.getElementById("legend-added");
    const legendDeleted = document.getElementById("legend-deleted");
    const rawA = state.meta.graph_a || "A";
    const rawB = state.meta.graph_b || "B";
    if (legendAdded) legendAdded.title = `Triple present in ${rawB} but not in ${rawA}`;
    if (legendDeleted) legendDeleted.title = `Triple present in ${rawA} but not in ${rawB}`;
  }

  function fmt(n) {
    return n === null || n === undefined ? "–" : Number(n).toLocaleString();
  }

  function card(label, value, cls, title) {
    const t = title ? ` title="${escapeHtml(title)}"` : "";
    return `<div class="card ${cls || ""}"${t}><div class="card-value">${value}</div><div class="card-label">${label}</div></div>`;
  }

  // Stacked bar: green for added, red for removed, scaled against `max`.
  function changeBar(added, removed, max) {
    const pa = max ? (100 * added) / max : 0;
    const pr = max ? (100 * removed) / max : 0;
    return `<div class="bar"><span class="bar-added" style="width:${pa}%"></span><span class="bar-removed" style="width:${pr}%"></span></div>`;
  }

  function renderIriText(iri) {
    if (iri === null || iri === undefined) return '<em class="muted">(untyped)</em>';
    const short = shortenIri(iri);
    return `<span title="${escapeHtml(iri)}">${escapeHtml(short || `<${iri}>`)}</span>`;
  }

  // Untyped subjects are grouped under class `null`; "" stands for it in the DOM.
  function classKey(c) {
    return c === null || c === undefined ? "" : c;
  }

  function subjectInClass(data) {
    return state.classFilter.subjects.has(data.s);
  }

  async function toggleClassFilter(key) {
    if (state.classFilter && state.classFilter.key === key) {
      state.classFilter = null;
    } else {
      const url = apiUrl("/api/class-subjects", key ? { class: key } : {});
      const resp = await fetch(url);
      if (!resp.ok) throw new Error(`${url} → ${resp.status}`);
      state.classFilter = { key, subjects: new Set(await resp.json()) };
    }
    for (const tr of els.summaryClasses.querySelectorAll("tr[data-class]")) {
      tr.classList.toggle("active", !!state.classFilter && tr.dataset.class === state.classFilter.key);
    }
    applyViewFilter(els.tripleView.value);
  }

  function setColumnFilter(field, value) {
    if (!state.table) return;
    const current = state.table.getHeaderFilterValue(field);
    state.table.setHeaderFilterValue(field, current === value ? "" : value);
  }

  function renderSummary(sum) {
    if (!sum) {
      els.summary.classList.add("hidden");
      els.summaryToggle.classList.add("hidden");
      return;
    }
    const t = sum.totals;
    const sub = sum.subjects;
    const changed = t.added + t.removed;
    const base = t.a_total || 0;
    const pct = base ? ` (${((100 * changed) / base).toFixed(1)}% of A)` : "";
    els.summaryCards.innerHTML = [
      card("triples in A", fmt(t.a_total)),
      card("triples in B", fmt(t.b_total)),
      card("common", fmt(t.common), "common"),
      card("added", `+${fmt(t.added)}`, "added", "Triples present in B but not in A"),
      card("removed", `−${fmt(t.removed)}`, "removed", "Triples present in A but not in B"),
      card("changed", fmt(changed), "", `Added + removed${pct}`),
      card("subjects affected", fmt(sub.affected), "", `Out of ${fmt(sub.a_total)} subjects in A and ${fmt(sub.b_total)} in B`),
      card("new subjects", fmt(sub.added), "added", "Subjects that only occur in B"),
      card("removed subjects", fmt(sub.removed), "removed", "Subjects that only occur in A"),
      card("modified subjects", fmt(sub.modified), "", "Subjects present on both sides with changed triples"),
    ].join("");

    const predMax = Math.max(1, ...sum.predicates.map((p) => p.added + p.removed));
    els.summaryPredicates.innerHTML =
      `<thead><tr><th>Predicate</th><th class="num">+</th><th class="num">−</th><th class="num">=</th><th></th></tr></thead><tbody>` +
      sum.predicates
        .map(
          (p) =>
            `<tr class="clickable" data-filter-field="p" data-filter-value="${escapeHtml(p.predicate)}">` +
            `<td>${renderIriText(p.predicate)}</td>` +
            `<td class="num added">${fmt(p.added)}</td><td class="num removed">${fmt(p.removed)}</td>` +
            `<td class="num muted">${fmt(p.common)}</td>` +
            `<td class="bar-cell">${changeBar(p.added, p.removed, predMax)}</td></tr>`,
        )
        .join("") +
      "</tbody>";

    const classMax = Math.max(1, ...sum.classes.map((c) => c.triples_added + c.triples_removed));
    els.summaryClasses.innerHTML =
      `<thead><tr><th>Class</th><th class="num" title="New instances (rdf:type added)">new</th><th class="num" title="Removed instances (rdf:type removed)">gone</th><th class="num" title="Affected subjects of this class">subj.</th><th class="num" title="Added triples on subjects of this class">+</th><th class="num" title="Removed triples on subjects of this class">−</th><th></th></tr></thead><tbody>` +
      sum.classes
        .map(
          (c) =>
            `<tr class="clickable${state.classFilter && state.classFilter.key === classKey(c.class) ? " active" : ""}" data-class="${escapeHtml(classKey(c.class))}">` +
            `<td>${renderIriText(c.class)}</td>` +
            `<td class="num added">${fmt(c.instances_added)}</td><td class="num removed">${fmt(c.instances_removed)}</td>` +
            `<td class="num">${fmt(c.subjects_affected)}</td>` +
            `<td class="num added">${fmt(c.triples_added)}</td><td class="num removed">${fmt(c.triples_removed)}</td>` +
            `<td class="bar-cell">${changeBar(c.triples_added, c.triples_removed, classMax)}</td></tr>`,
        )
        .join("") +
      "</tbody>";

    const statusLabel = { added: "new", removed: "gone", modified: "modified" };
    els.summarySubjects.innerHTML =
      `<thead><tr><th>Subject</th><th>Status</th><th class="num">+</th><th class="num">−</th></tr></thead><tbody>` +
      sum.top_subjects
        .map(
          (s) =>
            `<tr class="clickable" data-filter-field="s" data-filter-value="${escapeHtml(s.subject)}">` +
            `<td>${renderIriText(s.subject)}</td>` +
            `<td>${s.status ? `<span class="status status-${s.status}">${statusLabel[s.status]}</span>` : "–"}</td>` +
            `<td class="num added">${fmt(s.added)}</td><td class="num removed">${fmt(s.removed)}</td></tr>`,
        )
        .join("") +
      "</tbody>";

    els.summaryToggle.classList.remove("hidden");
    setSummaryVisible(els.summaryToggle.getAttribute("aria-expanded") !== "false");
  }

  function setSummaryVisible(visible) {
    els.summary.classList.toggle("hidden", !visible);
    els.summaryToggle.setAttribute("aria-expanded", visible ? "true" : "false");
    els.summaryToggle.classList.toggle("active", visible);
    if (state.table) state.table.redraw();
  }

  async function loadSummary() {
    const resp = await fetch(apiUrl("/api/summary"));
    if (!resp.ok) {
      renderSummary(null);
      return;
    }
    renderSummary(await resp.json());
  }

  async function loadDiffRows() {
    if (state.diffLoaded) return;
    showLoading("Loading diff rows\u2026");
    try {
      const rows = await streamRows(apiUrl("/api/rows", { include: "diff" }), null);
      if (rows.length > 0) {
        els.overlayMsg.textContent = `Rendering ${rows.length.toLocaleString()} rows\u2026`;
        await new Promise(r => setTimeout(r, 0));
        if (state.commonLoaded) {
          await state.table.addData(rows);
        } else {
          await state.table.setData(rows);
        }
      }
      state.diffLoaded = true;
    } finally {
      hideLoading();
    }
  }

  async function loadCommonRows() {
    if (state.commonLoaded) return;
    showLoading("Loading common rows\u2026");
    try {
      const rows = await streamRows(apiUrl("/api/rows", { include: "common" }), "=");
      if (rows.length > 0) {
        els.overlayMsg.textContent = `Rendering ${rows.length.toLocaleString()} rows\u2026`;
        await new Promise(r => setTimeout(r, 0));
        if (state.diffLoaded) {
          await state.table.addData(rows);
        } else {
          await state.table.setData(rows);
        }
      }
      state.commonLoaded = true;
    } finally {
      hideLoading();
    }
  }

  function applyViewFilter(mode) {
    state.table.clearFilter(false);
    if (state.classFilter) state.table.addFilter(subjectInClass);
    if (mode === "only-diff") {
      state.table.addFilter("a", "!=", "=");
    } else if (mode === "only-common") {
      state.table.addFilter("a", "=", "=");
    } else if (mode === "only-added") {
      state.table.addFilter("a", "=", "+");
    } else if (mode === "only-removed") {
      state.table.addFilter("a", "=", "-");
    }
    // "diff-and-common" → no additional filter
  }

  async function applyViewMode(mode) {
    const needsDiff = mode !== "only-common";
    const needsCommon = mode === "diff-and-common" || mode === "only-common";
    els.tripleView.disabled = true;
    els.commonError.classList.add("hidden");
    try {
      if (needsDiff && !state.diffLoaded) await loadDiffRows();
      if (needsCommon && !state.commonLoaded) await loadCommonRows();
      applyViewFilter(mode);
    } catch (e) {
      console.error("Failed to load rows:", e);
      els.commonError.textContent = e.message;
      els.commonError.classList.remove("hidden");
      els.tripleView.value = "only-diff";
      applyViewFilter("only-diff");
    } finally {
      els.tripleView.disabled = false;
    }
  }

  function setCommonOptionsDisabled(disabled) {
    for (const opt of els.tripleView.options) {
      if (opt.value === "diff-and-common" || opt.value === "only-common") {
        opt.disabled = disabled;
      }
    }
    if (disabled && (els.tripleView.value === "diff-and-common" || els.tripleView.value === "only-common")) {
      els.tripleView.value = "only-diff";
    }
  }

  // When the server runs with --data-dir, offer the files found there as
  // suggestions in the loader inputs (paths are relative to that directory).
  async function loadFileList() {
    try {
      const resp = await fetch("/api/files");
      if (!resp.ok) return;
      const data = await resp.json();
      if (!data.enabled) return;
      const list = document.getElementById("rdf-files");
      list.replaceChildren(...data.files.map((f) => {
        const opt = document.createElement("option");
        opt.value = f;
        return opt;
      }));
      els.pathA.placeholder = "a.ttl (relative to the data directory)";
      els.pathB.placeholder = "b.ttl (relative to the data directory)";
      els.pathDiff.placeholder = "diff.trig (relative to the data directory)";
    } catch (e) {
      console.error("Failed to list files:", e);
    }
  }

  async function init() {
    buildTable();
    loadFileList();
    els.pathA.value = sourceParams.get("a") || "";
    els.pathB.value = sourceParams.get("b") || "";
    els.pathDiff.value = sourceParams.get("diff") || "";
    if (sourceParams.toString()) showLoading("Computing diff\u2026");
    let meta;
    try {
      meta = await loadMeta();
    } catch (e) {
      els.meta.textContent = "error: " + e.message;
      els.loader.classList.remove("hidden");
      return;
    } finally {
      hideLoading();
    }
    const versionEl = document.getElementById("version");
    if (versionEl && meta.version) versionEl.textContent = `v${meta.version}`;
    renderMeta();

    if (!meta.loaded) {
      renderSummary(null);
      els.empty.classList.remove("hidden");
      els.loader.classList.remove("hidden");
      return;
    }

    await loadSummary();

    if (meta.from_diff_file) {
      setCommonOptionsDisabled(true);
      els.tripleView.title = "Common triples unavailable when loading a diff file";
    }

    await applyViewMode(els.tripleView.value);
  }

  els.openLoad.addEventListener("click", () => els.loader.classList.toggle("hidden"));

  els.summaryToggle.addEventListener("click", () =>
    setSummaryVisible(els.summaryToggle.getAttribute("aria-expanded") === "false"),
  );

  els.summary.addEventListener("click", (e) => {
    const classRow = e.target.closest("tr[data-class]");
    if (classRow) {
      toggleClassFilter(classRow.dataset.class).catch((err) => console.error("class filter failed:", err));
      return;
    }
    const tr = e.target.closest("tr[data-filter-field]");
    if (!tr) return;
    setColumnFilter(tr.dataset.filterField, tr.dataset.filterValue);
  });

  // Loading navigates to the URL naming the new files, so the result can be
  // bookmarked or shared and the back button returns to the previous diff.
  els.doLoad.addEventListener("click", () => {
    const q = new URLSearchParams();
    const diff = els.pathDiff.value.trim();
    const a = els.pathA.value.trim();
    const b = els.pathB.value.trim();
    if (diff) {
      q.set("diff", diff);
    } else if (a && b) {
      q.set("a", a);
      q.set("b", b);
    } else {
      els.loaderMsg.textContent = "Enter both file paths, or a diff file.";
      return;
    }
    window.location.search = q.toString();
  });

  els.tripleView.addEventListener("change", () => {
    applyViewMode(els.tripleView.value);
  });

  init().catch((e) => {
    els.meta.textContent = "init error: " + e.message;
  });
})();
