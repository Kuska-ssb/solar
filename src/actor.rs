use futures::{channel::mpsc, channel::oneshot, select, FutureExt, SinkExt};
use async_std::{
    fs::File,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    prelude::*,
    sync::{Arc, RwLock},
    task,
    task::JoinHandle,
};

use std::{collections::hash_map::HashMap, string::ToString};

#[derive(Debug)]
pub struct Void {}

pub type MsgSender = mpsc::UnboundedSender<Event>;
pub type MsgReceiver = mpsc::UnboundedReceiver<Event>;
pub type SignalSender = oneshot::Sender<Void>;
pub type SignalReceiver = oneshot::Receiver<Void>;

pub type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub enum Event {
    NewProc(Actor),
    DisconnectProc { actor_id: usize },
    Terminate,
}

#[derive(Debug)]
pub struct Actor {
    pub actor_id: usize,
    pub terminate: SignalSender,
    pub terminated: SignalReceiver,
}

struct ActorRegistry {
    sender  : MsgSender,
    msgloop : JoinHandle<()>,
}
impl ActorRegistry {
    pub fn new() -> Self {
        let (msgloop_sender, msgloop_receiver) = mpsc::unbounded();
        let msgloop = task::spawn(actor_registry_msg_loop(msgloop_receiver));
        Self {
            sender : msgloop_sender,
            msgloop,
        }
    }
}

pub fn spawn_actor<F>(fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = AnyResult<()>> + Send + 'static,
{
    task::spawn(async move {
        if let Err(e) = fut.await {
            eprintln!("{}", e)
        }
    })
}

pub async fn register_actor(
    msgloop: &mut MsgSender,
    actor_id: usize,
) -> AnyResult<(SignalReceiver, SignalSender)> {
    let (terminate_sender, terminate_receiver) = oneshot::channel::<Void>();
    let (terminated_sender, terminated_receiver) = oneshot::channel::<Void>();

    let peer = Actor {
        actor_id,
        terminate: terminate_sender,
        terminated: terminated_receiver,
    };

    msgloop.send(Event::NewProc(peer)).await.unwrap();

    Ok((terminate_receiver, terminated_sender))
}

pub async fn actor_registry_msg_loop(mut events: MsgReceiver) {
    let mut peers: HashMap<usize, Actor> = HashMap::new();

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
                debug!("Registering peer {}", peer.actor_id);
                peers.insert(peer.actor_id, peer);
            }
            Event::DisconnectProc { actor_id } => {
                debug!("Unregistering peer {}", actor_id);
                peers.remove(&actor_id);
            }
        }
    }

    // send a termination signal
    let (terms, termds): (Vec<_>, Vec<_>) = peers
        .drain()
        .map(|(_, peer)| {
            (
                (peer.actor_id, peer.terminate),
                (peer.actor_id, peer.terminated),
            )
        })
        .unzip();

    for (actor_id, term) in terms {
        debug!("Sending term signal to {}", actor_id);
        term.send(Void {}).unwrap();
    }

    // wait to be finished
    for (actor_id, termd) in termds {
        debug!("Waiting termd signal from {}", actor_id);
        let _ = termd.await;
    }
    drop(peers);
}

