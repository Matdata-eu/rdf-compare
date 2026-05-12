// rdf-compare web viewer — Tabulator-driven diff browser.
(function () {
  const state = {
    prefixes: [], // sorted by length desc for longest-prefix-wins shortening
    table: null,
    meta: null,
    diffLoaded: false,
    commonShown: false,
    wktSelection: new Set(), // wkt literal strings currently shown on the map
  };

  const els = {
    meta: document.getElementById("meta"),
    table: document.getElementById("table"),
    empty: document.getElementById("empty-state"),
    loader: document.getElementById("loader"),
    showCommon: document.getElementById("show-common"),
    openLoad: document.getElementById("open-load"),
    doLoad: document.getElementById("do-load"),
    pathA: document.getElementById("path-a"),
    pathB: document.getElementById("path-b"),
    pathDiff: document.getElementById("path-diff"),
    loaderMsg: document.getElementById("loader-msg"),
    overlay: document.getElementById("loading-overlay"),
    overlayMsg: document.getElementById("loading-msg"),
    commonError: document.getElementById("common-error"),
  };

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
    if (v === "+") return '<span class="badge added" title="Added in B">+</span>';
    if (v === "-") return '<span class="badge deleted" title="Removed from A">−</span>';
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
    const resp = await fetch("/api/meta");
    if (!resp.ok) throw new Error("/api/meta failed");
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
    els.meta.textContent =
      `A=${state.meta.graph_a} (${s.a_total ?? "?"} triples)` +
      ` · B=${state.meta.graph_b} (${s.b_total ?? "?"})` +
      ` · +${s.b_only ?? 0} −${s.a_only ?? 0}`;
  }

  async function loadDiffRows() {
    if (state.diffLoaded) return;
    showLoading("Loading diff rows\u2026");
    try {
      const rows = await streamRows("/api/rows?include=diff", null);
      if (rows.length > 0) {
        els.overlayMsg.textContent = `Rendering ${rows.length.toLocaleString()} rows\u2026`;
        await new Promise(r => setTimeout(r, 0));
        await state.table.setData(rows);
      }
      state.diffLoaded = true;
    } finally {
      hideLoading();
    }
  }

  async function loadCommonRows() {
    showLoading("Loading common rows\u2026");
    try {
      const rows = await streamRows("/api/rows?include=common", "=");
      if (rows.length > 0) {
        els.overlayMsg.textContent = `Rendering ${rows.length.toLocaleString()} rows\u2026`;
        await new Promise(r => setTimeout(r, 0));
        await state.table.addData(rows);
      }
    } finally {
      hideLoading();
    }
  }

  async function init() {
    buildTable();
    const meta = await loadMeta();
    const versionEl = document.getElementById("version");
    if (versionEl && meta.version) versionEl.textContent = `v${meta.version}`;
    renderMeta();

    if (!meta.loaded) {
      els.empty.classList.remove("hidden");
      els.loader.classList.remove("hidden");
      return;
    }

    if (meta.from_diff_file) {
      els.showCommon.disabled = true;
      els.showCommon.parentElement.title = "Common triples unavailable when loading a diff file";
    }

    await loadDiffRows();
  }

  els.openLoad.addEventListener("click", () => els.loader.classList.toggle("hidden"));

  els.doLoad.addEventListener("click", async () => {
    const body = {};
    if (els.pathDiff.value.trim()) {
      body.diff = els.pathDiff.value.trim();
    } else {
      body.file_a = els.pathA.value.trim();
      body.file_b = els.pathB.value.trim();
    }
    els.loaderMsg.textContent = "Loading…";
    try {
      const r = await fetch("/api/load", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!r.ok) throw new Error(await r.text());
      els.loaderMsg.textContent = "Loaded.";
      state.diffLoaded = false;
      state.commonShown = false;
      els.showCommon.checked = false;
      state.table.clearData();
      const meta = await loadMeta();
      renderMeta();
      els.empty.classList.add("hidden");
      els.loader.classList.add("hidden");
      if (meta.from_diff_file) {
        els.showCommon.disabled = true;
      } else {
        els.showCommon.disabled = false;
      }
      await loadDiffRows();
    } catch (e) {
      els.loaderMsg.textContent = "Error: " + e.message;
    }
  });

  els.showCommon.addEventListener("change", async () => {
    if (els.showCommon.checked && !state.commonShown) {
      els.showCommon.disabled = true;
      els.commonError.classList.add("hidden");
      try {
        await loadCommonRows();
        state.commonShown = true;
      } catch (e) {
        console.error("Failed to load common rows:", e);
        els.showCommon.checked = false;
        els.commonError.textContent = e.message;
        els.commonError.classList.remove("hidden");
      } finally {
        els.showCommon.disabled = false;
      }
    } else if (!els.showCommon.checked && state.commonShown) {
      els.commonError.classList.add("hidden");
      // remove common rows in place
      const rows = state.table.getRows();
      for (const r of rows) {
        if (r.getData().a === "=") r.delete();
      }
      state.commonShown = false;
    }
  });

  init().catch((e) => {
    els.meta.textContent = "init error: " + e.message;
  });
})();
