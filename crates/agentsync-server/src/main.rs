use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(about = "Private encrypted AgentSync relay; expose through an HTTPS reverse proxy")]
struct Args {
    /// New private directory, or an existing AgentSync relay directory.
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!(
                "AgentSync relay: startup or serving failed. Check loopback bind, private relay directory and AGENTSYNC_RELAY_TOKEN (64 lowercase hex characters)."
            );
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(
        args.listen.ip().is_loopback(),
        "use an HTTPS reverse proxy or tunnel to the loopback listener"
    );
    let token = std::env::var("AGENTSYNC_RELAY_TOKEN")?;
    let app = agentsync_server::router(&args.data_dir, &token)?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    eprintln!("AgentSync relay listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
