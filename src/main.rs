#![recursion_limit = "256"]

#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;
extern crate procfs;
extern crate sha2;
extern crate slice_deque;
#[macro_use]
extern crate serde;
extern crate anyhow;
extern crate toml;

use async_std::{fs::File, io::ReadExt, prelude::*};

use async_std::sync::{Arc, RwLock};
use once_cell::sync::{Lazy, OnceCell};
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(name = "🌞 Solar", about = "Sunbathing scuttlecrabs in kuskaland", version=env!("SOLAR_VERSION"))]
struct Opt {
    /// Where data is stored, ~/.local/share/local by default
    #[structopt(short, long, parse(from_os_str))]
    data: Option<PathBuf>,

    /// Connect to peers host:port:publickey,host:port:publickey,...
    #[structopt(short, long)]
    connect: Option<String>,

    /// List of friends, "connect" magical word means that --connect peers are friends
    #[structopt(short, long)]
    friends: Option<String>,

    /// Port to bind, 8008 by default
    #[structopt(short, long)]
    port: Option<u16>,

    /// Run lan discovery
    #[structopt(short, long)]
    lan: Option<bool>,
}

mod actors;
mod broker;
mod config;
mod storage;

use anyhow::Result;
use broker::*;
use config::Config;
use kuska_ssb::crypto::{ToSodiumObject, ToSsbId};
use storage::{blob::BlobStorage, kv::KvStorage};

const RPC_PORT: u16 = 8008;

pub static KV_STORAGE: Lazy<Arc<RwLock<KvStorage>>> =
    Lazy::new(|| Arc::new(RwLock::new(KvStorage::default())));
pub static BLOB_STORAGE: Lazy<Arc<RwLock<BlobStorage>>> =
    Lazy::new(|| Arc::new(RwLock::new(BlobStorage::default())));
pub static CONFIG: OnceCell<Config> = OnceCell::new();

#[async_std::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();

    println!("🌞 Solar {}", env!("SOLAR_VERSION"));

    let base_path = opt
        .data
        .unwrap_or(xdg::BaseDirectories::new()?.create_data_directory("solar")?);

    let rpc_port = opt.port.unwrap_or(RPC_PORT);
    let lan_discovery = opt.lan.unwrap_or(false);
    let listen = format!("0.0.0.0:{}", rpc_port);

    env_logger::init();
    log::set_max_level(log::LevelFilter::max());

    println!("Base configuration is {:?}", base_path);

    let mut key_file = base_path.clone();
    let mut feeds_folder = base_path.clone();
    let mut blobs_folder = base_path;

    key_file.push("solar.toml");
    feeds_folder.push("feeds");
    blobs_folder.push("blobs");
    std::fs::create_dir_all(&feeds_folder)?;
    std::fs::create_dir_all(&blobs_folder)?;

    let mut config = if !key_file.is_file() {
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

    let mut connects = Vec::new();
    if let Some(connect) = opt.connect {
        for peer in connect.split(',') {
            let invalid_peer_msg = || format!("invalid peer {}", peer);
            let parts = peer.split(':').collect::<Vec<&str>>();
            if parts.len() != 3 {
                panic!(invalid_peer_msg());
            }
            let server = parts[0].to_string();
            let port = parts[1]
                .parse::<u32>()
                .unwrap_or_else(|_| panic!(invalid_peer_msg()));
            let peer_pk = parts[2]
                .to_ed25519_pk_no_suffix()
                .unwrap_or_else(|_| panic!(invalid_peer_msg()));
            connects.push((server, port, peer_pk));
        }
    }

    if let Some(friends) = opt.friends {
        for friend in friends.split(',') {
            if friend == "connect" {
                for conn in &connects {
                    config.friends.push(format!("@{}", conn.2.to_ssb_id()));
                }
            } else {
                config.friends.push(friend.to_string())
            }
        }
    }
    debug!(target:"solar", "friends are {:?}",config.friends);
    let owned_id = config.owned_identity()?;
    let _err = CONFIG.set(config);

    println!(
        "Server started on {}:{}",
        listen,
        base64::encode(&owned_id.pk[..])
    );

    KV_STORAGE
        .write()
        .await
        .open(&feeds_folder, BROKER.lock().await.create_sender())?;

    BLOB_STORAGE
        .write()
        .await
        .open(blobs_folder, BROKER.lock().await.create_sender());

    Broker::spawn(actors::ctrlc::actor());

    Broker::spawn(actors::tcp_server::actor(owned_id.clone(), listen));
    if lan_discovery {
        Broker::spawn(actors::lan_discovery::actor(owned_id.clone(), RPC_PORT));
    }

    for (server, port, peer_pk) in connects {
        Broker::spawn(actors::peer::actor(
            owned_id.clone(),
            actors::peer::Connect::TcpServer {
                server,
                port,
                peer_pk,
            },
        ));
    }

    let msgloop = BROKER.lock().await.take_msgloop();
    msgloop.await;

    println!("Gracefully finished");
    Ok(())
}
