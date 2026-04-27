//! W3C trace-context ingress middleware (Phase 2 / B1).
//!
//! For every incoming HTTP request, extract any `traceparent` /
//! `tracestate` headers and attach the resulting `opentelemetry::Context`
//! as the parent of the request's tracing root span. Combined with
//! `tracing_actix_web::TracingLogger`, this stitches every server span
//! into the upstream caller's distributed trace.
//!
//! ## Wiring
//!
//! Must be wrapped **after** `TracingLogger::default()` so the root span
//! exists in `req.extensions()` by the time we run:
//!
//! ```ignore
//! App::new()
//!     .wrap(W3CParentContext)         // inner — runs after root span set
//!     .wrap(TracingLogger::default()) // outer — creates root span first
//! ```
//!
//! In Actix, the *last* `.wrap` call is the *outermost* middleware on
//! the request path, so the order above gives us the desired
//! "TracingLogger → W3CParentContext → handler" pipeline.
//!
//! ## Propagator dependency
//!
//! The extraction is a no-op until a global text-map propagator is
//! installed (`opentelemetry::global::set_text_map_propagator`). The
//! `observability::init_node` family of helpers does this. Until that
//! call is wired into fold_db_node startup (separate Phase 1 follow-up),
//! every extracted context will be empty and parents will not be
//! attached at runtime — but the middleware test below installs a
//! propagator ad-hoc so the round-trip is validated in CI.

use std::future::{ready, Ready};
use std::rc::Rc;

use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error, HttpMessage};
use futures_util::future::LocalBoxFuture;
use tracing_actix_web::RootSpan;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub struct W3CParentContext;

impl<S, B> Transform<S, ServiceRequest> for W3CParentContext
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = W3CParentContextService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(W3CParentContextService {
            service: Rc::new(service),
        }))
    }
}

pub struct W3CParentContextService<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for W3CParentContextService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // actix-http 3 uses http 0.2 internally; observability::propagation
        // takes &http::HeaderMap from http 1.x. The two HeaderName/Value
        // types are not interchangeable, so we round-trip through bytes.
        // Only ASCII trace-context headers matter for W3C propagation,
        // and any value that isn't valid in 1.x is silently skipped —
        // an unparseable traceparent is just a missing parent.
        let mut headers = http::HeaderMap::with_capacity(req.headers().len());
        for (name, value) in req.headers() {
            if let (Ok(n), Ok(v)) = (
                http::HeaderName::from_bytes(name.as_str().as_bytes()),
                http::HeaderValue::from_bytes(value.as_bytes()),
            ) {
                headers.append(n, v);
            }
        }
        let parent = observability::propagation::extract_parent_context(&headers);

        if let Some(root_span) = req.extensions().get::<RootSpan>().cloned() {
            root_span.set_parent(parent);
        }

        let svc = self.service.clone();
        Box::pin(async move { svc.call(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, web, App, HttpResponse};
    use opentelemetry::global;
    use opentelemetry::trace::{TraceContextExt, TracerProvider};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
    use std::sync::{Mutex, Once};
    use tracing::Span;
    use tracing_actix_web::TracingLogger;
    use tracing_subscriber::layer::SubscriberExt;

    static INIT_TRACING: Once = Once::new();

    /// Install a global text-map propagator AND a tracing subscriber with
    /// a `tracing_opentelemetry::OpenTelemetryLayer` attached. Without
    /// the layer, spans have no `OtelData` and `set_parent` is a no-op
    /// from the otel side — `.context()` returns an invalid SpanContext
    /// even when the W3C header was extracted correctly.
    fn install_tracing() {
        INIT_TRACING.call_once(|| {
            global::set_text_map_propagator(TraceContextPropagator::new());
            let provider = SdkTracerProvider::builder().build();
            let tracer = provider.tracer("fold_db_node-test");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let subscriber = tracing_subscriber::registry().with(otel_layer);
            tracing::subscriber::set_global_default(subscriber)
                .expect("global tracing subscriber should not yet be set");
        });
    }

    #[actix_web::test]
    async fn extracts_traceparent_into_root_span_parent() {
        install_tracing();

        // Capture the root span's extracted parent trace_id from inside
        // the handler — the handler runs after both middlewares, so the
        // current span is the TracingLogger root span with our parent
        // already attached.
        let captured: web::Data<Mutex<Option<String>>> = web::Data::new(Mutex::new(None));

        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .wrap(W3CParentContext)
                .wrap(TracingLogger::default())
                .route(
                    "/",
                    web::get().to(|state: web::Data<Mutex<Option<String>>>| async move {
                        let parent = Span::current().context();
                        let trace_id = format!("{:032x}", parent.span().span_context().trace_id());
                        *state.lock().unwrap() = Some(trace_id);
                        HttpResponse::Ok().finish()
                    }),
                ),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/")
            .insert_header((
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            ))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success(), "handler should succeed");

        let trace_id = captured
            .lock()
            .unwrap()
            .clone()
            .expect("handler should have recorded the trace_id");
        assert_eq!(
            trace_id, "0af7651916cd43dd8448eb211c80319c",
            "incoming traceparent trace_id should propagate onto the handler's current span"
        );
    }

    #[actix_web::test]
    async fn missing_traceparent_starts_a_fresh_trace() {
        install_tracing();

        // No incoming traceparent → extract_parent_context returns an
        // empty Context → set_parent(empty) leaves the root span as a
        // brand-new trace root. We assert the span has a fresh, valid
        // trace_id that does NOT match the round-trip test's fixture.
        let captured: web::Data<Mutex<Option<String>>> = web::Data::new(Mutex::new(None));

        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .wrap(W3CParentContext)
                .wrap(TracingLogger::default())
                .route(
                    "/",
                    web::get().to(|state: web::Data<Mutex<Option<String>>>| async move {
                        let span_cx = Span::current().context();
                        let trace_id = format!("{:032x}", span_cx.span().span_context().trace_id());
                        *state.lock().unwrap() = Some(trace_id);
                        HttpResponse::Ok().finish()
                    }),
                ),
        )
        .await;

        let req = test::TestRequest::get().uri("/").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let trace_id = captured.lock().unwrap().clone().expect("handler ran");
        assert_ne!(
            trace_id, "0af7651916cd43dd8448eb211c80319c",
            "absent traceparent should not inherit the round-trip test's fixture trace_id"
        );
        assert_ne!(
            trace_id, "00000000000000000000000000000000",
            "OpenTelemetry layer should mint a fresh trace_id"
        );
    }
}
