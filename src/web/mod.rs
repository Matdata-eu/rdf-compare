//! Local web viewer for rdf-compare.

pub mod api;
pub mod assets;

use crate::cli::InputFormat;
use crate::diff::{DiffResult, compute_diff, load_diff_file};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared application state.
#[derive(Clone, Default)]
pub struct AppState {
    pub data: Arc<Mutex<Option<Arc<DiffResult>>>>,
    /// When set, `/api/load` only accepts paths inside this (canonical)
    /// directory and `/api/files` lists the RDF files found under it.
    pub data_dir: Option<Arc<PathBuf>>,
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
    },
    Diff {
        diff: PathBuf,
        format: Option<InputFormat>,
        graph_a: Option<String>,
        graph_b: Option<String>,
    },
    Loaded(DiffResult),
}

pub async fn build_state(preload: Preload) -> Result<AppState> {
    let state = AppState::default();
    match preload {
        Preload::None => {}
        Preload::Loaded(mut d) => {
            d.sort_rows();
            *state.data.lock().await = Some(Arc::new(d));
        }
        Preload::Files {
            file_a,
            file_b,
            format_a,
            format_b,
            graph_a,
            graph_b,
            ignore_blank_nodes,
        } => {
            let inputs = crate::diff::DiffInputs {
                file_a,
                file_b,
                format_a,
                format_b,
                graph_a,
                graph_b,
                ignore_blank_nodes,
            };
            let mut result = tokio::task::spawn_blocking(move || compute_diff(&inputs))
                .await
                .context("diff task panicked")??;
            result.sort_rows();
            *state.data.lock().await = Some(Arc::new(result));
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
            *state.data.lock().await = Some(Arc::new(result));
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
pub fn run_blocking(
    bind: &str,
    open: bool,
    data_dir: Option<PathBuf>,
    preload: Preload,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    rt.block_on(async move {
        let mut state = build_state(preload).await?;
        if let Some(dir) = data_dir {
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
