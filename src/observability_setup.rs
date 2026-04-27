//! Local observability bootstrap.
//!
//! Phase 3 / T5. Composes the same FMT + RELOAD + RING + OTel stack that
//! `observability::init_node` builds upstream, plus the WEB layer (PR #640)
//! that upstream `init_node` does not yet wire. The WEB layer's
//! [`WebHandle`] is what `/api/logs/stream` subscribes to.
//!
//! When upstream `init_node` learns to compose WEB, this module collapses
//! to a `pub use observability::init_node` re-export and `NodeObsGuard`
//! becomes a thin wrapper.
//!
//! ## Single-init invariant
//!
//! `tracing::subscriber::set_global_default` is the actual one-shot gate;
//! a second call returns `SetGlobalDefaultError`. We surface that as
//! [`SetupError::AlreadyInstalled`] so callers can distinguish "you tried
//! to init twice in this process" from any other plumbing failure.

use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, io};

use observability::layers::fmt::{build_fmt_layer, FmtGuard, FmtTarget};
use observability::layers::reload::{build_reload_layer, ReloadHandle};
use observability::layers::ring::{build_ring_layer, RingHandle, OBS_RING_CAPACITY};
use observability::layers::web::{build_web_layer, WebHandle, OBS_WEB_CAPACITY};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
use tracing_log::LogTracer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Registry};

const OBS_FILE_PATH_ENV: &str = "OBS_FILE_PATH";

/// Errors raised by [`init_node_with_web`].
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// `tracing::subscriber::set_global_default` rejected our subscriber
    /// because one was already installed in this process.
    #[error("tracing subscriber already installed for this process")]
    AlreadyInstalled,
    /// Could not open the configured FMT sink (log file).
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// `build_fmt_writer` returned an `ObsError` other than IO.
    #[error("observability: {0}")]
    Obs(#[from] observability::ObsError),
}

/// RAII guard returned by [`init_node_with_web`].
///
/// Holds the FMT worker, RING / RELOAD / WEB handles, and is dropped when
/// the binary exits. Same lifetime contract as `observability::ObsGuard`:
/// dropping mid-process stops the FMT flush thread and may lose buffered
/// log lines.
#[must_use = "NodeObsGuard must be held for the lifetime of the binary"]
pub struct NodeObsGuard {
    _fmt_guard: FmtGuard,
    ring: RingHandle,
    reload: Arc<ReloadHandle>,
    web: WebHandle,
}

impl NodeObsGuard {
    pub fn ring(&self) -> &RingHandle {
        &self.ring
    }
    pub fn reload(&self) -> Arc<ReloadHandle> {
        Arc::clone(&self.reload)
    }
    pub fn web(&self) -> &WebHandle {
        &self.web
    }

    /// Cheap clones of every handle, ready to wrap in `web::Data`.
    pub fn handles(&self) -> ObsHandles {
        ObsHandles {
            ring: self.ring.clone(),
            web: self.web.clone(),
            reload: Arc::clone(&self.reload),
        }
    }
}

/// The handles `/api/logs*` endpoints subscribe to. `ReloadHandle` is
/// not `Clone` upstream, so we share it through `Arc`. `RingHandle` and
/// `WebHandle` are already cheap-clone (`Arc` internally).
#[derive(Clone)]
pub struct ObsHandles {
    pub ring: RingHandle,
    pub web: WebHandle,
    pub reload: Arc<ReloadHandle>,
}

/// Build and install a tracing subscriber matching `observability::init_node`
/// plus the WEB broadcast layer. Returns a guard that owns the FMT worker
/// and exposes RING / RELOAD / WEB handles for the HTTP server to wire
/// into Actix `web::Data`.
///
/// Mirrors upstream `init_node`'s layer composition and globals exactly so
/// this can collapse to a `pub use` once upstream wires WEB. The WEB layer
/// is appended after RING; both lift trace_id / span_id off the parent
/// span's `OtelData` extension independently — order between them is
/// irrelevant because OTel's `on_new_span` runs before either.
pub fn init_node_with_web(service_name: &'static str) -> Result<NodeObsGuard, SetupError> {
    assert!(!service_name.is_empty(), "service_name required");

    let path = default_node_log_path()?;
    let (reload_layer, reload) = build_reload_layer::<Registry>(default_env_filter());
    let (ring_layer, ring) = build_ring_layer(OBS_RING_CAPACITY);
    let (web_layer, web) = build_web_layer(OBS_WEB_CAPACITY);

    let tracer_provider = SdkTracerProvider::builder().build();
    let tracer = tracer_provider.tracer(service_name);
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // FMT goes last so `build_fmt_layer<S>` infers `S` against the full
    // composed subscriber type at this exact spot — `impl Layer<S>` is
    // existential, so it can't be threaded through a `Layered<...>`
    // sandwich. Upstream `init_node` works around this by inlining
    // `tracing_subscriber::fmt::layer()` and using the private
    // `build_fmt_writer` + `RedactingFormat`. Until those become public,
    // putting FMT outermost is the equivalent that compiles. RING and
    // WEB still see every event because all five layers run on every
    // emitted event regardless of `with()` order.
    let subscriber = Registry::default()
        .with(reload_layer)
        .with(otel_layer)
        .with(ring_layer)
        .with(web_layer);
    let (fmt_layer, fmt_guard) = build_fmt_layer(FmtTarget::File(path))?;
    let subscriber = subscriber.with(fmt_layer);

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| SetupError::AlreadyInstalled)?;
    install_globals();

    Ok(NodeObsGuard {
        _fmt_guard: fmt_guard,
        ring,
        reload: Arc::new(reload),
        web,
    })
}

fn default_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

fn default_node_log_path() -> Result<PathBuf, SetupError> {
    if let Ok(p) = std::env::var(OBS_FILE_PATH_ENV) {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME").map_err(|_| {
        SetupError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME not set; set OBS_FILE_PATH to choose a log path explicitly",
        ))
    })?;
    let mut dir = PathBuf::from(home);
    dir.push(".folddb");
    fs::create_dir_all(&dir)?;
    dir.push("observability.jsonl");
    Ok(dir)
}

fn install_globals() {
    global::set_text_map_propagator(TraceContextPropagator::new());
    let _ = LogTracer::init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_env_filter_does_not_panic() {
        let _ = default_env_filter();
    }

    #[test]
    fn default_node_log_path_honours_obs_file_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("custom.jsonl");
        let prev = std::env::var(OBS_FILE_PATH_ENV).ok();
        std::env::set_var(OBS_FILE_PATH_ENV, &target);

        let resolved = default_node_log_path().expect("path resolves");

        match prev {
            Some(v) => std::env::set_var(OBS_FILE_PATH_ENV, v),
            None => std::env::remove_var(OBS_FILE_PATH_ENV),
        }

        assert_eq!(resolved, target);
    }
}
