use anyhow::Result;
use clap::Parser;

use crate::{
    agent::Agent,
    cli::{Cli, Command, ConfigCommand},
    config::AppConfig,
};

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.json_logs);

    let config = AppConfig::load(&cli)?;

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

fn init_tracing(json_logs: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if json_logs {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}
