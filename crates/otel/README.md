# Flo OpenTelemetry Library (`flo-otel`)

A reusable OpenTelemetry library, providing W3C trace context propagation utilities for gRPC services and centralized observability configuration.

## Features

### W3C Trace Context Propagation for gRPC

Provides ready-to-use utilities for implementing W3C trace context propagation in gRPC services, enabling distributed tracing across microservices.

#### Core Components

- **`GrpcMetadataExtractor`**: Extracts W3C trace context headers from gRPC metadata
- **`GrpcMetadataInjector`**: Injects W3C trace context headers into gRPC metadata  
- **`apply_w3c_trace_context()`**: Applies extracted context as current OpenTelemetry context
- **`inject_trace_context_into_request()`**: Injects current trace context into outgoing gRPC requests

#### Quick Start

1. **Add dependency** to your service's `Cargo.toml`:
```toml
[dependencies]
flo-otel = { path = "../otel" }
```

2. **Apply context in gRPC interceptor**:
```rust
use flo_otel::grpc::apply_w3c_trace_context;
use tonic::{Request, Status, service::Interceptor};

impl Interceptor for MyServiceInterceptor {
  fn call(&mut self, req: Request<()>) -> Result<Request<()>, Status> {
    // Apply W3C trace context from incoming request
    apply_w3c_trace_context(req.metadata());
    
    // Your service-specific logic here
    Ok(req)
  }
}
```

3. **Inject context in outgoing requests**:
```rust
use flo_otel::grpc::inject_trace_context_into_request;

// Before making a gRPC call to another service
let request = inject_trace_context_into_request(create_player_request);
let response = downstream_client.create_player(request).await?;
```

#### Advanced Usage

**Manual context extraction and injection**:
```rust
use flo_otel::grpc::{GrpcMetadataExtractor, GrpcMetadataInjector};
use opentelemetry::global;

// Extract context from metadata
let extractor = GrpcMetadataExtractor(request.metadata());
let context = global::get_text_map_propagator(|propagator| {
  propagator.extract(&extractor)
});

// Inject context into metadata
let mut injector = GrpcMetadataInjector(request.metadata_mut());
global::get_text_map_propagator(|propagator| {
  propagator.inject_context(&context, &mut injector);
});
```

### OpenTelemetry Configuration

The library provides centralized OpenTelemetry configuration with W3C trace context propagation enabled by default.

#### OtelBuilder Configuration

Use `OtelBuilder` to configure OpenTelemetry for your service:

```rust
use flo_otel::builder::OtelBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  // Basic configuration
  let _otel_guard = OtelBuilder::new()
    .project_name("my-service")  // Required: service name
    .build()
    .await;

  // Advanced configuration
  let _otel_guard = OtelBuilder::new()
    .project_name("my-service")
    .tracing_endpoint("http://jaeger:14250")  // Optional: OTLP traces endpoint
    .metrics_endpoint("http://prometheus:9090")  // Optional: OTLP metrics endpoint
    .add_attribute("version", "1.0.0")  // Optional: custom attributes
    .add_attribute("environment", "production")
    .build()
    .await;

  // Your service code here
  
  Ok(())
}
```

#### Configuration Options

- **`project_name(name)`** - Required. Sets the service name for telemetry
- **`tracing_endpoint(url)`** - Optional. OTLP endpoint for trace export (if not set, traces are disabled)
- **`metrics_endpoint(url)`** - Optional. OTLP endpoint for metrics export (if not set, metrics are disabled)
- **`add_attribute(key, value)`** - Optional. Add custom attributes to all telemetry data

#### Environment Variables

The builder also respects standard OpenTelemetry environment variables:
- `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` - Traces endpoint
- `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` - Metrics endpoint
- `OTEL_SERVICE_NAME` - Service name (overridden by `project_name()`)

**Important**: Keep the `_otel_guard` in scope for the lifetime of your application to ensure proper cleanup.

### Integration Steps

To add W3C trace context propagation to any gRPC service:

1. **Add dependency**: Add `flo-otel` to your `Cargo.toml`
2. **Initialize OpenTelemetry**: Use `OtelBuilder` in your main function
3. **Add to interceptor**: Call `apply_w3c_trace_context()` in your gRPC interceptor
4. **Downstream calls**: Use `inject_trace_context_into_request()` for outgoing requests

This enables distributed tracing with minimal code changes and maintains trace continuity across service boundaries.

## API Reference

### `grpc` Module

#### Functions

**`apply_w3c_trace_context(metadata: &MetadataMap) -> opentelemetry::Context`**
- Extracts W3C trace context from gRPC metadata and applies it as current context
- Should be called from gRPC interceptors for incoming requests
- Returns the extracted OpenTelemetry context

**`inject_trace_context_into_request<T>(request: Request<T>) -> Request<T>`**
- Injects current trace context into outgoing gRPC request metadata
- Should be called before making gRPC calls to downstream services
- Returns the request with W3C trace context headers added

#### Types

**`GrpcMetadataExtractor<'a>(&'a MetadataMap)`**
- Implements `opentelemetry::propagation::Extractor` for gRPC metadata
- Used internally by `apply_w3c_trace_context()`

**`GrpcMetadataInjector<'a>(&'a mut MetadataMap)`**
- Implements `opentelemetry::propagation::Injector` for gRPC metadata  
- Used internally by `inject_trace_context_into_request()`

## Integration Examples

### Service Integration Example

```rust
use flo_otel::grpc::{apply_w3c_trace_context, inject_trace_context_into_request};
use tonic::{Request, Response, Status, service::Interceptor};

pub struct TracingInterceptor;

impl Interceptor for TracingInterceptor {
  fn call(&mut self, req: Request<()>) -> Result<Request<()>, Status> {
    // Apply W3C trace context from incoming request
    apply_w3c_trace_context(req.metadata());
    Ok(req)
  }
}

// In your service implementation
async fn call_downstream_service(&self, data: SomeData) -> Result<Response, Error> {
  let request = Request::new(DownstreamRequest { data });
  
  // Inject trace context for distributed tracing
  let request = inject_trace_context_into_request(request);
  
  self.downstream_client.process(request).await
}
```

### Multi-Service Architecture

For microservices that both receive and make gRPC calls:

```rust
use flo_otel::grpc::{apply_w3c_trace_context, inject_trace_context_into_request};

// Configure your gRPC server with tracing interceptor
let service = MyServiceServer::new(service_impl)
  .interceptor(TracingInterceptor);

// Configure your gRPC clients to inject context
impl MyService {
  async fn handle_request(&self, req: MyRequest) -> Result<MyResponse, Error> {
    // Process request...
    
    // Call downstream service with trace context
    let downstream_req = inject_trace_context_into_request(
      Request::new(DownstreamRequest::from(req))
    );
    
    let response = self.downstream_client.process(downstream_req).await?;
    
    Ok(MyResponse::from(response))
  }
}
```

## Technical Details

- **W3C Compatibility**: Fully compliant with W3C Trace Context specification
- **Performance**: Minimal overhead using efficient header extraction/injection
- **Memory Safety**: Uses `std::mem::forget()` pattern for context lifecycle management
- **Error Handling**: Graceful handling of malformed or missing trace headers

## Troubleshooting

**Context not propagating between services:**
1. Verify both services use `apply_w3c_trace_context()` in their interceptors
2. Ensure outgoing requests use `inject_trace_context_into_request()`
3. Check that OpenTelemetry is configured with `TraceContextPropagator::new()`

**Missing trace headers:**
- Enable debug logging: `RUST_LOG=debug`
- Look for "Applying W3C trace context" log entries
- Verify upstream services are sending `traceparent` headers

**Performance concerns:**
- The library uses minimal allocations and efficient header parsing
- Context extraction/injection adds negligible latency (<1ms typically)
- Consider using sampling for high-traffic services 