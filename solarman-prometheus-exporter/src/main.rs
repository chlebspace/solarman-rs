use anyhow::Context;
use clap::Parser;
use solarman_tokio::Client;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::info;

use std::sync::Arc;

use crate::metric_store::MetricStore;

mod metric;
mod metric_store;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Datalogger IP address
    address: String,

    /// Datalogger serial number
    serial: u32,

    /// Register map path
    regmap: PathBuf,

    /// Modbus slave ID (usually 1)
    #[arg(short, long, default_value = "1")]
    slave_id: u8,

    /// Prometheus metrics bind address
    #[arg(short, long, default_value = "[::]:9090")]
    bind: SocketAddr,
}

pub struct LoggerConfig {
    pub addr: SocketAddr,
    pub serial: u32,
    pub modbus_slave_id: u8,
}

pub struct MetricManager {
    store: MetricStore,
    max_store_age: Duration,
    last_scrape: Option<Instant>,
    logger_cfg: LoggerConfig,
}

impl MetricManager {
    pub fn new(store: MetricStore, logger_cfg: LoggerConfig, cache_age: Duration) -> Self {
        Self {
            store,
            max_store_age: cache_age,
            logger_cfg,
            last_scrape: None,
        }
    }

    pub async fn export(&mut self) -> anyhow::Result<String> {
        if let Some(last_scrape) = self.last_scrape
            && last_scrape.elapsed() < self.max_store_age
        {
            tracing::debug!("cache still valid, re-using last scraped metrics");
        } else {
            tracing::debug!("cache out of date, fetching metrics from inverter");
            let mut client = Client::connect(
                self.logger_cfg.addr,
                self.logger_cfg.serial,
                self.logger_cfg.modbus_slave_id,
            )
            .await
            .with_context(|| "failed to connect to the data logging stick")?;
            self.store.update_from_solarman(&mut client).await?;
            self.last_scrape = Some(Instant::now());
        }
        Ok(self.store.encode_prometheus())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let mut regmap_source = String::new();
    File::open(args.regmap)
        .await?
        .read_to_string(&mut regmap_source)
        .await?;
    let regmap = MetricStore::create(&regmap_source)?;

    let logger_cfg = LoggerConfig {
        addr: args.address.parse()?,
        serial: args.serial,
        modbus_slave_id: args.slave_id,
    };
    let metric_manager = MetricManager::new(regmap, logger_cfg, Duration::from_secs(30));
    let metric_manager = Arc::new(Mutex::new(metric_manager));

    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| "failed to bind the prometheus metrics socket")?;
    info!("Metrics server listening on http://{}", args.bind);

    loop {
        let (mut socket, _) = listener.accept().await?;
        let exporter = metric_manager.clone();

        tokio::spawn(async move {
            // TODO: write proper http support
            socket
                .write_all(exporter.lock().await.export().await?.as_bytes())
                .await?;
            anyhow::Ok(())
        });
    }
}
