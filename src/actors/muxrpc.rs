use async_std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
};
use futures::{select, FutureExt, SinkExt};

use kuska_handshake::async_std::{handshake_server, BoxStream};
use kuska_ssb::{
    api::{ApiHelper, CreateHistoryStreamArgs},
    crypto::ToSsbId,
    discovery::ssb_net_id,
    keystore::OwnedIdentity,
    rpc::RpcStream,
};

use crate::broker::*;
use crate::error::SolarResult;
use crate::storage::ChStoRecv;

use super::rpcs::{GetHandler, HistoryStreamHandler, RpcHandler, RpcInput, WhoAmIHandler};

pub async fn actor(server_id: OwnedIdentity, addr: impl ToSocketAddrs) -> SolarResult<()> {
    let broker = BROKER.lock().await.register("sbot-listener", false).await?;

    let mut ch_terminate = broker.ch_terminate.fuse();

    let listener = TcpListener::bind(addr).await?;
    let mut incoming = listener.incoming();

    loop {
        select_biased! {
          _ = ch_terminate => break,
          stream = incoming.next().fuse() => {
            if let Some(stream) = stream {
              let stream = stream?;
              Broker::spawn(handle_connection(stream, server_id.clone()));
            } else {
              break;
            }
          },
        }
    }
    let _ = broker.ch_terminated.send(Void {});
    Ok(())
}

async fn handle_connection(mut stream: TcpStream, server_id: OwnedIdentity) -> SolarResult<()> {
    let broker = BROKER.lock().await.register("sbot-instance", true).await?;

    let OwnedIdentity {
        pk: server_pk,
        sk: server_sk,
        id,
    } = server_id.clone();
    let handshake =
        handshake_server(&mut stream, ssb_net_id(), server_pk, server_sk.clone()).await?;
    let peer_ssb_id = handshake.peer_pk.to_ssb_id();
    info!("💃 handshake complete, user {}", &peer_ssb_id);

    let (box_stream_read, box_stream_write) =
        BoxStream::from_handshake(&stream, &stream, handshake, 0x8000).split_read_write();

    let rpc = RpcStream::new(box_stream_read, box_stream_write);
    let mut api = ApiHelper::new(rpc);

    let ActorEndpoint {
        ch_terminate,
        mut ch_broker,
        ch_storage,
        actor_id,
        ..
    } = broker;
    let res = sbot_loop(ch_terminate, ch_storage.unwrap(), &mut api, id, peer_ssb_id).await;

    if let Err(err) = res {
        warn!("client terminated with error {:?}", err);
    }

    ch_broker
        .send(BrokerEvent::Disconnect { actor_id })
        .await
        .unwrap();
    Ok(())
}

async fn sbot_loop<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync>(
    ch_terminate: ChSigRecv,
    mut ch_storage: ChStoRecv,
    api: &mut ApiHelper<R, W>,
    server_ssb_id: String,
    peer_ssb_id: String,
) -> SolarResult<()> {
    let mut ch_terminate = ch_terminate.fuse();

    let mut history_stream_handler = HistoryStreamHandler::default();
    let mut whoami_handler = WhoAmIHandler::new(&peer_ssb_id);
    let mut get_handler = GetHandler::default();

    let mut handlers: Vec<&mut dyn RpcHandler<R, W>> = vec![
        &mut history_stream_handler,
        &mut whoami_handler,
        &mut get_handler,
    ];

    // why this is requiered? i cannot remember it :(
    let args = CreateHistoryStreamArgs::new(server_ssb_id.clone());
    let _initial_chs_req_id = api.create_history_stream_req_send(&args).await?;

    loop {
        let input = select! {
          msg = api.rpc().recv().fuse() => {
            let (rpc_id, msg) = msg?;
            RpcInput::Network(rpc_id,msg)
          },
          id = ch_storage.next().fuse() => {
            if let Some(id) = id {
                info!("got user change : {:?}",id);
                RpcInput::Storage(id)
            } else {
                RpcInput::None
            }
          },
          value = ch_terminate =>  {
            break;
          }
        };
        let mut handled = false;
        for handler in handlers.iter_mut() {
            if handler.handle(api, &input).await? {
                handled = true;
                break;
            }
        }
        if !handled {
            trace!("message not processed: {:?}", input);
        }
    }

    Ok(())
}
