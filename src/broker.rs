use core::any::Any;
use async_std::{prelude::*, sync::{Mutex,Arc}, task, task::JoinHandle};
use futures::{channel::mpsc, channel::oneshot, FutureExt, SinkExt};

use once_cell::sync::Lazy;

use std::collections::hash_map::HashMap;

use crate::error::SolarResult;

#[derive(Debug)]
pub struct Void {}

pub type BrokerMessage = Arc<dyn Any + Send + Sync>; 

pub type ChBrokerSend = mpsc::UnboundedSender<BrokerEvent>;
pub type ChSigSend = oneshot::Sender<Void>;
pub type ChSigRecv = oneshot::Receiver<Void>;
pub type ChMsgSend = mpsc::UnboundedSender<BrokerMessage>;
pub type ChMsgRecv = mpsc::UnboundedReceiver<BrokerMessage>;

#[derive(PartialEq,Debug)]
pub enum Destination {
    Actor(usize),
    Broadcast,
}

#[derive(Debug)]
pub enum BrokerEvent {
    Connect(BrokerEndpoint),
    Disconnect { actor_id: usize },
    Message {to:Destination,msg:BrokerMessage},
    Terminate,
}

impl BrokerEvent {
    pub fn new<A>(to:Destination,any : A) -> Self
    where A : Any + Send + Sync {
        BrokerEvent::Message{to,msg:Arc::new(any)}
    } 
}

#[derive(Debug)]
pub struct BrokerEndpoint {
    pub actor_id: usize,
    pub ch_terminate: ChSigSend,
    pub ch_terminated: ChSigRecv,

    pub ch_msg: Option<ChMsgSend>,
}

#[derive(Debug)]
pub struct ActorEndpoint {
    pub actor_id: usize,

    pub ch_broker: ChBrokerSend,
    pub ch_terminate: ChSigRecv,
    pub ch_terminated: ChSigSend,

    pub ch_msg: Option<ChMsgRecv>,
}

#[derive(Debug)]
pub struct Broker {
    last_actor_id: usize,
    sender: ChBrokerSend,
    msgloop: Option<JoinHandle<()>>,
}

pub static BROKER: Lazy<Mutex<Broker>> = Lazy::new(|| Mutex::new(Broker::new()));

impl Broker {
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
    pub async fn register(
        &mut self,
        name: &str,
        msg_notify: bool
    ) -> SolarResult<ActorEndpoint> {
        self.last_actor_id += 1;

        trace!(target:"solar-actor","registering actor {}={}", self.last_actor_id, name);

        let (terminate_sender, terminate_receiver) = oneshot::channel::<Void>();
        let (terminated_sender, terminated_receiver) = oneshot::channel::<Void>();

        let (msg_sender, msg_receiver) = if msg_notify {
            let (s, r) = mpsc::unbounded::<BrokerMessage>();
            (Some(s), Some(r))
        } else {
            (None, None)
        };

        let broker_endpoint = BrokerEndpoint {
            actor_id: self.last_actor_id,
            ch_terminate: terminate_sender,
            ch_terminated: terminated_receiver,
            ch_msg: msg_sender,
        };
        let actor_endpoint = ActorEndpoint {
            actor_id: self.last_actor_id,
            ch_broker: self.sender.clone(),
            ch_terminate: terminate_receiver,
            ch_terminated: terminated_sender,
            ch_msg: msg_receiver,
        };

        self.sender
            .send(BrokerEvent::Connect(broker_endpoint))
            .await
            .unwrap();

        Ok(actor_endpoint)
    }
    pub fn create_sender(&self) -> ChBrokerSend {
        self.sender.clone()
    }

    pub fn spawn<F>(fut: F) -> task::JoinHandle<()>
    where
        F: Future<Output = SolarResult<()>> + Send + 'static,
    {
        task::spawn(async move {
            if let Err(e) = fut.await {
                eprintln!("{}", e)
            }
        })
    }
    async fn msg_loop(mut events: mpsc::UnboundedReceiver<BrokerEvent>) {
        let mut actors: HashMap<usize, BrokerEndpoint> = HashMap::new();

        loop {
            let event = select_biased! {
                event = events.next().fuse() => match event {
                    None => break,
                    Some(event) => event,
                },
            };
            match event {
                BrokerEvent::Terminate => {
                    info!("Msg Got terminate ");
                    break;
                }
                BrokerEvent::Connect(actor) => {
                    trace!(target:"solar-actor", "Registering actor {}", actor.actor_id);
                    actors.insert(actor.actor_id, actor);
                }
                BrokerEvent::Disconnect { actor_id } => {
                    trace!(target:"solar-actor","Unregistering actor {}", actor_id);
                    actors.remove(&actor_id);
                }
                BrokerEvent::Message{to,msg} => {
                    for actor in actors.values_mut() {
                        if to == Destination::Actor(actor.actor_id) || to == Destination::Broadcast {
                            if let Some(ch) = &mut actor.ch_msg {
                                let _ = ch.send(msg.clone()).await;
                            }    
                        }
                    }
                }
            }
        }

        trace!(target:"solar-actor","***Loop finished**");

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
            trace!(target:"solar-actor","Sending term signal to {}", actor_id);
            let _ = term.send(Void {});
        }

        // wait to be finished
        for (actor_id, termd) in termds {
            trace!(target:"solar-actor","Waiting termd signal from {}", actor_id);
            let _ = termd.await;
        }

        trace!(target:"solar-actor","***All actors finished**");

        drop(actors);
    }
}
