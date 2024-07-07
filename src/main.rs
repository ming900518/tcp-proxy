mod types;

use std::{error::Error, process::exit};

use clap::Parser;
use futures_util::future::join_all;
use mimalloc::MiMalloc;
use nix::sys::resource::{getrlimit, setrlimit, Resource};
use tracing::{debug, error, warn, Level};
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};
use types::{Cli, ProxyConfig, RawConfig};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> Result<(), Box<dyn Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let cli = Cli::parse();

            let tracing = tracing_subscriber::registry().with(tracing_subscriber::fmt::layer());

            if cli.debug {
                tracing
                    .with(Targets::new().with_default(Level::DEBUG))
                    .init();
            } else {
                tracing
                    .with(Targets::new().with_default(Level::INFO))
                    .init();
            }

            let proxy_config = ProxyConfig::from_raw(&RawConfig::read_from_path(&cli.config_path)?);

            let current_limit = getrlimit(Resource::RLIMIT_NOFILE);
            let desired_limit = ((proxy_config.len() / 10 + 1) * 20) as u64;

            match current_limit {
                Ok((soft_limit, hard_limit)) if soft_limit <= proxy_config.len() as u64 => {
                    debug!("Current system limit of open files and sockets ({soft_limit}) is not enough, will try to increase the limit to {desired_limit}.");
                    setrlimit(Resource::RLIMIT_NOFILE, desired_limit, hard_limit)?;
                    tokio::spawn(async move {
                        if let Err(error) = tokio::signal::ctrl_c().await {
                            error!("Unable to listen CTRL-C keybind ({error:?}), system limit will not be recovered");
                        } else {
                            debug!("User pressed CTRL-C! System limit has been set to {desired_limit}, recovering the original limit: {soft_limit}.");
                            if let Err(error) = setrlimit(Resource::RLIMIT_NOFILE, soft_limit, hard_limit) {
                                warn!("Unable to recover the original system limit ({error}), use `ulimit -n {soft_limit}` command manually to recover.");
                                exit(1);
                            } else {
                                exit(0);
                            }
                        }
                    });
                    debug!("System limit has been set to {desired_limit}, the original limit will be recovered when exiting this tool with CTRL-C.");
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

            Ok(())
        })
}
