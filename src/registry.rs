use async_std::{prelude::*, sync::Mutex, task, task::JoinHandle};
use futures::{channel::mpsc, channel::oneshot, select, FutureExt, SinkExt};

use once_cell::sync::Lazy;

use std::collections::hash_map::HashMap;

#[derive(Debug)]
pub struct Void {}

pub type RegistryCh = mpsc::UnboundedSender<Event>;
pub type SigTerminated = oneshot::Sender<Void>;
pub type SigTerminate = oneshot::Receiver<Void>;

pub type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub enum Event {
    Connect(RegistryEndpoint),
    Disconnect { actor_id: usize },
    Terminate,
}

#[derive(Debug)]
pub struct RegistryEndpoint {
    pub actor_id: usize,
    pub terminate: SigTerminated,
    pub terminated: SigTerminate,
}
#[derive(Debug)]
pub struct ActorEndpoint {
    pub actor_id: usize,
    pub ch: RegistryCh,
    pub terminate: SigTerminate,
    pub terminated: SigTerminated,
}

#[derive(Debug)]
pub struct Registry {
    last_actor_id: usize,
    sender: RegistryCh,
    msgloop: Option<JoinHandle<()>>,
}

pub static REGISTRY: Lazy<Mutex<Registry>> = Lazy::new(|| Mutex::new(Registry::new()));

impl Registry {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded();
        let msgloop = task::spawn(Self::msg_loop(receiver));
        Self {
            last_actor_id: 0,
            sender,
            msgloop: Some(msgloop),
        }
    }
    pub fn take_msgloop(&mut self) -> JoinHandle<()> {
        self.msgloop.take().unwrap()
    }
    pub async fn register(&mut self, name: &str) -> AnyResult<ActorEndpoint> {
        self.last_actor_id += 1;

        info!("Registering actor {}={}", self.last_actor_id, name);

        let (terminate_sender, terminate_receiver) = oneshot::channel::<Void>();
        let (terminated_sender, terminated_receiver) = oneshot::channel::<Void>();

        let registry_endpoint = RegistryEndpoint {
            actor_id: self.last_actor_id,
            terminate: terminate_sender,
            terminated: terminated_receiver,
        };
        let actor_endpoint = ActorEndpoint {
            actor_id: self.last_actor_id,
            ch: self.sender.clone(),
            terminate: terminate_receiver,
            terminated: terminated_sender,
        };

        self.sender
            .send(Event::Connect(registry_endpoint))
            .await
            .unwrap();

        Ok(actor_endpoint)
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
        let mut peers: HashMap<usize, RegistryEndpoint> = HashMap::new();

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
            info!("Sending term signal to {}", actor_id);
            let _ = term.send(Void {});
        }

        // wait to be finished
        for (actor_id, termd) in termds {
            info!("Waiting termd signal from {}", actor_id);
            let _ = termd.await;
        }
        drop(peers);
    }
}
