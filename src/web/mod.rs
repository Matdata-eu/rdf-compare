//! Local web viewer for rdf-compare.

pub mod api;
pub mod assets;
pub mod cache;

use crate::cli::InputFormat;
use crate::diff::{DiffResult, compute_diff, load_diff_file};
use anyhow::{Context, Result};
use cache::DiffCache;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// Default number of diffs kept in memory by the viewer.
pub const DEFAULT_CACHE_SIZE: usize = 4;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Diff preloaded from the command line, shown when the page URL names
    /// no files.
    pub default: Option<Arc<DiffResult>>,
    /// Diffs requested through the page URL (`?a=…&b=…` or `?diff=…`).
    pub cache: Arc<DiffCache>,
    /// When set, URL paths resolve relative to this (canonical) directory,
    /// paths outside it are rejected, and `/api/files` lists the RDF files
    /// found under it.
    pub data_dir: Option<Arc<PathBuf>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            default: None,
            cache: Arc::new(DiffCache::new(DEFAULT_CACHE_SIZE)),
            data_dir: None,
        }
    }
}

/// Server options that do not depend on what is preloaded.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub data_dir: Option<PathBuf>,
    pub cache_size: usize,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            cache_size: DEFAULT_CACHE_SIZE,
        }
    }
}

/// Server lifecycle wrapper.
pub struct Server {
    pub addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl Server {
    pub async fn join(self) -> Result<()> {
        self.handle.await.context("web server task panicked")?;
        Ok(())
    }
}

/// Spec describing what the viewer should preload before it starts serving.
pub enum Preload {
    None,
    Files {
        file_a: PathBuf,
        file_b: PathBuf,
        format_a: Option<InputFormat>,
        format_b: Option<InputFormat>,
        graph_a: Option<String>,
        graph_b: Option<String>,
        ignore_blank_nodes: bool,
        normalization: crate::normalize::Normalization,
    },
    Diff {
        diff: PathBuf,
        format: Option<InputFormat>,
        graph_a: Option<String>,
        graph_b: Option<String>,
    },
    Loaded(Box<DiffResult>),
}

pub async fn build_state(preload: Preload) -> Result<AppState> {
    let mut state = AppState::default();
    match preload {
        Preload::None => {}
        Preload::Loaded(mut d) => {
            d.sort_rows();
            state.default = Some(Arc::new(*d));
        }
        Preload::Files {
            file_a,
            file_b,
            format_a,
            format_b,
            graph_a,
            graph_b,
            ignore_blank_nodes,
            normalization,
        } => {
            let inputs = crate::diff::DiffInputs {
                file_a,
                file_b,
                format_a,
                format_b,
                graph_a,
                graph_b,
                ignore_blank_nodes,
                normalization,
            };
            let mut result = tokio::task::spawn_blocking(move || compute_diff(&inputs))
                .await
                .context("diff task panicked")??;
            result.sort_rows();
            state.default = Some(Arc::new(result));
        }
        Preload::Diff {
            diff,
            format,
            graph_a,
            graph_b,
        } => {
            let inputs = crate::diff::LoadDiffInputs {
                diff,
                format,
                graph_a,
                graph_b,
            };
            let mut result = tokio::task::spawn_blocking(move || load_diff_file(&inputs))
                .await
                .context("load task panicked")??;
            result.sort_rows();
            state.default = Some(Arc::new(result));
        }
    }
    Ok(state)
}

/// Start the HTTP server. Returns once the listener is bound.
pub async fn start(bind: &str, state: AppState) -> Result<Server> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;
    let addr = listener.local_addr().context("local_addr")?;
    let app = api::router(state);
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("web server error: {e}");
        }
    });
    Ok(Server { addr, handle })
}

/// Synchronous helper used by `main.rs`. Builds a current-thread Tokio runtime,
/// starts the server, optionally opens the browser, and blocks until ctrl-c.
pub fn run_blocking(bind: &str, open: bool, config: ServeConfig, preload: Preload) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    rt.block_on(async move {
        let mut state = build_state(preload).await?;
        state.cache = Arc::new(DiffCache::new(config.cache_size));
        if let Some(dir) = config.data_dir {
            let dir = dir
                .canonicalize()
                .with_context(|| format!("data directory {} not found", dir.display()))?;
            eprintln!("serving RDF files from {}", dir.display());
            state.data_dir = Some(Arc::new(dir));
        }
        let server = start(bind, state).await?;
        let url = format!("http://{}/", server.addr);
        eprintln!("rdf-compare viewer listening on {url}");
        if open && let Err(e) = webbrowser::open(&url) {
            eprintln!("could not open browser: {e}");
        }
        // wait for ctrl-c / SIGTERM or server task end
        tokio::select! {
            r = shutdown_signal() => {
                r?;
                eprintln!("shutting down");
                Ok::<(), anyhow::Error>(())
            }
            _ = server.handle => Ok(()),
        }
    })
}

/// Resolves on ctrl-c, or on SIGTERM on unix (sent by `docker stop` and
/// Kubernetes when a pod is terminated).
async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).context("SIGTERM handler failed")?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r.context("ctrl-c handler failed"),
            _ = term.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("ctrl-c handler failed")
    }
}
