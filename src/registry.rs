use futures::{channel::mpsc, channel::oneshot, select, FutureExt, SinkExt};
use async_std::{
    prelude::*,
    task,
    task::JoinHandle,
};

use std::{collections::hash_map::HashMap};

#[derive(Debug)]
pub struct Void {}

pub type RegistryCh = mpsc::UnboundedSender<Event>;
pub type SigTerminated = oneshot::Sender<Void>;
pub type SigTerminate = oneshot::Receiver<Void>;

pub type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub enum Event {
    Connect(Actor),
    Disconnect { actor_id: usize },
    Terminate,
}

#[derive(Debug)]
pub struct Actor {
    pub actor_id: usize,
    pub terminate: SigTerminated,
    pub terminated: SigTerminate,
}

pub struct Registry {
    sender  : RegistryCh,
    msgloop : JoinHandle<()>,
}

impl Registry {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded();
        let msgloop = task::spawn(Self::msg_loop(receiver));
        Self {
            sender,
            msgloop,
        }
    }
    pub fn channel(&self) -> RegistryCh {
        self.sender.clone()
    }
    pub async fn join(self) {
        self.msgloop.await;
    } 
    pub async fn register(sender : &mut RegistryCh, actor_id: usize) -> AnyResult<(SigTerminate, SigTerminated)> {
        let (terminate_sender, terminate_receiver) = oneshot::channel::<Void>();
        let (terminated_sender, terminated_receiver) = oneshot::channel::<Void>();
    
        let actor = Actor {
            actor_id,
            terminate: terminate_sender,
            terminated: terminated_receiver,
        };
    
        sender.send(Event::Connect(actor)).await.unwrap();
    
        Ok((terminate_receiver, terminated_sender))
    }
    pub fn spawn<F>(fut: F) -> task::JoinHandle<()>
    where
        F: Future<Output = AnyResult<()>> + Send + 'static,
    {
        task::spawn(async move {
            if let Err(e) = fut.await {
                eprintln!("{}", e)
            }
        })
    }    
    async fn msg_loop(mut events: mpsc::UnboundedReceiver<Event>) {
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
                Event::Connect(peer) => {
                    debug!("Registering peer {}", peer.actor_id);
                    peers.insert(peer.actor_id, peer);
                }
                Event::Disconnect { actor_id } => {
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
}


