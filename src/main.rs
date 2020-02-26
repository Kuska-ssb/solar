#![recursion_limit = "256"]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;

use std::{
  collections::hash_map::HashMap,
  string::ToString
};

use std::time::Duration;

use futures::{channel::mpsc, channel::oneshot, select, FutureExt, SinkExt};
use async_std::{
  prelude::*,
  io::{Read,Write},
  net::{TcpListener, TcpStream, ToSocketAddrs},
  sync::{Arc, RwLock},
  task,
};

use async_ctrlc::CtrlC;

use kuska_handshake::async_std::{handshake_server, BoxStream};
use kuska_ssb::{
  crypto::ToSsbId,
  feed::Message,
  keystore::{OwnedIdentity,from_patchwork_local},
  rpc::{RecvMsg, RpcStream},
  discovery::ssb_net_id,
  api::{ApiHelper,ApiMethod,CreateHistoryStreamArgs,
    msgs::Post,
  }
};

mod storage;
mod error;

use error::AnyResult;
use storage::Storage;

type MsgSender = mpsc::UnboundedSender<Event>;
type MsgReceiver = mpsc::UnboundedReceiver<Event>;
type SignalSender = oneshot::Sender<Void>;
type SignalReceiver = oneshot::Receiver<Void>;

#[derive(Debug)]
struct Void {}

const PROCID_ACCEPT_LOOP: usize = 0;
const PROCID_GENERATOR: usize = 1;
const PROCID_PEERS: usize = 2;

lazy_static! {
  static ref DB_RWLOCK: Arc<RwLock<Storage>> = Arc::new(RwLock::new(Storage::default()));
}

#[async_std::main]
async fn main() -> AnyResult<()> {
  info!("Server started");
  env_logger::init();
  log::set_max_level(log::LevelFilter::max());

  let server_id = from_patchwork_local().expect("read local secret");
  let mut db = DB_RWLOCK.write().await;
  db.open()?;
  drop(db);

  let (msgloop_sender, msgloop_receiver) = mpsc::unbounded();
  let msgloop = task::spawn(msg_loop(msgloop_receiver));

  task::spawn(proc_control_c_handler(msgloop_sender.clone()));
  spawn_and_log_error(proc_generate_info(
    msgloop_sender.clone(),
    server_id.clone(),
  ));
  spawn_and_log_error(proc_accept_loop(
    msgloop_sender.clone(),
    server_id,
    "127.0.0.1:8080",
  ));

  msgloop.await;
  Ok(())
}

async fn proc_control_c_handler(mut msgloop: MsgSender) {
  let ctrlc = CtrlC::new().expect("cannot create Ctrl+C handler?");
  ctrlc.await;
  info!("GOT CRTL-C");
  let _ = msgloop.send(Event::Terminate).await;
}

async fn proc_generate_info(mut msgloop: MsgSender, server_id: OwnedIdentity) -> AnyResult<()> {
  let (terminate, terminated) = register_proc(&mut msgloop, PROCID_GENERATOR).await?;
  let mut terminate = terminate.fuse();

  loop {
    select_biased! {
      _ = terminate => {
          break;
      }
      _ = task::sleep(Duration::from_secs(5)).fuse() => {
          let db = DB_RWLOCK.write().await;

          let last_msg =  if let Some(last_id) = db.get_feed_len(&server_id.id)? {
            Some(db.get_feed(&server_id.id, last_id)?.into_message()?)
          } else {
            None
          };

          let post = Post::new("test1".to_string(), None).to_msg()?;
          let msg = Message::sign(last_msg.as_ref(), &server_id, post)?;
          let next_id = db.append_feed(msg)?;

          println!("Recoding sensor data {} ...",next_id);
 
          drop(db)
      }
    }
  }
  let _ = terminated.send(Void {});
  Ok(())
}

async fn proc_accept_loop(
  mut msgloop: MsgSender,
  server_id: OwnedIdentity,
  addr: impl ToSocketAddrs,
) -> AnyResult<()> {
  let (terminate, terminated) = register_proc(&mut msgloop, PROCID_ACCEPT_LOOP).await?;
  let mut terminate = terminate.fuse();

  let listener = TcpListener::bind(addr).await?;
  let mut incoming = listener.incoming();
  let mut proc_id = PROCID_PEERS;
  loop {
    select_biased! {
      _ = terminate => {
        break;
      }
      stream = incoming.next().fuse() => {
        if let Some(stream) = stream {
          let stream = stream?;
          spawn_and_log_error(connection_loop(msgloop.clone(), stream, proc_id, server_id.clone()));
          proc_id += 1;
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
  _proc_id: usize,
  peer_ssb_id: String,
  terminate_receiver: SignalReceiver,
) -> AnyResult<()> {
  let mut terminate_receiver = terminate_receiver.fuse();

  loop {
    let res = select! {
      msg = api.rpc().recv().fuse() => msg,
      value = terminate_receiver =>  {
        println!("{:?}",value);
        break;
      }
    };

    let (rpc_id, msg) = res?;

    match msg {
      RecvMsg::Request(req) => {
        let selector = req.name.iter().map(|v| v.as_str()).collect::<Vec<_>>();
        match ApiMethod::from_selector(&selector) {
          Some(ApiMethod::WhoAmI) => {
            api.whoami_res_send(rpc_id, peer_ssb_id.clone()).await?;
          }
          Some(ApiMethod::Get) => {
            let args: Vec<String> = serde_json::from_value(req.args)?;
            let msg = DB_RWLOCK.read().await.get_message(&args[0]);
            match msg {
              Ok(msg) => api.get_res_send(rpc_id,&msg).await?,
              Err(err) => {
                let msg = format!("{}",err);
                api.rpc().send_error(rpc_id, &msg).await?
              }
            };
          }
          Some(ApiMethod::CreateHistoryStream) => {
            let mut args: Vec<CreateHistoryStreamArgs> = serde_json::from_value(req.args)?;
            let req = args.pop().unwrap();
            let req_id = if req.id.starts_with("@") {
              req.id
            } else {
              format!("@{}", req.id).to_string()
            };

            let from = req.seq.unwrap_or(0u64);
            let last = DB_RWLOCK.read().await.get_feed_len(&req_id)?.map_or(0,|x|x+1);

            for n in from..last {
              let data = DB_RWLOCK.read().await.get_feed(&req_id,n)?;
              api.feed_res_send(rpc_id, &data.to_string()).await?;
            }           

            api.rpc().send_stream_eof(rpc_id).await?;
          }
          _ => {
            warn!("Unknown method requested {:?}", selector);
            api.rpc().send_error(rpc_id, "unknown method").await?;
          }
        }
      }
      RecvMsg::ErrorResponse(err) => {
        println!("error {}", err);
      }
      RecvMsg::CancelStreamRespose() => {}
      RecvMsg::BodyResponse(body) => {
        println!("recieved {}", String::from_utf8_lossy(&body));
      }
    }
  }

  Ok(())
}

async fn register_proc(
  msgloop: &mut MsgSender,
  proc_id: usize,
) -> AnyResult<(SignalReceiver, SignalSender)> {
  let (terminate_sender, terminate_receiver) = oneshot::channel::<Void>();
  let (terminated_sender, terminated_receiver) = oneshot::channel::<Void>();

  let peer = Proc {
    proc_id,
    terminate: terminate_sender,
    terminated: terminated_receiver,
  };

  msgloop.send(Event::NewProc(peer)).await.unwrap();

  Ok((terminate_receiver, terminated_sender))
}

async fn connection_loop(
  mut msgloop: MsgSender,
  mut stream: TcpStream,
  proc_id: usize,
  server_id: OwnedIdentity,
) -> AnyResult<()> {

  /* perform handshake ----------------------------------------------------------------------------- */

  let OwnedIdentity {
    pk: server_pk,
    sk: server_sk,
    ..
  } = server_id.clone();
  let handshake = handshake_server(&mut stream, ssb_net_id(), server_pk, server_sk.clone()).await?;
  let peer_ssb_id = handshake.peer_pk.to_ssb_id();
  info!("💃 handshake complete, user {}", &peer_ssb_id);

  let (box_stream_read, box_stream_write) =
    BoxStream::from_handshake(&stream, &stream, handshake, 0x8000).split_read_write();

  /* perform handshake ----------------------------------------------------------------------------- */

  let rpc = RpcStream::new(box_stream_read, box_stream_write);
  let api = ApiHelper::new(rpc);

  let (terminate, _terminated) = register_proc(&mut msgloop, proc_id).await?;

  let res = process_ssb_cmd(api, proc_id, peer_ssb_id, terminate).await;
  if let Err(err) = res {
    warn!("client terminated with error {:?}", err);
  }

  msgloop
    .send(Event::DisconnectProc { proc_id })
    .await
    .unwrap();
  Ok(())
}

#[derive(Debug)]
enum Event {
  NewProc(Proc),
  DisconnectProc { proc_id: usize },
  Terminate,
}

#[derive(Debug)]
struct Proc {
  proc_id: usize,
  terminate: SignalSender,
  terminated: SignalReceiver,
}

async fn msg_loop(mut events: MsgReceiver) {
  let mut peers: HashMap<usize, Proc> = HashMap::new();

  loop {
    let event = select! {
        event = events.next().fuse() => match event {
            None => break,
            Some(event) => event,
        },
    };
    match event {
      Event::Terminate => {
        info!("Msg Got terminate ");
        break;
      }
      Event::NewProc(peer) => {
        debug!("Registering peer {}", peer.proc_id);
        peers.insert(peer.proc_id, peer);
      }
      Event::DisconnectProc { proc_id } => {
        debug!("Unregistering peer {}", proc_id);
        peers.remove(&proc_id);
      }
    }
  }

  // send a termination signal
  let (terms, termds): (Vec<_>, Vec<_>) = peers
    .drain()
    .map(|(_, peer)| {
      (
        (peer.proc_id, peer.terminate),
        (peer.proc_id, peer.terminated),
      )
    })
    .unzip();

  for (proc_id, term) in terms {
    debug!("Sending term signal to {}", proc_id);
    term.send(Void {}).unwrap();
  }

  // wait to be finished
  for (proc_id, termd) in termds {
    debug!("Waiting termd signal from {}", proc_id);
    let _ = termd.await;
  }
  drop(peers);
}

fn spawn_and_log_error<F>(fut: F) -> task::JoinHandle<()>
where
  F: Future<Output = AnyResult<()>> + Send + 'static,
{
  task::spawn(async move {
    if let Err(e) = fut.await {
      eprintln!("{}", e)
    }
  })
}