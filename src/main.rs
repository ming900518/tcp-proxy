mod types;

use std::{error::Error, path::PathBuf, sync::OnceLock};

use actix_web::{
    post,
    web::{Json, Path},
    App, HttpServer,
};
use clap::Parser;
use futures_util::future::{abortable, join_all};
use mimalloc::MiMalloc;
use nix::sys::resource::{getrlimit, setrlimit, Resource};
use tokio::sync::Notify;
use tracing::{debug, warn, info, Level};
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};
use types::{Cli, ProxyConfig, RawConfig};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
static RESTART_PROXY: Notify = Notify::const_new();

fn main() -> Result<(), Box<dyn Error>> {
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

    std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                HttpServer::new(|| App::new().service(add_rules).service(remove_rules))
                    .disable_signals()
                    .bind(("0.0.0.0", 65535))?
                    .run()
                    .await
            })
            .ok();
    });

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let config_path = CONFIG_PATH.get_or_init(|| cli.config_path.clone());
            loop {
                let proxy_config = ProxyConfig::from_raw(&RawConfig::read_from_path(
                    config_path
                )?);

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

                let (task, handle) = abortable(
                    join_all(proxy_config.into_iter().map(|config| {
                        let boxed_config = Box::new(config);
                        let leaked_config = Box::leak(boxed_config);
                        leaked_config.start_proxy()
                    }))
                );

                tokio::spawn(async move {
                    RESTART_PROXY.notified().await;
                    info!("Restart signal received, restarting proxy server.");
                    handle.abort();
                });

                task.await.ok();
            }
        })
}

#[post("/add")]
async fn add_rules(Json(new_raw_config): Json<Vec<RawConfig>>) -> String {
    let Some(config_path) = CONFIG_PATH.get() else {
        return String::from("Couldn't get config path.");
    };

    let Ok(original_raw_config) = RawConfig::read_from_path(config_path) else {
        return String::from("Original config is invalid.");
    };

    let Ok(new_config_json) =
        serde_json::to_string_pretty(&[original_raw_config, new_raw_config].concat())
    else {
        return String::from("Unable to generate new config.");
    };

    let Ok(()) = std::fs::write(config_path, new_config_json) else {
        return String::from("Unable to write newly generated config.");
    };

    RESTART_PROXY.notify_one();

    String::from("OK")
}

#[post("/remove/{ip}")]
async fn remove_rules(path: Path<String>) -> String {
    let Some(config_path) = CONFIG_PATH.get() else {
        return String::from("Couldn't get config path.");
    };

    let Ok(mut original_raw_config) = RawConfig::read_from_path(config_path) else {
        return String::from("Original config is invalid.");
    };

    while let Some(i) = original_raw_config
        .iter()
        .position(|raw_config| raw_config.ip == *path)
    {
        original_raw_config.remove(i);
    }

    let Ok(new_config_json) = serde_json::to_string_pretty(&original_raw_config) else {
        return String::from("Unable to generate new config.");
    };

    let Ok(()) = std::fs::write(config_path, new_config_json) else {
        return String::from("Unable to write newly generated config.");
    };

    RESTART_PROXY.notify_one();

    String::from("OK")
}
