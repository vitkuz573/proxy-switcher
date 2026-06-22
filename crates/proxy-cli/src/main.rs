use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "proxy-cli", about = "Control proxy-switcher daemon")]
struct Cli {
    #[arg(short, long, default_value = "http://127.0.0.1:8080")]
    api_url: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show daemon status
    Status,
    /// List all proxies
    List,
    /// Switch to a specific proxy
    Switch { id: String },
    /// Rotate to next proxy
    Rotate,
}

#[derive(Deserialize)]
struct StatusResponse {
    active_proxy: Option<serde_json::Value>,
    pool_size: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = Client::new();

    match cli.command {
        Commands::Status => {
            let resp: StatusResponse = client
                .get(format!("{}/api/v1/status", cli.api_url))
                .send()
                .await?
                .json()
                .await?;

            println!("Active proxy: {:?}", resp.active_proxy);
            println!("Pool size: {}", resp.pool_size);
        }
        Commands::List => {
            let resp: Vec<serde_json::Value> = client
                .get(format!("{}/api/v1/proxies", cli.api_url))
                .send()
                .await?
                .json()
                .await?;

            for proxy in &resp {
                println!("{}", serde_json::to_string_pretty(proxy)?);
            }
        }
        Commands::Switch { id } => {
            let resp: serde_json::Value = client
                .post(format!("{}/api/v1/proxies/{}/switch", cli.api_url, id))
                .send()
                .await?
                .json()
                .await?;

            println!("Switched to: {}", serde_json::to_string_pretty(&resp)?);
        }
        Commands::Rotate => {
            let resp: serde_json::Value = client
                .post(format!("{}/api/v1/rotate", cli.api_url))
                .send()
                .await?
                .json()
                .await?;

            println!("Rotated to: {}", serde_json::to_string_pretty(&resp)?);
        }
    }

    Ok(())
}
