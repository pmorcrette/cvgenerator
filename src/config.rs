use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Host beautiful, themeable CVs and manage them through a REST API or MCP"
)]
pub struct Cli {
    #[command(flatten)]
    pub config: Config,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the web server (default): CV pages, admin UI, REST API and MCP over HTTP.
    Serve,
    /// Run the MCP server over stdio, for local AI agents (Claude Desktop, IDEs...).
    Mcp,
}

#[derive(Debug, Clone, Args)]
pub struct Config {
    /// Address the web server listens on.
    #[arg(long, env = "CV_BIND", default_value = "0.0.0.0:8080")]
    pub bind: SocketAddr,

    /// Directory where CVs and custom themes are stored.
    #[arg(long, env = "CV_DATA_DIR", default_value = "./data")]
    pub data_dir: PathBuf,

    /// Secret required (as `Authorization: Bearer <token>`) by the REST API,
    /// the admin UI and the MCP endpoint. Leave unset only on trusted networks.
    #[arg(long, env = "CV_API_TOKEN", hide_env_values = true)]
    pub api_token: Option<String>,

    /// Public base URL, used to give agents links to rendered CVs.
    #[arg(long, env = "CV_PUBLIC_URL")]
    pub public_url: Option<String>,

    /// CV shown on `/`. When unset `/` redirects to the admin dashboard.
    #[arg(long, env = "CV_HOME")]
    pub home: Option<String>,

    /// Create an example CV when the data directory is empty.
    #[arg(long, env = "CV_SEED_EXAMPLE", default_value_t = true, action = clap::ArgAction::Set)]
    pub seed_example: bool,
}

impl Config {
    /// Absolute URL (or path when no public URL is configured) of a CV page.
    pub fn cv_url(&self, id: &str) -> String {
        let base = self
            .public_url
            .as_deref()
            .unwrap_or("")
            .trim_end_matches('/');
        format!("{base}/cv/{id}")
    }

    pub fn api_token(&self) -> Option<&str> {
        self.api_token.as_deref().filter(|t| !t.is_empty())
    }
}
