use std::time::Duration;
use once_cell::sync::Lazy;
use std::collections::HashSet;

use async_std::{
    sync::{Arc,RwLock},
    io::{Read, Write},
    net::TcpStream,
    prelude::*,
    task,
};

use futures::{select, FutureExt, SinkExt};

use kuska_handshake::{
    HandshakeComplete,
    async_std::{
        handshake_server,
        handshake_client,
        BoxStream
    }    
};

use kuska_ssb::{
    discovery::ssb_net_id,
    api::ApiHelper,
    crypto::{ed25519,ToSsbId},
    rpc::RpcStream,
    keystore::OwnedIdentity,
};

use crate::broker::*;
use crate::error::SolarResult;
use crate::storage::kv::ChStoRecv;

use super::rpc::{
    BlobsHandler, GetHandler, HistoryStreamHandler, RpcHandler, RpcInput, WhoAmIHandler,
};

pub enum Connect {
    Announce{ server: String, port: u32, peer_pk: ed25519::PublicKey },
    Recieved{ stream: TcpStream }
}

pub static CONNECTED_PEERS: Lazy<Arc<RwLock<HashSet<ed25519::PublicKey>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashSet::new())));

pub async fn actor(
    id: OwnedIdentity,
    connect: Connect,
) -> SolarResult<()> {

    if let Err(err) = actor_inner(id,connect).await {
        warn!("peer failed: {:?}", err);
    } 
    Ok(())
}

pub async fn actor_inner(
    id: OwnedIdentity,
    connect: Connect,
) -> SolarResult<()> {

    let OwnedIdentity {pk,sk,..} = id;
    let (stream,handshake) = match connect {
        Connect::Announce{server,port,peer_pk} => {
            if CONNECTED_PEERS.read().await.contains(&peer_pk) {
                return Ok(());
            }
            let server_port = format!("{}:{}", server, port);
            let mut stream = TcpStream::connect(server_port).await?;
            let handshake = handshake_client(&mut stream, ssb_net_id(), pk, sk, peer_pk).await?; 
            info!("💃 connected to peer {}", handshake.peer_pk.to_ssb_id());

            (stream,handshake)    
        }
        Connect::Recieved{mut stream} => {
            let handshake = handshake_server(&mut stream, ssb_net_id(), pk, sk).await?;
            if CONNECTED_PEERS.read().await.contains(&handshake.peer_pk) {
                return Ok(());
            }
            info!("💃 recieved connection from peer {}", handshake.peer_pk.to_ssb_id());

            (stream,handshake)
        }
    };
    
    let ActorEndpoint {
        ch_terminate,
        mut ch_broker,
        ch_storage,
        actor_id,
        ..
    } = BROKER.lock().await.register("peer", true).await?;

    let peer_pk = handshake.peer_pk.clone();
    CONNECTED_PEERS.write().await.insert(peer_pk.clone());
    let res = peer_loop(&stream, &stream,handshake, ch_terminate, ch_storage.unwrap()).await;
    CONNECTED_PEERS.write().await.remove(&peer_pk);

    if let Err(err) = res {
        warn!("💀 client terminated with error {:?}", err);
    } else {
        info!("👋 finished connection with {}", &peer_pk.to_ssb_id());
    }

    let _ = ch_broker
        .send(BrokerEvent::Disconnect { actor_id })
        .await;

    Ok(())

}

async fn peer_loop<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync>(
    reader: R,
    writer: W,
    handshake: HandshakeComplete,
    ch_terminate: ChSigRecv,
    mut ch_storage: ChStoRecv
) -> SolarResult<()> {
    let peer_ssb_id = handshake.peer_pk.to_ssb_id();

    let (box_stream_read, box_stream_write) =
        BoxStream::from_handshake(reader, writer, handshake, 0x8000).split_read_write();

    let rpc = RpcStream::new(box_stream_read, box_stream_write);
    let mut api = ApiHelper::new(rpc);

    let mut history_stream_handler = HistoryStreamHandler::default();
    let mut whoami_handler = WhoAmIHandler::new(&peer_ssb_id);
    let mut get_handler = GetHandler::default();
    let mut blobs_handler = BlobsHandler::default();

    let mut handlers: Vec<&mut dyn RpcHandler<R, W>> = vec![
        &mut history_stream_handler,
        &mut whoami_handler,
        &mut get_handler,
        &mut blobs_handler,
    ];

    let mut ch_terminate = ch_terminate.fuse();
    loop {
        let input = select! {
          value = ch_terminate =>  {
            break;
          },
          msg = api.rpc().recv().fuse() => {
            let (rpc_id, msg) = msg?;
            RpcInput::Network(rpc_id,msg)
          },
          id = ch_storage.next().fuse() => {
            if let Some(id) = id {
                RpcInput::Storage(id)
            } else {
                RpcInput::None
            }
          },
          _ = task::sleep(Duration::from_secs(1)).fuse() => {
            RpcInput::Timer
          },
        };
        let mut handled = false;
        for handler in handlers.iter_mut() {
            match handler.handle(&mut api, &input).await {
                Ok(has_been_handled) => {
                    if has_been_handled {
                        handled = true;
                        break;
                    }
                }
                Err(err) => {
                    error!("handler {} failed with {:?}", handler.name(), err);
                }
            }
        }
        if !handled {
            trace!("message not processed: {:?}", input);
        }
    }
    Ok(())
}