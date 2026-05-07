//! Embedded static assets for the web viewer.

pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! txt {
    ($p:literal, $mime:literal, $file:literal) => {
        Asset {
            path: $p,
            mime: $mime,
            bytes: include_bytes!(concat!("../../", $file)),
        }
    };
}

pub static ASSETS: &[Asset] = &[
    txt!(
        "/assets/app/index.html",
        "text/html; charset=utf-8",
        "assets/app/index.html"
    ),
    txt!(
        "/assets/app/app.js",
        "application/javascript",
        "assets/app/app.js"
    ),
    txt!(
        "/assets/app/map.js",
        "application/javascript",
        "assets/app/map.js"
    ),
    txt!(
        "/assets/app/styles.css",
        "text/css; charset=utf-8",
        "assets/app/styles.css"
    ),
    txt!(
        "/assets/vendor/tabulator/tabulator.min.js",
        "application/javascript",
        "assets/vendor/tabulator/tabulator.min.js"
    ),
    txt!(
        "/assets/vendor/tabulator/tabulator.min.css",
        "text/css; charset=utf-8",
        "assets/vendor/tabulator/tabulator.min.css"
    ),
    txt!(
        "/assets/vendor/leaflet/leaflet.js",
        "application/javascript",
        "assets/vendor/leaflet/leaflet.js"
    ),
    txt!(
        "/assets/vendor/leaflet/leaflet.css",
        "text/css; charset=utf-8",
        "assets/vendor/leaflet/leaflet.css"
    ),
    txt!(
        "/assets/vendor/leaflet/images/marker-icon.png",
        "image/png",
        "assets/vendor/leaflet/images/marker-icon.png"
    ),
    txt!(
        "/assets/vendor/leaflet/images/marker-icon-2x.png",
        "image/png",
        "assets/vendor/leaflet/images/marker-icon-2x.png"
    ),
    txt!(
        "/assets/vendor/leaflet/images/marker-shadow.png",
        "image/png",
        "assets/vendor/leaflet/images/marker-shadow.png"
    ),
    txt!(
        "/assets/vendor/leaflet/images/layers.png",
        "image/png",
        "assets/vendor/leaflet/images/layers.png"
    ),
    txt!(
        "/assets/vendor/leaflet/images/layers-2x.png",
        "image/png",
        "assets/vendor/leaflet/images/layers-2x.png"
    ),
    txt!(
        "/assets/vendor/wellknown/wellknown.js",
        "application/javascript",
        "assets/vendor/wellknown/wellknown.js"
    ),
];

pub fn lookup(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.path == path)
}
