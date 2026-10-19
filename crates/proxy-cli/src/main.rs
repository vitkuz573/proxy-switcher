use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::Deserialize;

/// The languages the tool speaks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Locale {
    English,
    Russian,
}

impl Locale {
    /// The language to speak.
    ///
    /// An explicit choice wins, then the environment, then English. The
    /// environment is how a user says once what language a machine speaks.
    fn resolve(explicit: Option<&str>) -> Self {
        if let Some(explicit) = explicit {
            if explicit.eq_ignore_ascii_case("ru") {
                return Locale::Russian;
            }
            if explicit.eq_ignore_ascii_case("en") {
                return Locale::English;
            }
        }

        for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(name) {
                let tag = value.split('.').next().unwrap_or_default().to_lowercase();
                if tag.starts_with("ru") {
                    return Locale::Russian;
                }
                if tag.starts_with("en") {
                    return Locale::English;
                }
            }
        }

        Locale::English
    }

    /// The text for a key.
    fn text(self, key: &'static str) -> &'static str {
        match (self, key) {
            (Locale::English, "activeProxy") => "Active proxy:",
            (Locale::English, "poolSize") => "Pool size:",
            (Locale::English, "healthy") => "Healthy:",
            (Locale::English, "switchedTo") => "Switched to:",
            (Locale::English, "rotatedTo") => "Rotated to:",
            (Locale::English, "cleared") => "Cleared the active proxy",

            (Locale::Russian, "activeProxy") => "Активный прокси:",
            (Locale::Russian, "poolSize") => "Размер пула:",
            (Locale::Russian, "healthy") => "Рабочих:",
            (Locale::Russian, "switchedTo") => "Переключено на:",
            (Locale::Russian, "rotatedTo") => "Переключено на:",
            (Locale::Russian, "cleared") => "Активный прокси снят",

            (_, other) => other,
        }
    }
}

#[derive(Parser)]
#[command(name = "proxy-cli", about = "Control the proxy-switcher engine")]
struct Cli {
    #[arg(short, long, default_value = "http://127.0.0.1:9099")]
    api_url: String,

    /// The language for the output: en or ru
    #[arg(long)]
    locale: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show daemon status
    Status,
    /// List all proxies
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
    let locale = Locale::resolve(cli.locale.as_deref());
    let client = Client::new();

    match cli.command {
        Commands::Status => {
            let response = client
                .get(format!("{}/api/v1/summary", cli.api_url))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let summary: SummaryResponse = response.json().await?;

            println!("{} {:?}", locale.text("activeProxy"), summary.active_proxy);
            println!("{}   {}", locale.text("poolSize"), summary.pool_size);
            println!("{}     {}", locale.text("healthy"), summary.healthy_count);
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

            println!(
                "{} {}",
                locale.text("switchedTo"),
                serde_json::to_string_pretty(&proxy)?
            );
        }

        Commands::Rotate => {
            let response = client
                .post(format!("{}/api/v1/active-proxy/rotate", cli.api_url))
                .send()
                .await?;
            let response = expect_ok(response).await?;
            let proxy: serde_json::Value = response.json().await?;

            println!(
                "{} {}",
                locale.text("rotatedTo"),
                serde_json::to_string_pretty(&proxy)?
            );
        }

        Commands::Clear => {
            let response = client
                .delete(format!("{}/api/v1/active-proxy", cli.api_url))
                .send()
                .await?;
            expect_ok(response).await?;

            println!("{}", locale.text("cleared"));
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
