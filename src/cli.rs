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
    /// Run the HTTP server (stub implementation)
    Server(ServerArgs),
}

#[derive(clap::Args, Debug)]
pub struct ServerArgs {
    /// Address to bind the HTTP server to
    #[arg(long, default_value = "0.0.0.0:8080")]
    pub address: SocketAddr,
}
