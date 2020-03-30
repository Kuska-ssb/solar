use std::{string::ToString};

use async_std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
};
use futures::{select, FutureExt, SinkExt};

use kuska_handshake::async_std::{handshake_server, BoxStream};
use kuska_ssb::{
    api::{ApiHelper, ApiMethod, CreateHistoryStreamArgs},
    crypto::ToSsbId,
    discovery::ssb_net_id,
    keystore::OwnedIdentity,
    rpc::{RecvMsg, RpcStream},
};

use crate::storage::DB;
use crate::registry::*;

const ACTOR_ACCEPT_LOOP: usize = 0;
const ACTOR_PEERS: usize = 3;

pub async fn actor(
    mut registry_ch: RegistryCh,
    server_id: OwnedIdentity,
    addr: impl ToSocketAddrs,
) -> AnyResult<()> {
    let (terminate, terminated) = Registry::register(&mut registry_ch, ACTOR_ACCEPT_LOOP).await?;
    let mut terminate = terminate.fuse();

    let listener = TcpListener::bind(addr).await?;
    let mut incoming = listener.incoming();
    let mut actor_id = ACTOR_PEERS;
    loop {
        select_biased! {
          _ = terminate => break,
          stream = incoming.next().fuse() => {
            if let Some(stream) = stream {
              let stream = stream?;
              Registry::spawn(connection_loop(registry_ch.clone(), stream, actor_id, server_id.clone()));
              actor_id += 1;
            } else {
              break;
            }
          },
        }
    }
    let _ = terminated.send(Void {});
    Ok(())
}

async fn process_ssb_cmd<R: Read + Unpin, W: Write + Unpin>(
    mut api: ApiHelper<R, W>,
    _actor_id: usize,
    server_ssb_id: String,
    peer_ssb_id: String,
    terminate_receiver: SigTerminate,
) -> AnyResult<()> {
    let mut terminate_receiver = terminate_receiver.fuse();

    // why this is requiered? i cannot remember it :(
    let args = CreateHistoryStreamArgs::new(server_ssb_id.clone());
    let _initial_chs_req_id = api.create_history_stream_req_send(&args).await?;

    loop {
        let res = select! {
          msg = api.rpc().recv().fuse() => msg,
          value = terminate_receiver =>  {
            break;
          }
        };

        let (rpc_id, msg) = res?;

        match msg {
            RecvMsg::RpcRequest(req) => {
                let selector = req.name.iter().map(|v| v.as_str()).collect::<Vec<_>>();
                match ApiMethod::from_selector(&selector) {
                    Some(ApiMethod::WhoAmI) => {
                        api.whoami_res_send(rpc_id, peer_ssb_id.clone()).await?;
                    }
                    Some(ApiMethod::Get) => {
                        let args: Vec<String> = serde_json::from_value(req.args)?;
                        let msg = DB.read().await.get_message(&args[0]);
                        match msg {
                            Ok(msg) => api.get_res_send(rpc_id, &msg).await?,
                            Err(err) => {
                                let msg = format!("{}", err);
                                api.rpc().send_error(rpc_id, req.rpc_type, &msg).await?
                            }
                        };
                    }
                    Some(ApiMethod::CreateHistoryStream) => {
                        let mut args: Vec<CreateHistoryStreamArgs> =
                            serde_json::from_value(req.args)?;

                        let req = args.pop().unwrap();
                        let req_id = if req.id.starts_with('@') {
                            req.id.clone()
                        } else {
                            format!("@{}", req.id).to_string()
                        };

                        let from = req.seq.unwrap_or(1u64);
                        let last = DB
                            .read()
                            .await
                            .get_feed_len(&req_id)?
                            .map_or(0, |x| x + 1);

                        let with_keys = req.keys.unwrap_or(true);

                        info!("Sending history stream of {} ({}..{})",req.id,from,last);
                        for n in from..last {
                            let data = DB.read().await.get_feed(&req_id, n-1)?;
                            let data =if with_keys {
                                data.to_string()
                            } else {
                                data.value.to_string()
                            };
                            info!(" - [with_keys={}]{}",with_keys,&data.to_string());
                            api.feed_res_send(rpc_id, &data).await?;    
                        }
                        if !req.live.unwrap_or(false) {
                            api.rpc().send_stream_eof(rpc_id).await?;
                        }
                    }
                    _ => {
                        warn!("Unknown method requested {:?}, erroring to client", selector);
                        api.rpc().send_error(rpc_id, req.rpc_type, "unknown method").await?;
                    }
                }
            }
            RecvMsg::ErrorResponse(err) => {
                warn!("error {}", err);
            }
            RecvMsg::CancelStreamRespose() => {
                api.rpc().send_stream_eof(-rpc_id).await?;
            }
            RecvMsg::OtherRequest(_type,body) => {
                warn!("recieved unknown OtherRequest '{}'", String::from_utf8_lossy(&body));
            }
            RecvMsg::RpcResponse(_type, body) => {
                warn!("recieved unknown RpcResponse '{}'", String::from_utf8_lossy(&body));
            }
        }
    }

    Ok(())
}

async fn connection_loop(
    mut registry_ch: RegistryCh,
    mut stream: TcpStream,
    actor_id: usize,
    server_id: OwnedIdentity,
) -> AnyResult<()> {
    /* perform handshake ----------------------------------------------------------------------------- */

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

    /* perform handshake ----------------------------------------------------------------------------- */

    let rpc = RpcStream::new(box_stream_read, box_stream_write);
    let api = ApiHelper::new(rpc);

    let (terminate, _terminated) = Registry::register(&mut registry_ch, actor_id).await?;

    let res = process_ssb_cmd(api, actor_id, id,  peer_ssb_id, terminate).await;
    if let Err(err) = res {
        warn!("client terminated with error {:?}", err);
    }

    registry_ch
        .send(Event::Disconnect { actor_id })
        .await
        .unwrap();
    Ok(())
}