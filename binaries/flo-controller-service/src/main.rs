use flo_controller::{serve_grpc, serve_socket, ControllerState};
use flo_otel::builder::OtelBuilder;
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  #[cfg(debug_assertions)]
  {
    std::env::set_var(
      "RUST_LOG",
      "flo_controller_service=debug,flo_controller=debug,flo_event=debug",
    );
    dotenv::dotenv()?;
  }

  let _guard: Result<flo_otel::otel::OtelGuard, Box<dyn Error + 'static>> = OtelBuilder::new()
    .set_environment({
      let var_name = "OTEL_DEPLOYMENT_ENVIRONMENT";
      std::env::var(var_name).unwrap_or_else(|_| {
        eprintln!(
          "WARNING: {} not set, falling back to \"{}\"",
          var_name, "local-undefined"
        );
        "local-undefined".to_string()
      })
    })
    .set_project_name({
      let var_name = "OTEL_SERVICE_NAME";
      std::env::var(var_name).unwrap_or_else(|_| {
        eprintln!(
          "WARNING: {} not set, falling back to \"{}\"",
          var_name,
          env!("CARGO_PKG_NAME")
        );
        env!("CARGO_PKG_NAME").to_string()
      })
    })
    .set_version(env!("CARGO_PKG_VERSION").to_string())
    .set_tracing_endpoint({
      let var_name = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
      std::env::var(var_name).unwrap_or_else(|_| {
        eprintln!(
          "WARNING: {} not set, continuing without trace export",
          var_name
        );
        "".to_string()
      })
    })
    .set_metrics_endpoint({
      let var_name = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT";
      std::env::var(var_name).unwrap_or_else(|_| {
        eprintln!(
          "WARNING: {} not set, continuing without metrics export",
          var_name
        );
        "".to_string()
      })
    })
    .build();

  tracing::info!("Starting flo-controller-service");
  let state = ControllerState::init().await?.into_ref();

  #[cfg(unix)]
  {
    use tokio::signal::unix::{signal, SignalKind};
    let mut stream = signal(SignalKind::hangup())?;
    tokio::spawn({
      let state = state.clone();
      async move {
        loop {
          stream.recv().await;
          tracing::info!("reloading");
          if let Err(err) = state.reload().await {
            tracing::error!("reload error: {}", err);
          }
        }
      }
    });
  }

  tracing::info!("Initiating gRPC and socket serving");
  tokio::try_join!(serve_grpc(state.clone()), serve_socket(state.clone()))?;

  Ok(())
}
