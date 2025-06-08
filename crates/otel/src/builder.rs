use crate::otel::{init_tracing_subscriber, Config, OtelGuard};
use opentelemetry::KeyValue;
use opentelemetry_semantic_conventions::attribute::DEPLOYMENT_ENVIRONMENT_NAME;
use opentelemetry_semantic_conventions::attribute::SERVICE_VERSION;

#[derive(Debug, Default)]
pub struct OtelBuilder {
  attributes: Vec<KeyValue>,
  project_name: Option<String>,
  environment: Option<String>,
  tracing_endpoint: Option<String>,
  metrics_endpoint: Option<String>,
}

impl OtelBuilder {
  #[must_use]
  pub fn new() -> Self {
    Self {
      attributes: Vec::new(),
      project_name: None,
      environment: None,
      tracing_endpoint: None,
      metrics_endpoint: None,
    }
  }

  #[must_use]
  pub fn set_project_name(mut self, name: String) -> Self {
    self.project_name = Some(name);
    self
  }

  #[must_use]
  pub fn set_version(mut self, version: String) -> Self {
    self
      .attributes
      .push(KeyValue::new(SERVICE_VERSION, version));
    self
  }

  #[must_use]
  pub fn set_environment(mut self, environment: String) -> Self {
    self.environment = Some(environment.clone());
    self.attributes.push(KeyValue::new(
      DEPLOYMENT_ENVIRONMENT_NAME,
      environment.to_string(),
    ));
    self
  }

  pub fn set_tracing_endpoint(mut self, endpoint: String) -> Self {
    self.tracing_endpoint = Some(endpoint);
    self
  }

  pub fn set_metrics_endpoint(mut self, endpoint: String) -> Self {
    self.metrics_endpoint = Some(endpoint);
    self
  }

  /// Can be used to set extra `attributes`
  /// to your telemetry. E.g. `KeyValue::new(SERVICE_NAME, "my-service")`
  pub fn set_attributes(mut self, attributes: &mut Vec<KeyValue>) -> Self {
    self.attributes.append(attributes);
    self
  }

  /// It's important that the guard is not dropped and must be assigned to a named variable
  ///     let _guard = OtelBuilder::new()
  //         .set_environment("dev".to_string())
  //         .set_project_name(env!("CARGO_PKG_NAME").to_string())
  //         .set_version(env!("CARGO_PKG_VERSION").to_string())
  //         .set_tracing_endpoint("http://jaeger:4317".to_string())
  //         .set_metrics_endpoint("http://otel-collector:4318".to_string())
  //         .build()
  //         .unwrap();
  ///
  pub fn build(self) -> Result<OtelGuard, Box<dyn std::error::Error>> {
    let environment = self.environment.ok_or("Environment not set")?;
    if environment.is_empty() {
      return Err("Environment cannot be empty".into());
    }
    let project_name = self.project_name.ok_or("Project name not set")?;
    if project_name.is_empty() {
      return Err("Project name cannot be empty".into());
    }
    let otel = Config {
      project_name,
      attributes: self.attributes,
      tracing_endpoint: self.tracing_endpoint,
      metrics_endpoint: self.metrics_endpoint,
    };
    Ok(init_tracing_subscriber(otel))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use opentelemetry_semantic_conventions::resource::SERVICE_NAME;

  #[test]
  fn test_ok() {
    let guard = OtelBuilder::new()
      .set_environment("dev".to_string())
      .set_project_name("project".to_string())
      .set_tracing_endpoint("http://jaeger:4317".to_string())
      .set_metrics_endpoint("http://otel-collector:4318".to_string())
      .build()
      .unwrap();
  }

  #[test]
  fn test_custom_attributes() {
    let guard = OtelBuilder::new()
      .set_environment("dev".to_string())
      .set_project_name("project".to_string())
      .set_tracing_endpoint("http://jaeger:4317".to_string())
      .set_metrics_endpoint("http://otel-collector:4318".to_string())
      .set_attributes(&mut vec![KeyValue::new(SERVICE_NAME, "test-service")]);
    assert_eq!(
      guard.attributes,
      vec![
        KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, "dev"),
        KeyValue::new(SERVICE_NAME, "test-service")
      ]
    );
  }

  #[test]
  fn test_missing() {
    // env not set
    assert!(OtelBuilder::new()
      .set_project_name("project".to_string())
      .set_attributes(&mut vec![KeyValue::new(SERVICE_NAME, "test-service")])
      .set_tracing_endpoint("http://jaeger:4317".to_string())
      .set_metrics_endpoint("http://otel-collector:4318".to_string())
      .build()
      .is_err());
    // env empty
    assert!(OtelBuilder::new()
      .set_environment("".to_string())
      .set_project_name("project".to_string())
      .set_attributes(&mut vec![KeyValue::new(SERVICE_NAME, "test-service")])
      .set_tracing_endpoint("http://jaeger:4317".to_string())
      .set_metrics_endpoint("http://otel-collector:4318".to_string())
      .build()
      .is_err());
    // project name not set
    assert!(OtelBuilder::new()
      .set_environment("dev".to_string())
      .set_attributes(&mut vec![KeyValue::new(SERVICE_NAME, "test-service")])
      .set_tracing_endpoint("http://jaeger:4317".to_string())
      .set_metrics_endpoint("http://otel-collector:4318".to_string())
      .build()
      .is_err());
  }
}
