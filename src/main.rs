use tnnl::{client, config, server};

use clap::{Parser, Subcommand};

const DEFAULT_CONTROL_PORT: u16 = 9443;
const DEFAULT_HTTP_PORT: u16 = 8080;

#[derive(Parser)]
#[command(name = "tnnl", version, about = "Expose localhost to the internet. Self-hosted ngrok alternative.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the relay server on a public VPS
    Server {
        /// Base domain for tunnel subdomains (e.g. tunnel.example.com)
        #[arg(long)]
        domain: String,

        /// Authentication token clients must provide (omit for open/public server)
        #[arg(long)]
        token: Option<String>,

        /// Port for client control connections
        #[arg(long, default_value_t = DEFAULT_CONTROL_PORT)]
        control_port: u16,

        /// Port for public HTTP traffic
        #[arg(long, default_value_t = DEFAULT_HTTP_PORT)]
        http_port: u16,
    },

    /// Expose a local HTTP port through the tunnel
    Http {
        /// Local port to expose (e.g. 3000)
        port: u16,

        /// Server address (hostname or IP) — overrides config file
        #[arg(long = "to")]
        server: Option<String>,

        /// Authentication token — overrides config file
        #[arg(long)]
        token: Option<String>,

        /// Request a specific subdomain — overrides config file
        #[arg(long)]
        subdomain: Option<String>,

        /// Server control port — overrides config file
        #[arg(long)]
        control_port: Option<u16>,

        /// Protect the tunnel with HTTP basic auth (format: user:pass)
        #[arg(long)]
        auth: Option<String>,

        /// Print full request and response headers + body for every request
        #[arg(long)]
        inspect: bool,
    },

    /// Re-send a captured request to your local server
    Replay {
        /// Request ID shown in the tunnel log (e.g. 3)
        id: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Server {
            domain,
            token,
            control_port,
            http_port,
        } => {
            server::run(control_port, http_port, &domain, token.as_deref()).await?;
        }
        Command::Http {
            port,
            server,
            token,
            subdomain,
            control_port,
            auth,
            inspect,
        } => {
            let cfg = config::load();

            let server_addr = server.or(cfg.server).unwrap_or_else(|| "127.0.0.1".to_string());
            let token = token.or(cfg.token).unwrap_or_default();
            let ctrl_port = control_port.or(cfg.control_port).unwrap_or(DEFAULT_CONTROL_PORT);
            let sub = subdomain.or(cfg.subdomain);
            let basic_auth = auth.or(cfg.auth);
            let do_inspect = inspect || cfg.inspect.unwrap_or(false);

            client::run(port, &server_addr, ctrl_port, &token, sub.as_deref(), basic_auth.as_deref(), do_inspect).await?;
        }
        Command::Replay { id } => {
            client::replay(id).await?;
        }
    }

    Ok(())
}
