#![recursion_limit = "256"]

#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;
extern crate get_if_addrs;
extern crate plotters;
extern crate procfs;
extern crate sha2;
extern crate slice_deque;
#[macro_use]
extern crate serde;
extern crate toml;

use async_std::fs::File;
use async_std::io::ReadExt;
use async_std::prelude::*;

use async_std::sync::{Arc, RwLock};
use once_cell::sync::Lazy;

mod actors;
mod broker;
mod config;
mod error;
mod storage;

use broker::*;
use config::Config;
use error::SolarResult;
use storage::blob::BlobStorage;
use storage::feed::FeedStorage;

const LISTEN: &str = "0.0.0.0:8008";
const RPC_PORT: u16 = 8008;

pub static FEED_STORAGE: Lazy<Arc<RwLock<FeedStorage>>> =
    Lazy::new(|| Arc::new(RwLock::new(FeedStorage::default())));
pub static BLOB_STORAGE: Lazy<Arc<RwLock<BlobStorage>>> =
    Lazy::new(|| Arc::new(RwLock::new(BlobStorage::default())));

#[async_std::main]
async fn main() -> SolarResult<()> {
    env_logger::init();
    log::set_max_level(log::LevelFilter::max());

    let base_path = xdg::BaseDirectories::new()?.create_data_directory("solar")?;

    println!("Base configuration is {:?}", base_path);

    let mut key_file = base_path.clone();
    let mut feeds_folder = base_path.clone();
    let mut blobs_folder = base_path;

    key_file.push("solar.toml");
    feeds_folder.push("feeds");
    blobs_folder.push("blobs");
    std::fs::create_dir_all(&feeds_folder)?;
    std::fs::create_dir_all(&blobs_folder)?;

    let config = if !key_file.is_file() {
        println!("Private key not found, generated new one in {:?}", key_file);
        let config = Config::create();
        let mut file = File::create(key_file).await?;
        file.write_all(&config.to_toml()?).await?;
        config
    } else {
        let mut file = File::open(key_file).await?;
        let mut raw: Vec<u8> = Vec::new();
        file.read_to_end(&mut raw).await?;
        Config::from_toml(&raw)?
    };
    let owned_id = config.owned_identity()?;

    println!(
        "Server started on {}:{}",
        LISTEN,
        base64::encode(&owned_id.pk[..])
    );

    FEED_STORAGE
        .write()
        .await
        .open(&feeds_folder, BROKER.lock().await.create_sender())?;

    BLOB_STORAGE.write().await.open(blobs_folder);

    Broker::spawn(actors::ctrlc::actor());
    Broker::spawn(actors::landiscovery::actor(
        base64::encode(&owned_id.pk[..]),
        RPC_PORT,
    ));
    Broker::spawn(actors::sensor::actor(owned_id.clone()));
    Broker::spawn(actors::muxrpc::actor(owned_id, LISTEN));

    let msgloop = BROKER.lock().await.take_msgloop();
    msgloop.await;

    println!("Gracefully finished");
    Ok(())
}
