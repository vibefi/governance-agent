#[tokio::main]
async fn main() {
    if let Err(err) = governance_agent::app::run().await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
