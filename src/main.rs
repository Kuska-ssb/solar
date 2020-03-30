#![recursion_limit = "256"]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;
extern crate get_if_addrs;


use async_std::{
    fs::File,
};

use kuska_ssb::{
    keystore::{read_patchwork_config, write_patchwork_config, OwnedIdentity},
};

mod storage;
mod registry;
mod actors;

use storage::DB;
use registry::*;

const LISTEN: &str = "0.0.0.0:8008";
const RPC_PORT: u16 = 8008;

#[async_std::main]
async fn main() -> AnyResult<()> {
    env_logger::init();
    log::set_max_level(log::LevelFilter::max());

    let base_path = xdg::BaseDirectories::new()?.create_data_directory("solar")?;

    println!("Base configuration is {:?}", base_path);

    let mut key_file = base_path.clone();
    let mut db_folder = base_path;

    key_file.push("secret");
    db_folder.push("db");

    let server_id = if !key_file.is_file() {
        println!("Private key not found, generated new one in {:?}", key_file);

        let id = OwnedIdentity::create();
        let mut file = File::create(key_file).await?;
        write_patchwork_config(&id, &mut file).await?;
        id
    } else {
        let mut file = File::open(key_file).await?;
        read_patchwork_config(&mut file).await?
    };

    println!("Server started on {}:{}", LISTEN, base64::encode(&server_id.pk[..]));

    DB.write().await.open(&db_folder)?;

    let reg = Registry::new();

    Registry::spawn(actors::ctrlc::actor(reg.channel()));
    Registry::spawn(actors::landiscovery::actor(
        reg.channel(),
        base64::encode(&server_id.pk[..]),
        RPC_PORT
    ));
    Registry::spawn(actors::sensor::actor(
        reg.channel(),
        server_id.clone(),
    ));
    Registry::spawn(actors::sbot::actor(reg.channel(), server_id, LISTEN));

    reg.join().await;

    println!("Gracefully finished");
    Ok(())
}
