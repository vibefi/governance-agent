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
    #[command(
        about = "Run the agent scan/review loop",
        long_about = "Continuously scans new blocks for proposals, reviews them, and optionally submits votes when auto-vote is enabled."
    )]
    Run(RunArgs),
    #[command(
        about = "Review a single proposal by id",
        long_about = "Fetches one proposal by id, runs bundle + LLM review, and prints the resulting decision logs."
    )]
    ReviewOnce(ReviewOnceArgs),
    #[command(
        about = "Process a historical block range",
        long_about = "Backfills proposal processing for a block range. Use --to-block to cap the range, or omit it to scan through the current chain tip."
    )]
    Backfill(BackfillArgs),
    #[command(
        about = "Run health checks (RPC, storage, notifier config)",
        long_about = "Verifies RPC connectivity and chain id, reports transport mode, prints configured storage path, and indicates notifier configuration. It does not scan proposals or submit votes."
    )]
    Status,
    #[command(about = "Inspect resolved runtime configuration")]
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long, help = "Run a single scan cycle and exit")]
    pub once: bool,
}

#[derive(Debug, Args)]
pub struct ReviewOnceArgs {
    #[arg(long, help = "Proposal id to review (uint256 as decimal or 0x hex)")]
    pub proposal_id: String,
}

#[derive(Debug, Args)]
pub struct BackfillArgs {
    #[arg(long, help = "Start block number (inclusive)")]
    pub from_block: u64,

    #[arg(long, help = "End block number (inclusive); defaults to latest block")]
    pub to_block: Option<u64>,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    #[command(about = "Print the fully resolved config as JSON")]
    Print,
}
