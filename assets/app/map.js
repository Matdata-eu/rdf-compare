// Map widget for gsp:wktLiteral cells.
(function () {
  let map = null;
  let layer = null;
  const WKT_DATATYPE = "http://www.opengis.net/ont/geosparql#wktLiteral";

  function ensureMap() {
    const el = document.getElementById("map");
    el.classList.remove("hidden");
    if (map) return map;
    map = L.map(el).setView([0, 0], 2);
    L.tileLayer("https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png", {
      maxZoom: 19,
      attribution: "© OpenStreetMap",
    }).addTo(map);
    return map;
  }

  function isWkt(cellData) {
    if (!cellData || cellData.t !== "lit") return false;
    if (!cellData.dt) return false;
    return cellData.dt === WKT_DATATYPE;
  }

  function stripCrs(s) {
    // optional `<crs> ` prefix per GeoSPARQL.
    return s.replace(/^<[^>]*>\s*/, "").trim();
  }

  function showWkts(values) {
    const el = document.getElementById("map");
    if (layer) {
      layer.remove();
      layer = null;
    }
    if (!values.length) {
      el.classList.add("hidden");
      return;
    }
    const m = ensureMap();
    const features = [];
    for (const v of values) {
      try {
        const geom = wellknown.parse(stripCrs(v));
        if (geom) features.push({ type: "Feature", geometry: geom, properties: {} });
      } catch (e) {
        // ignore malformed wkt
      }
    }
    if (!features.length) return;
    layer = L.geoJSON({ type: "FeatureCollection", features });
    layer.addTo(m);
    try {
      m.fitBounds(layer.getBounds(), { padding: [20, 20], maxZoom: 16 });
    } catch (e) {
      // single point or empty bounds
    }
  }

  window.MapWidget = { isWkt, showWkts };
})();
