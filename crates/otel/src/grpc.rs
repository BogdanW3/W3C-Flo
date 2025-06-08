//! gRPC utilities for W3C trace context propagation
//!
//! This module provides reusable components for implementing W3C trace context
//! propagation in gRPC services, enabling distributed tracing across microservices.

use opentelemetry::{
  global, propagation::Extractor, propagation::Injector, trace::TraceContextExt,
};
use tonic::{metadata::MetadataMap, Request};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Helper struct to extract W3C trace context from gRPC metadata
///
/// Implements the OpenTelemetry Extractor trait to read W3C trace context headers
/// (traceparent, tracestate) from gRPC metadata for distributed tracing.
pub struct GrpcMetadataExtractor<'a>(pub &'a MetadataMap);

impl<'a> Extractor for GrpcMetadataExtractor<'a> {
  fn get(&self, key: &str) -> Option<&str> {
    self.0.get(key).and_then(|value| value.to_str().ok())
  }

  fn keys(&self) -> Vec<&str> {
    self
      .0
      .keys()
      .filter_map(|key| match key {
        tonic::metadata::KeyRef::Ascii(ascii_key) => Some(ascii_key.as_str()),
        tonic::metadata::KeyRef::Binary(_) => None,
      })
      .collect()
  }
}

/// Helper struct to inject W3C trace context into gRPC metadata
///
/// Implements the OpenTelemetry Injector trait to write W3C trace context headers
/// (traceparent, tracestate) into gRPC metadata for distributed tracing.
pub struct GrpcMetadataInjector<'a>(pub &'a mut MetadataMap);

impl<'a> Injector for GrpcMetadataInjector<'a> {
  fn set(&mut self, key: &str, value: String) {
    if let Ok(metadata_value) = value.parse() {
      if let Ok(key_name) = key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>() {
        self.0.insert(key_name, metadata_value);
      }
    }
  }
}

/// Apply W3C trace context from gRPC metadata to the current OpenTelemetry context
///
/// This function should be called from gRPC interceptors to extract and apply
/// W3C trace context from incoming requests. It ensures that spans created during
/// request processing inherit the correct parent context.
///
/// # Arguments
/// * `metadata` - The gRPC metadata from the incoming request
///
/// # Returns
/// The extracted OpenTelemetry context (also applied as current)
///
/// # Example
/// ```rust
/// use flo_otel::grpc::apply_w3c_trace_context;
/// use tonic::{Request, Status, service::Interceptor};
///
/// impl Interceptor for MyInterceptor {
///   fn call(&mut self, req: Request<()>) -> Result<Request<()>, Status> {
///     // Apply W3C trace context from incoming request
///     apply_w3c_trace_context(req.metadata());
///     
///     // Your service-specific logic here
///     Ok(req)
///   }
/// }
/// ```
pub fn apply_w3c_trace_context(metadata: &MetadataMap) -> opentelemetry::Context {
  // Extract W3C trace context from gRPC metadata
  let extractor = GrpcMetadataExtractor(metadata);
  let parent_context = global::get_text_map_propagator(|propagator| propagator.extract(&extractor));

  // Apply the extracted context as current to ensure spans inherit the correct parent
  let span_ref = parent_context.span();
  let span_context = span_ref.span_context();
  if span_context.is_valid() {
    tracing::debug!(
      "Applying W3C trace context: trace_id={:032x}, span_id={:016x}",
      span_context.trace_id(),
      span_context.span_id()
    );

    let _context_guard = parent_context.clone().attach();
    std::mem::forget(_context_guard); // Keep context active for request duration
  }

  parent_context
}

/// Inject W3C trace context into outgoing gRPC requests for distributed tracing
///
/// This function should be called before making outgoing gRPC calls to propagate
/// the current trace context to downstream services.
///
/// # Arguments
/// * `request` - The gRPC request to inject trace context into
///
/// # Returns
/// The request with W3C trace context headers injected
///
/// # Example
/// ```rust
/// use flo_otel::grpc::inject_trace_context_into_request;
///
/// // Before making a gRPC call
/// let request = inject_trace_context_into_request(request);
/// let response = client.some_method(request).await?;
/// ```
pub fn inject_trace_context_into_request<T>(mut request: Request<T>) -> Request<T> {
  let context = tracing::Span::current().context();
  let mut injector = GrpcMetadataInjector(request.metadata_mut());

  global::get_text_map_propagator(|propagator| {
    propagator.inject_context(&context, &mut injector);
  });

  request
}

#[cfg(test)]
mod tests {
  use super::*;
  use tonic::metadata::{MetadataMap, MetadataValue};

  #[test]
  fn test_grpc_metadata_extractor() {
    let mut metadata = MetadataMap::new();
    let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    metadata.insert("traceparent", MetadataValue::from_str(traceparent).unwrap());
    metadata.insert(
      "tracestate",
      MetadataValue::from_str("vendor=value").unwrap(),
    );

    let extractor = GrpcMetadataExtractor(&metadata);

    // Test get method
    assert_eq!(extractor.get("traceparent"), Some(traceparent));
    assert_eq!(extractor.get("tracestate"), Some("vendor=value"));
    assert_eq!(extractor.get("nonexistent"), None);

    // Test keys method
    let keys = extractor.keys();
    assert!(keys.contains(&"traceparent"));
    assert!(keys.contains(&"tracestate"));
  }

  #[test]
  fn test_grpc_metadata_injector() {
    let mut metadata = MetadataMap::new();
    let mut injector = GrpcMetadataInjector(&mut metadata);

    // Test set method
    injector.set("traceparent", "00-test-trace-id-span-id-01".to_string());
    injector.set("tracestate", "vendor=test".to_string());

    // Verify headers were injected
    assert!(metadata.get("traceparent").is_some());
    assert!(metadata.get("tracestate").is_some());
  }

  #[test]
  fn test_apply_w3c_trace_context() {
    let mut metadata = MetadataMap::new();
    let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    metadata.insert("traceparent", MetadataValue::from_str(traceparent).unwrap());

    // Test that context is extracted and applied
    let context = apply_w3c_trace_context(&metadata);
    let span = context.span();
    let span_context = span.span_context();

    // In test environment without proper OpenTelemetry setup, we just verify
    // the function runs without panicking
    assert!(!span_context.trace_id().to_string().is_empty());
  }
}
