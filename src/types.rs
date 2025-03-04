#![allow(dead_code)]

use std::{
    collections::BTreeSet,
    error::Error,
    fs::File,
    io::BufReader,
    iter::zip,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
};

use clap::Parser;
use serde::{Deserialize, Serialize};
use time::macros::{format_description, offset};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::fmt::time::OffsetTime;

#[derive(Parser)]
#[command(
    version,
    about = "tcp-proxy - Tokio based, flexible TCP Proxy implementation."
)]
pub struct Cli {
    pub config_path: Option<PathBuf>,
    #[arg(long)]
    /// Display debug logs.
    pub debug: bool,
}

impl Cli {
    pub fn init_logger(&self) {
        #[rustfmt::skip]
        let (source_info, max_level) = match (cfg!(debug_assertions), self.debug) {
            (source_info @ true, true) => (source_info, Level::TRACE),
            (source_info @ true, false)
             | (source_info @ false, true) => (source_info, Level::DEBUG),
            (source_info @ false, false) => (source_info, Level::INFO),
        };

        let format = tracing_subscriber::fmt::format()
            .compact()
            .with_timer(OffsetTime::new(
                offset!(+8),
                format_description!("[year]-[month]-[day]T[hour]:[minute]:[second][offset_hour sign:mandatory]:[offset_minute padding:zero]"),
            ))
            .with_target(true)
            .with_thread_names(source_info)
            .with_source_location(source_info)
            .with_line_number(source_info);

        tracing_subscriber::fmt()
            .event_format(format)
            .with_max_level(max_level)
            .init();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawConfig {
    ip: String,
    port: SourcePortOptions,
    target_port: TargetPortOptions,
}

impl RawConfig {
    pub fn read_from_path(path: &Path) -> Result<Vec<Self>, Box<dyn Error>> {
        let reader = BufReader::new(File::open(path)?);
        serde_json::from_reader(reader).map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
enum SourcePortOptions {
    Range { start: u16, end: u16 },
    Single(u16),
}

impl Default for SourcePortOptions {
    fn default() -> Self {
        Self::Range {
            start: u16::default(),
            end: u16::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
enum TargetPortOptions {
    Range { start: u16, end: u16 },
    Single(u16),
}

impl Default for TargetPortOptions {
    fn default() -> Self {
        Self::Range {
            start: u16::default(),
            end: u16::default(),
        }
    }
}

#[derive(Debug)]
pub struct ProxyConfig {
    pub source_addr: SocketAddrV4,
    pub target_addr: SocketAddrV4,
}

impl ProxyConfig {
    const fn new((source_addr, target_addr): (SocketAddrV4, SocketAddrV4)) -> Self {
        Self {
            source_addr,
            target_addr,
        }
    }

    #[rustfmt::skip]
    pub fn from_raw(raw_config_list: &[RawConfig]) -> Vec<Self> {
        let target_ip = Ipv4Addr::new(0, 0, 0, 0);
        raw_config_list
            .iter()
            .filter_map(|raw_config| {
                let Ok(source_ip) = raw_config.ip.parse() else {
                    return None;
                };
                match (raw_config.port, raw_config.target_port) {
                    (
                        SourcePortOptions::Range { start: source_start, end: source_end },
                        TargetPortOptions::Range { start: target_start, end: target_end },
                    ) => {
                        if source_end - source_start != target_end - target_start {
                            warn!("IP {}'s source ports and target ports has different lengths, some port will not be exposed.", raw_config.ip);
                        }
                        let result = zip(source_start..=source_end, target_start..=target_end)
                            .map(|(source_port, target_port)| (SocketAddrV4::new(source_ip, source_port), SocketAddrV4::new(target_ip, target_port)))
                            .collect();
                        Some(result)
                    }
                    (
                        SourcePortOptions::Single(source_port),
                        TargetPortOptions::Single(target_port),
                    ) => {
                        let result = vec![(SocketAddrV4::new(source_ip, source_port), SocketAddrV4::new(target_ip, target_port))];
                        Some(result)
                    }
                    _ => {
                        error!("IP {}'s port option is invalid, the setup process for this IP will be skipped.", raw_config.ip);
                        None
                    },
                }
            })
            .flatten()
            .collect::<BTreeSet<(SocketAddrV4, SocketAddrV4)>>()
            .into_iter()
            .map(Self::new)
            .collect()
    }

    pub async fn start_proxy(&'static self) -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind(self.target_addr).await?;
        info!(
            "Proxy for {} started, bind as {}.",
            self.source_addr, self.target_addr
        );
        while let Ok((mut inbound_stream, client_addr)) = listener.accept().await {
            tokio::spawn(async move {
                debug!("New user: {client_addr}");
                let mut outbound_stream = TcpStream::connect(self.source_addr).await?;
                match tokio::io::copy_bidirectional(&mut inbound_stream, &mut outbound_stream).await
                {
                    Ok((to_outbound, to_inbound)) => {
                        debug!("Processed {to_outbound} bytes from client, {to_inbound} bytes from server.");
                    }
                    Err(err) => {
                        warn!("Error while proxying: {}", err);
                    }
                }
                Ok::<(), Box<dyn Error + Sync + Send + 'static>>(())
            });
        }

        Ok(())
    }
}
