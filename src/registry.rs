use async_std::{prelude::*, sync::Mutex, task, task::JoinHandle};
use futures::{channel::mpsc, channel::oneshot, select, FutureExt, SinkExt};

use once_cell::sync::Lazy;

use std::collections::hash_map::HashMap;

#[derive(Debug)]
pub struct Void {}

pub type ChStoRecv = mpsc::UnboundedReceiver<String>;
pub type ChStoSend = mpsc::UnboundedSender<String>;
pub type ChRegevSend = mpsc::UnboundedSender<Event>;
pub type ChSigSend = oneshot::Sender<Void>;
pub type ChSigRecv = oneshot::Receiver<Void>;

pub type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;

#[derive(Debug)]
pub enum Event {
    Connect(RegistryEndpoint),
    Disconnect { actor_id: usize },
    IdUpdated(String),
    Terminate,
}

#[derive(Debug)]
pub struct RegistryEndpoint {
    pub actor_id: usize,
    pub ch_terminate: ChSigSend,
    pub ch_terminated: ChSigRecv,
    pub ch_storage: Option<ChStoSend>,
}

#[derive(Debug)]
pub struct ActorEndpoint {
    pub actor_id: usize,

    pub ch_registry: ChRegevSend,
    pub ch_terminate: ChSigRecv,
    pub ch_terminated: ChSigSend,

    pub ch_storage: Option<ChStoRecv>,
}

#[derive(Debug)]
pub struct Registry {
    last_actor_id: usize,
    sender: ChRegevSend,
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
    pub async fn register(&mut self, name: &str, storage_notify :bool) -> AnyResult<ActorEndpoint> {
        self.last_actor_id += 1;

        info!("Registering actor {}={}", self.last_actor_id, name);

        let (terminate_sender, terminate_receiver) = oneshot::channel::<Void>();
        let (terminated_sender, terminated_receiver) = oneshot::channel::<Void>();

        let (sto_sender, sto_receiver) = if storage_notify {
            let (s,r) = mpsc::unbounded::<String>();
            (Some(s),Some(r))
        } else {
            (None,None)
        };

        let registry_endpoint = RegistryEndpoint {
            actor_id: self.last_actor_id,
            ch_terminate: terminate_sender,
            ch_terminated: terminated_receiver,
            ch_storage: sto_sender,
        };
        let actor_endpoint = ActorEndpoint {
            actor_id: self.last_actor_id,
            ch_registry: self.sender.clone(),
            ch_terminate: terminate_receiver,
            ch_terminated: terminated_sender,
            ch_storage : sto_receiver,
        };

        self.sender
            .send(Event::Connect(registry_endpoint))
            .await
            .unwrap();

        Ok(actor_endpoint)
    }
    pub fn create_sender(&self) -> ChRegevSend {
        self.sender.clone()
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
        let mut actors: HashMap<usize, RegistryEndpoint> = HashMap::new();

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
                Event::Connect(actor) => {
                    debug!("Registering actor {}", actor.actor_id);
                    actors.insert(actor.actor_id, actor);
                }
                Event::Disconnect { actor_id } => {
                    debug!("Unregistering actor {}", actor_id);
                    actors.remove(&actor_id);
                }
                Event::IdUpdated(id) => {
                    for actor in actors.values_mut() {
                        if let Some(ch) = &mut actor.ch_storage {
                            let _ = ch.send(id.clone()).await;
                        }
                    }                    
                }
            }
        }

        // send a termination signal
        let (terms, termds): (Vec<_>, Vec<_>) = actors
            .drain()
            .map(|(_, actor)| {
                (
                    (actor.actor_id, actor.ch_terminate),
                    (actor.actor_id, actor.ch_terminated),
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
        drop(actors);
    }
}