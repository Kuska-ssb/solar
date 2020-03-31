use std::string::ToString;

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

use crate::registry::*;
use crate::storage::DB;

pub async fn actor(server_id: OwnedIdentity, addr: impl ToSocketAddrs) -> AnyResult<()> {
    let reg = REGISTRY.lock().await.register("sbot-listener",false).await?;

    let mut ch_terminate = reg.ch_terminate.fuse();

    let listener = TcpListener::bind(addr).await?;
    let mut incoming = listener.incoming();
    loop {
        select_biased! {
          _ = ch_terminate => break,
          stream = incoming.next().fuse() => {
            if let Some(stream) = stream {
              let stream = stream?;
              Registry::spawn(handle_connection(stream, server_id.clone()));
            } else {
              break;
            }
          },
        }
    }
    let _ = reg.ch_terminated.send(Void {});
    Ok(())
}

async fn handle_connection(mut stream: TcpStream, server_id: OwnedIdentity) -> AnyResult<()> {
    let reg = REGISTRY.lock().await.register("sbot-instance", true).await?;

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
    let api = ApiHelper::new(rpc);

    let ActorEndpoint {
        ch_terminate,
        mut ch_registry,
        mut ch_storage,
        actor_id,
        ..
    } = reg;

    let res = sbot_loop(
        ch_terminate,ch_storage.unwrap(),
        api, id, peer_ssb_id).await;
    if let Err(err) = res {
        warn!("client terminated with error {:?}", err);
    }

    ch_registry.send(Event::Disconnect { actor_id }).await.unwrap();
    Ok(())
}

async fn sbot_loop<R: Read + Unpin, W: Write + Unpin>(
    ch_terminate: ChSigRecv,
    mut ch_storage: ChStoRecv,
    mut api: ApiHelper<R, W>,
    server_ssb_id: String,
    peer_ssb_id: String,
) -> AnyResult<()> {
    let mut ch_terminate = ch_terminate.fuse();

    // why this is requiered? i cannot remember it :(
    let args = CreateHistoryStreamArgs::new(server_ssb_id.clone());
    let _initial_chs_req_id = api.create_history_stream_req_send(&args).await?;

    loop {
        let res = select! {
          wire_msg = api.rpc().recv().fuse() => wire_msg,
          storage_msg = ch_storage.next().fuse() => {
            info!("got storage notification: {:?}",storage_msg);
            break; 
          },
          value = ch_terminate =>  {
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
                        let last = DB.read().await.get_feed_len(&req_id)?.map_or(0, |x| x + 1);

                        let with_keys = req.keys.unwrap_or(true);

                        info!("Sending history stream of {} ({}..{})", req.id, from, last);
                        for n in from..last {
                            let data = DB.read().await.get_feed(&req_id, n - 1)?;
                            let data = if with_keys {
                                data.to_string()
                            } else {
                                data.value.to_string()
                            };
                            info!(" - [with_keys={}]{}", with_keys, &data.to_string());
                            api.feed_res_send(rpc_id, &data).await?;
                        }
                        if !req.live.unwrap_or(false) {
                            api.rpc().send_stream_eof(rpc_id).await?;
                        }
                    }
                    _ => {
                        debug!(
                            "Unknown method requested {:?}, erroring to client",
                            selector
                        );
                        api.rpc()
                            .send_error(rpc_id, req.rpc_type, "unknown method")
                            .await?;
                    }
                }
            }
            RecvMsg::ErrorResponse(err) => {
                warn!("error {}", err);
            }
            RecvMsg::CancelStreamRespose() => {
                api.rpc().send_stream_eof(-rpc_id).await?;
            }
            RecvMsg::OtherRequest(_type, body) => {
                debug!(
                    "recieved unknown OtherRequest '{}'",
                    String::from_utf8_lossy(&body)
                );
            }
            RecvMsg::RpcResponse(_type, body) => {
                debug!(
                    "recieved unknown RpcResponse '{}'",
                    String::from_utf8_lossy(&body)
                );
            }
        }
    }

    Ok(())
}

