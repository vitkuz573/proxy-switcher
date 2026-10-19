use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::Deserialize;

/// The engine's own API.
///
/// This tool talks to the engine directly, over loopback, without
/// authentication: it is an administration tool for the machine the engine
/// runs on, not a client for the public control plane. The public API has its
/// own client, in `cli/`.
#[derive(Parser)]
#[command(name = "proxy-cli", about = "Control the proxy-switcher engine")]
struct Cli {
    #[arg(short, long, default_value = "http://127.0.0.1:9099")]
    api_url: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the engine status
    Status,
    /// List the proxies in the pool
    List {
        /// How many to show
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Route traffic through a specific proxy
    Switch { id: String },
    /// Move to the next proxy in the pool
    Rotate,
    /// Stop routing through a proxy
    Clear,
    /// Show the routing policy
    Routing,
}

#[derive(Deserialize)]
struct SummaryResponse {
    active_proxy: Option<serde_json::Value>,
    pool_size: usize,
    healthy_count: usize,
}

/// Turn a non-success answer into a readable error.
///
/// The engine answers a failure with an RFC 9457 problem document, so the
/// reason is in the body rather than in the status alone.
async fn expect_ok(response: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(String::from))
        .unwrap_or(body);

    anyhow::bail!("{status}: {detail}")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = Client::new();

    match cli.command {
        Commands::Status => {
            let response = client
                .get(format!("{}/api/v1/summary", cli.api_url))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let summary: SummaryResponse = response.json().await?;

            println!("Active proxy: {:?}", summary.active_proxy);
            println!("Pool size:   {}", summary.pool_size);
            println!("Healthy:     {}", summary.healthy_count);
        }

        Commands::List { limit } => {
            let response = client
                .get(format!("{}/api/v1/proxies?limit={limit}", cli.api_url))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let proxies: Vec<serde_json::Value> = response.json().await?;

            for proxy in &proxies {
                println!("{}", serde_json::to_string_pretty(proxy)?);
            }
        }

        Commands::Switch { id } => {
            let response = client
                .put(format!("{}/api/v1/active-proxy", cli.api_url))
                .header("content-type", "application/json")
                .body(format!(r#"{{"proxy_id":{}}}"#, serde_json::to_string(&id)?))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let proxy: serde_json::Value = response.json().await?;

            println!("Switched to: {}", serde_json::to_string_pretty(&proxy)?);
        }

        Commands::Rotate => {
            let response = client
                .post(format!("{}/api/v1/active-proxy/rotate", cli.api_url))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let proxy: serde_json::Value = response.json().await?;

            println!("Rotated to: {}", serde_json::to_string_pretty(&proxy)?);
        }

        Commands::Clear => {
            let response = client
                .delete(format!("{}/api/v1/active-proxy", cli.api_url))
                .send()
                .await?;
            expect_ok(response).await?;

            println!("Cleared the active proxy");
        }

        Commands::Routing => {
            let response = client
                .get(format!("{}/api/v1/routing", cli.api_url))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let routing: serde_json::Value = response.json().await?;

            println!("{}", serde_json::to_string_pretty(&routing)?);
        }
    }

    Ok(())
}
