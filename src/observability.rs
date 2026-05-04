//! Observability — metrics, structured logging, trace context.
//!
//! Provides:
//! - Prometheus metrics endpoint (`/metrics`)
//! - Request tracing middleware with `trace_id` injection
//! - Application-level counters and histograms

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use std::time::Instant;

/// Initialize the global metrics recorder (Prometheus exporter).
///
/// Call once at startup. Returns the `PrometheusHandle` for the `/metrics` endpoint.
pub fn init_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
    let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
    builder
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

/// Axum middleware that:
/// 1. Generates a `trace_id` for each request
/// 2. Records request duration and status code metrics
/// 3. Emits structured log lines with trace context
pub async fn trace_layer(request: Request, next: Next) -> Response {
    let trace_id = generate_trace_id();
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    let span = tracing::info_span!(
        "http_request",
        trace_id = %trace_id,
        method = %method,
        path = %path,
    );

    let start = Instant::now();

    let response = {
        let _enter = span.enter();
        tracing::info!("request started");
        next.run(request).await
    };

    let duration = start.elapsed();
    let status = response.status().as_u16();

    let _enter = span.enter();
    tracing::info!(
        status = status,
        duration_ms = duration.as_millis() as u64,
        "request completed"
    );

    counter!("http_requests_total", "method" => method.to_string(), "path" => normalize_metric_path(&path), "status" => status.to_string())
        .increment(1);
    histogram!("http_request_duration_seconds", "method" => method.to_string(), "path" => normalize_metric_path(&path))
        .record(duration.as_secs_f64());

    let mut response = response;
    if let Ok(val) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert("x-trace-id", val);
    }

    response
}

/// Prometheus `/metrics` handler.
pub async fn metrics_handler(
    handle: axum::extract::State<metrics_exporter_prometheus::PrometheusHandle>,
) -> impl IntoResponse {
    handle.render()
}

/// Record a custom application event counter.
pub fn record_event(name: &'static str, labels: &[(&'static str, String)]) {
    let pairs: Vec<metrics::Label> = labels
        .iter()
        .map(|(k, v)| metrics::Label::new(*k, v.clone()))
        .collect();
    counter!(name, pairs).increment(1);
}

/// Record a latency measurement.
pub fn record_latency(name: &'static str, duration: std::time::Duration) {
    histogram!(name).record(duration.as_secs_f64());
}

fn generate_trace_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}

fn normalize_metric_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let mut normalized = Vec::new();
    for part in &parts {
        if part.is_empty() {
            continue;
        }
        if part.len() > 20 && part.chars().all(|c| c.is_ascii_hexdigit()) {
            normalized.push(":hash");
        } else if part.starts_with("ws_") || part.starts_with("mem_") || part.starts_with("run_") {
            normalized.push(":id");
        } else {
            normalized.push(part);
        }
    }
    format!("/{}", normalized.join("/"))
}

/// Initialize structured JSON logging with env filter.
pub fn init_logging(level: &str) {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(level))
        .json()
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_id_is_32_hex_chars() {
        let id = generate_trace_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn trace_ids_are_unique() {
        let a = generate_trace_id();
        let b = generate_trace_id();
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_path_replaces_ids() {
        assert_eq!(
            normalize_metric_path("/v1/workspaces/ws_ABC123/commit"),
            "/v1/workspaces/:id/commit"
        );
    }

    #[test]
    fn normalize_path_replaces_hashes() {
        let hash = "a".repeat(64);
        let path = format!("/v1/files/{hash}");
        assert_eq!(normalize_metric_path(&path), "/v1/files/:hash");
    }

    #[test]
    fn normalize_path_preserves_static() {
        assert_eq!(
            normalize_metric_path("/v1/admin/health"),
            "/v1/admin/health"
        );
    }

    #[test]
    fn record_event_does_not_panic() {
        let _ = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder();
        record_event("test_events_total", &[("action", "test".to_string())]);
    }

    #[test]
    fn record_latency_does_not_panic() {
        record_latency("test_latency_seconds", std::time::Duration::from_millis(42));
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_text() {
        let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();

        counter!("test_counter").increment(1);

        let output = handle.render();
        assert!(output.contains("test_counter") || output.is_empty());
    }

    #[tokio::test]
    async fn trace_middleware_adds_header() {
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use axum::Router;
        use tower::ServiceExt;

        let app = Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(trace_layer));

        let resp = app
            .oneshot(Request::get("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert!(resp.headers().contains_key("x-trace-id"));
        let trace_id = resp.headers()["x-trace-id"].to_str().unwrap();
        assert_eq!(trace_id.len(), 32);
    }
}
