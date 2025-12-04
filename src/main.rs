mod cli;

use clap::Parser;
use cli::{Cli, Commands};
use fetchbox::api;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Api(args) => {
            api::run_with_config_path(args.address, args.config).await?
        }
        Commands::Worker => {
            eprintln!(
                "Worker mode is temporarily disabled during architecture transition"
            );
            std::process::exit(1);
        }
    }

    Ok(())
}
