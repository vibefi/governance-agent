use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "governance-agent")]
#[command(about = "VibeFi governance review and voting agent")]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        default_value = "devnet",
        env = "GOV_AGENT_PROFILE"
    )]
    pub profile: String,

    #[arg(long, global = true, env = "GOV_AGENT_RPC_URL")]
    pub rpc_url: Option<String>,

    #[arg(
        long,
        global = true,
        env = "GOV_AGENT_AUTO_VOTE",
        default_value_t = false
    )]
    pub auto_vote: bool,

    #[arg(long, global = true)]
    pub json_logs: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Run(RunArgs),
    ReviewOnce(ReviewOnceArgs),
    Backfill(BackfillArgs),
    Doctor,
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub once: bool,
}

#[derive(Debug, Args)]
pub struct ReviewOnceArgs {
    #[arg(long)]
    pub proposal_id: u64,
}

#[derive(Debug, Args)]
pub struct BackfillArgs {
    #[arg(long)]
    pub from_block: u64,

    #[arg(long)]
    pub to_block: Option<u64>,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Print,
}
