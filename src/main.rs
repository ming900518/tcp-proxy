mod types;

use std::{error::Error, fs::write, process::ExitCode};

use clap::Parser;
use futures_util::future::join_all;
use mimalloc::MiMalloc;
use nix::sys::resource::{getrlimit, setrlimit, Resource};
use serde_json::to_string_pretty;
use tracing::{debug, warn};
use types::{Cli, ProxyConfig, RawConfig};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[tokio::main]
async fn main() -> Result<ExitCode, Box<dyn Error>> {
    let cli = Cli::parse();

    cli.init_logger();

    let Some(config_path) = cli.config_path else {
        let example_raw_config = vec![RawConfig::default()];
        write("config.json", to_string_pretty(&example_raw_config)?)?;
        warn!("No config specified, example config has been saved to `./config.json`.");
        return Ok(ExitCode::FAILURE);
    };

    let proxy_config = ProxyConfig::from_raw(&RawConfig::read_from_path(&config_path)?);

    let desired_limit = (proxy_config.len() / 10 * 20 + 1) as u64;

    match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok((soft_limit, hard_limit)) if soft_limit <= proxy_config.len() as u64 => {
            debug!("Current system limit of open files and sockets ({soft_limit}) is not enough, trying to increase the limit to {desired_limit}.");
            setrlimit(Resource::RLIMIT_NOFILE, desired_limit, hard_limit)?;
            debug!("System limit has been set to {desired_limit}.");
        },
        Err(error) => warn!("Unable to fetch the current system limit ({error}). This tool might fail to listen all the ports specified in the configuration file, if you noticed any problem, try execute `ulimit -n {desired_limit}` command and restart this tool."),
        _ => ()
    }

    join_all(proxy_config.into_iter().map(|config| {
        let boxed_config = Box::new(config);
        let leaked_config = Box::leak(boxed_config);
        leaked_config.start_proxy()
    }))
    .await;

    Ok(ExitCode::SUCCESS)
}
