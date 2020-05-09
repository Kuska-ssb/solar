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
use futures::stream::StreamExt;

use async_stream::stream;
use futures::{FutureExt, SinkExt};

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
    rpc::{RpcStreamReader,RpcStreamWriter},
    keystore::OwnedIdentity,
};

use crate::broker::*;
use crate::error::SolarResult;

use super::rpc::{
    BlobsGetHandler, BlobsWantsHandler, GetHandler, HistoryStreamHandler, RpcHandler, RpcInput, WhoAmIHandler,
};

pub enum Connect {
    TcpServer { server: String, port: u32, peer_pk: ed25519::PublicKey },
    ClientStream { stream: TcpStream }
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
        Connect::TcpServer{server,port,peer_pk} => {
            if CONNECTED_PEERS.read().await.contains(&peer_pk) {
                return Ok(());
            }
            let server_port = format!("{}:{}", server, port);
            let mut stream = TcpStream::connect(server_port).await?;
            let handshake = handshake_client(&mut stream, ssb_net_id(), pk, sk, peer_pk).await?; 
            info!("💃 connected to peer {}", handshake.peer_pk.to_ssb_id());

            (stream,handshake)    
        }
        Connect::ClientStream{mut stream} => {
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
        ch_msg,
        actor_id,
        ..
    } = BROKER.lock().await.register("peer", true).await?;

    let peer_pk = handshake.peer_pk;
    CONNECTED_PEERS.write().await.insert(peer_pk);
    let res = peer_loop(actor_id, &stream, &stream,handshake, ch_terminate, ch_msg.unwrap()).await;
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
    actor_id : usize,
    reader: R,
    writer: W,
    handshake: HandshakeComplete,
    ch_terminate: ChSigRecv,
    mut ch_msg: ChMsgRecv
) -> SolarResult<()> {
    let peer_ssb_id = handshake.peer_pk.to_ssb_id();

    let (box_stream_read, box_stream_write) =
        BoxStream::from_handshake(reader, writer, handshake, 0x8000).split_read_write();

    let mut rpc_reader = RpcStreamReader::new(box_stream_read);
    let rpc_writer = RpcStreamWriter::new(box_stream_write);
    let mut api = ApiHelper::new(rpc_writer);

    let mut history_stream_handler = HistoryStreamHandler::new(actor_id);
    let mut whoami_handler = WhoAmIHandler::new(&peer_ssb_id);
    let mut get_handler = GetHandler::default();
    let mut blobs_get_handler = BlobsGetHandler::default();
    let mut blobs_wants_handler = BlobsWantsHandler::default();

    let mut handlers: Vec<&mut dyn RpcHandler<W>> = vec![
        &mut history_stream_handler,
        &mut whoami_handler,
        &mut get_handler,
        &mut blobs_get_handler,
        &mut blobs_wants_handler,
    ];

    let mut ch_broker = BROKER.lock().await.create_sender();
    let mut ch_terminate_fuse = ch_terminate.fuse();

    let rpc_recv_fuse = stream! {
        while let Ok(v) = rpc_reader.recv().await {
            yield v            
        }
    };
    pin_mut!(rpc_recv_fuse);

    loop {
        let input = select_biased! {
          value = ch_terminate_fuse =>  {
            break;
          },
          packet = rpc_recv_fuse.select_next_some() => {
            let (rpc_id, packet) = packet;
            RpcInput::Network(rpc_id,packet)
          },
          msg = ch_msg.next().fuse() => {
            if let Some(msg) = msg {
                RpcInput::Message(msg)
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
            match handler.handle(&mut api, &input, &mut ch_broker).await {
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