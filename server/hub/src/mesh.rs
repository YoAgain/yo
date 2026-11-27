use super::{shutdown_signal, var};
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use yo_mesh::{load, Result};

pub async fn serve(path: String, stop: tokio::sync::oneshot::Receiver<()>) {
    let result = match load(path) {
        Ok(config) => {
            yo_mesh::run_with(config, async {
                let _ = stop.await;
            })
            .await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = result {
        eprintln!("yo-hub: mesh: {e}");
    }
}

pub async fn command(args: &[String]) -> Result<bool> {
    let status: SocketAddr = var("YO_RELAY_STATUS").unwrap_or_else(|| "127.0.0.1:8091".into()).parse()?;
    if !status.ip().is_loopback() {
        return Err("relay status requires loopback".into());
    }
    match args {
        [cmd] if cmd == "relay-status" => {
            let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build()?;
            let json: serde_json::Value =
                client.get(format!("http://{status}/status")).send().await?.error_for_status()?.json().await?;
            println!("{json}");
            Ok(json["upstream_ok"] == true)
        }
        [cmd, upstream, rest @ ..] if cmd == "--relay" && rest.len() <= 1 => {
            let path = match rest.first() {
                Some(path) => PathBuf::from(path),
                None => PathBuf::from(
                    std::env::var_os("CREDENTIALS_DIRECTORY").ok_or("set CREDENTIALS_DIRECTORY or pass MESH_TOML")?,
                )
                .join("mesh.toml"),
            };
            let listen = var("YO_LISTEN").unwrap_or_else(|| "127.0.0.1:8090".into()).parse()?;
            yo_mesh::relay(load(path)?, upstream.clone(), listen, status, shutdown_signal()).await?;
            Ok(true)
        }
        _ => Err("usage: yo-hub --relay UPSTREAM [MESH_TOML] | yo-hub relay-status".into()),
    }
}
