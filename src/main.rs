#![recursion_limit = "256"]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;
extern crate get_if_addrs;

use std::{collections::hash_map::HashMap, string::ToString};
use std::time::Duration;

use async_std::{
    fs::File,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    prelude::*,
    sync::{Arc, RwLock},
    task,
};
use futures::{channel::mpsc, channel::oneshot, select, FutureExt, SinkExt};

use async_ctrlc::CtrlC;

use kuska_handshake::async_std::{handshake_server, BoxStream};
use kuska_ssb::{
    api::{msgs::Post, ApiHelper, ApiMethod, CreateHistoryStreamArgs},
    crypto::ToSsbId,
    discovery::ssb_net_id,
    feed::Message,
    keystore::{read_patchwork_config, write_patchwork_config, OwnedIdentity},
    rpc::{RecvMsg, RpcStream},
};

mod storage;
mod actor;

use get_if_addrs::{get_if_addrs, IfAddr};
use storage::Storage;
use actor::*;


const PROCID_ACCEPT_LOOP: usize = 0;
const PROCID_BROADCAST_PEERS: usize = 1;
const PROCID_GENERATOR: usize = 2;
const PROCID_PEERS: usize = 3;
const LISTEN: &str = "0.0.0.0:8008";
const RPC_PORT : u16 = 8008;

lazy_static! {
    static ref DB_RWLOCK: Arc<RwLock<Storage>> = Arc::new(RwLock::new(Storage::default()));
}

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

    DB_RWLOCK.write().await.open(&db_folder)?;

    let (msgloop_sender, msgloop_receiver) = mpsc::unbounded();
    let msgloop = task::spawn(actor_registry_msg_loop(msgloop_receiver));

    spawn_actor(actor_ctrlc_handler(msgloop_sender.clone()));
    spawn_actor(actor_broadcast_peers(
        msgloop_sender.clone(),
        base64::encode(&server_id.pk[..]),
    ));
    spawn_actor(actor_generate_info(
        msgloop_sender.clone(),
        server_id.clone(),
    ));
    spawn_actor(actor_accept_loop(msgloop_sender.clone(), server_id, LISTEN));

    msgloop.await;

    println!("Gracefully finished");
    Ok(())
}

async fn actor_ctrlc_handler(mut msgloop: MsgSender) -> AnyResult<()> {
    let ctrlc = CtrlC::new().expect("cannot create Ctrl+C handler?");
    ctrlc.await;
    println!("Got CTRL-C, sending termination signal to jobs...");
    let _ = msgloop.send(Event::Terminate).await;
    Ok(())
}

async fn actor_generate_info(mut msgloop: MsgSender, server_id: OwnedIdentity) -> AnyResult<()> {
    let (terminate, terminated) = register_actor(&mut msgloop, PROCID_GENERATOR).await?;
    let mut terminate = terminate.fuse();

    loop {
        select_biased! {
          _ = terminate => break,
          _ = task::sleep(Duration::from_secs(5)).fuse() => {
              let db = DB_RWLOCK.write().await;

              let last_msg =  if let Some(last_id) = db.get_feed_len(&server_id.id)? {
                Some(db.get_feed(&server_id.id, last_id)?.into_message()?)
              } else {
                None
              };

              let markdown = format!("Sensor recording... current temperature is {:?}",std::time::SystemTime::now()); 
              let post = Post::new(markdown, Vec::new()).to_msg()?;
              let msg = Message::sign(last_msg.as_ref(), &server_id, post)?;
              info!("Adding {:?}",msg);
              let next_id = db.append_feed(msg)?;

              println!("Recoding sensor data {} ...",next_id);

              drop(db)
          }
        }
    }
    let _ = terminated.send(Void {});
    Ok(())
}

async fn actor_broadcast_peers(mut msgloop: MsgSender, server_pk: String) -> AnyResult<()> {
    let mut packets = Vec::new();

    for if_addr in get_if_addrs()? {
        let addrs = match if_addr.addr {
            IfAddr::V4(v4) if !v4.is_loopback() && v4.broadcast.is_some() => {
                Some((IpAddr::V4(v4.ip), IpAddr::V4(v4.broadcast.unwrap())))
            }
            IfAddr::V6(v6) if !v6.is_loopback() && v6.broadcast.is_some() => {
                Some((IpAddr::V6(v6.ip), IpAddr::V6(v6.broadcast.unwrap())))
            }
            _ => None,
        };

        if let Some((local, broadcast)) = addrs {
            let local_addr = SocketAddr::new(local, RPC_PORT);
            let broadcast_addr = SocketAddr::new(broadcast, RPC_PORT);
            let msg = format!("net:{}:8008~shs:{}", local, server_pk);
            match UdpSocket::bind(SocketAddr::new(local, RPC_PORT)).await {
                Ok(_) => packets.push((local_addr, broadcast_addr, msg)),
                Err(err) => warn!("cannot broadcast to {:?} {:?}", local_addr, err),
            };
        }
    }
    let broadcast_list = packets.iter().map(|(_,broadcast,_)| broadcast.to_string()).collect::<Vec<_>>().join(",");
    info!("broadcast will be sent to {}",broadcast_list);

    let (terminate, terminated) = register_actor(&mut msgloop, PROCID_BROADCAST_PEERS).await?;
    let mut terminate = terminate.fuse();

    loop {
        select_biased! {
          _ = terminate => break,
          _ = task::sleep(Duration::from_secs(1)).fuse() => {
            debug!("sending broadcast");
            for msg in &packets {
                if let Ok(socket) = UdpSocket::bind(msg.0).await {
                    let _ = socket.set_broadcast(true);
                    match socket.send_to(msg.2.as_bytes(),"255.255.255.255:8008").await {
                        Err(err) => debug!("err {}",err),
                        _ => {},
                    }
                }
            }
          }
        }
    }
    let _ = terminated.send(Void {});
    Ok(())
}

async fn actor_accept_loop(
    mut msgloop: MsgSender,
    server_id: OwnedIdentity,
    addr: impl ToSocketAddrs,
) -> AnyResult<()> {
    let (terminate, terminated) = register_actor(&mut msgloop, PROCID_ACCEPT_LOOP).await?;
    let mut terminate = terminate.fuse();

    let listener = TcpListener::bind(addr).await?;
    let mut incoming = listener.incoming();
    let mut actor_id = PROCID_PEERS;
    loop {
        select_biased! {
          _ = terminate => break,
          stream = incoming.next().fuse() => {
            if let Some(stream) = stream {
              let stream = stream?;
              spawn_actor(connection_loop(msgloop.clone(), stream, actor_id, server_id.clone()));
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
    terminate_receiver: SignalReceiver,
) -> AnyResult<()> {
    let mut terminate_receiver = terminate_receiver.fuse();

    // why this is requiered? i cannot remember it :(
    let args = CreateHistoryStreamArgs::new(server_ssb_id.clone());
    let initial_chs_req_id = api.create_history_stream_req_send(&args).await?;

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
                        let msg = DB_RWLOCK.read().await.get_message(&args[0]);
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
                        let last = DB_RWLOCK
                            .read()
                            .await
                            .get_feed_len(&req_id)?
                            .map_or(0, |x| x + 1);

                        let with_keys = req.keys.unwrap_or(true);

                        info!("Sending history stream of {} ({}..{})",req.id,from,last);
                        for n in from..last {
                            let data = DB_RWLOCK.read().await.get_feed(&req_id, n-1)?;
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
    mut msgloop: MsgSender,
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

    let (terminate, _terminated) = register_actor(&mut msgloop, actor_id).await?;

    let res = process_ssb_cmd(api, actor_id, id,  peer_ssb_id, terminate).await;
    if let Err(err) = res {
        warn!("client terminated with error {:?}", err);
    }

    msgloop
        .send(Event::DisconnectProc { actor_id })
        .await
        .unwrap();
    Ok(())
}


