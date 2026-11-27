use std::{env, process::ExitCode};
use yo_mesh::{load, run};

#[tokio::main(worker_threads = 2)]
async fn main() -> ExitCode {
    let path = env::args_os().nth(1).unwrap_or_else(|| "/etc/yo/mesh.toml".into());
    let result = match load(path) {
        Ok(config) => run(config).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("yo-mesh: {error}");
            ExitCode::FAILURE
        }
    }
}
