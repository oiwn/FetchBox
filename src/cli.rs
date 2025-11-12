use clap::{Parser, Subcommand};
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[command(name = "fetchbox")]
#[command(about = "FetchBox CLI", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the FetchBox API service
    Api(ApiArgs),

    /// Run the FetchBox download worker
    Worker,
}

#[derive(clap::Args, Debug)]
pub struct ApiArgs {
    /// Address to bind the API server to
    #[arg(long, default_value = "0.0.0.0:8080")]
    pub address: SocketAddr,

    /// Path to Fjall ledger storage
    #[arg(long, default_value = "data/ledger")]
    pub ledger_path: String,
}
