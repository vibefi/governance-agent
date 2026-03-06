use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    agent::Agent,
    cli::{Cli, Command, ConfigCommand},
    config::{AppConfig, ObservabilityConfig},
    observability,
};

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(&cli)?;

    let _telemetry_guard = init_tracing(cli.json_logs, &config.observability)?;
    if should_init_metrics(&cli.command) {
        observability::init_metrics(&config.observability)?;
    }

    match &cli.command {
        Command::Config(args) => match args.command {
            ConfigCommand::Print => {
                println!("{}", serde_json::to_string_pretty(&config)?);
                Ok(())
            }
        },
        Command::Status => {
            let agent = Agent::new(config)?;
            agent.status().await
        }
        Command::Run(args) => {
            let agent = Agent::new(config)?;
            agent.run_loop(args.once).await
        }
        Command::Backfill(args) => {
            let agent = Agent::new(config)?;
            agent.backfill(args.from_block, args.to_block).await
        }
        Command::ReviewOnce(args) => {
            let agent = Agent::new(config)?;
            agent.review_once(args.proposal_id.clone()).await
        }
    }
}

fn should_init_metrics(command: &Command) -> bool {
    matches!(
        command,
        Command::Run(_) | Command::Backfill(_) | Command::ReviewOnce(_)
    )
}

fn init_tracing(json_logs: bool, cfg: &ObservabilityConfig) -> Result<TelemetryGuard> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let mut guard = TelemetryGuard::default();
    let otlp_endpoint = cfg
        .otlp_endpoint
        .clone()
        .filter(|endpoint| !endpoint.trim().is_empty());

    if let Some(endpoint) = otlp_endpoint {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .with_timeout(Duration::from_secs(cfg.otlp_timeout_secs))
            .build()?;

        let resource = Resource::builder_empty()
            .with_attributes([KeyValue::new("service.name", cfg.otlp_service_name.clone())])
            .build();

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build();
        let tracer = tracer_provider.tracer("gov-agent");
        opentelemetry::global::set_tracer_provider(tracer_provider.clone());

        if json_logs {
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().json())
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .try_init()?;
        } else {
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer())
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .try_init()?;
        }

        tracing::info!(
            service_name = %cfg.otlp_service_name,
            "otlp tracing exporter enabled"
        );
        guard.tracer_provider = Some(tracer_provider);
    } else if json_logs {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()?;
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .try_init()?;
    }

    Ok(guard)
}

#[derive(Default)]
struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
}
