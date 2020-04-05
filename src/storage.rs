use async_std::sync::{Arc, RwLock};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_cbor;
use futures::SinkExt;

use kuska_ssb::feed::{Feed, Message};
use crate::broker::{ChBrokerSend,BrokerEvent};

use futures::channel::mpsc;

const PREFIX_LASTFEED: u8 = 0u8;
const PREFIX_FEED: u8 = 1u8;
const PREFIX_MESSAGE: u8 = 2u8;

#[derive(Debug,Clone)]
pub enum StorageEvent {
    IdChanged(String)
}

pub type ChStoRecv = mpsc::UnboundedReceiver<StorageEvent>;
pub type ChStoSend = mpsc::UnboundedSender<StorageEvent>;

pub static DB: Lazy<Arc<RwLock<Storage>>> = Lazy::new(|| Arc::new(RwLock::new(Storage::default())));

pub struct Storage {
    db: Option<sled::Db>,
    ch_broker: Option<ChBrokerSend>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FeedRef {
    author: String,
    seq_no: u64,
}

#[derive(Debug)]
pub enum Error {
    NotFound,
    Sled(sled::Error),
    Feed(kuska_ssb::feed::Error),
    Cbor(serde_cbor::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<sled::Error> for Error {
    fn from(err: sled::Error) -> Self {
        Error::Sled(err)
    }
}

impl From<kuska_ssb::feed::Error> for Error {
    fn from(err: kuska_ssb::feed::Error) -> Self {
        Error::Feed(err)
    }
}

impl From<serde_cbor::Error> for Error {
    fn from(err: serde_cbor::Error) -> Self {
        Error::Cbor(err)
    }
}

impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

impl Storage {
    pub fn default() -> Self {
        Self { db: None, ch_broker : None}
    }

    pub fn open(&mut self, path: &std::path::Path, ch_broker : ChBrokerSend) -> Result<()> {
        self.db = Some(sled::open(path)?);
        self.ch_broker = Some(ch_broker);
        Ok(())
    }
    pub fn get_feed_len(&self, user_id: &str) -> Result<Option<u64>> {
        let db = self.db.as_ref().unwrap();
        let key = Self::key_lastfeed(user_id);
        let count = if let Some(value) = db.get(&key)?.map(|v| v) {
            let mut u64_buffer = [0u8; 8];
            u64_buffer.copy_from_slice(&value);
            Some(u64::from_be_bytes(u64_buffer))
        } else {
            None
        };

        Ok(count)
    }

    fn key_feed(user_id: &str, feed_seq: u64) -> Vec<u8> {
        let mut key = Vec::new();
        key.push(PREFIX_FEED);
        key.extend_from_slice(&feed_seq.to_be_bytes()[..]);
        key.extend_from_slice(user_id.as_bytes());
        key
    }

    fn key_lastfeed(user_id: &str) -> Vec<u8> {
        let mut key = Vec::new();
        key.push(PREFIX_LASTFEED);
        key.extend_from_slice(user_id.as_bytes());
        key
    }

    fn key_message(message_id: &str) -> Vec<u8> {
        let mut key = Vec::new();
        key.push(PREFIX_MESSAGE);
        key.extend_from_slice(message_id.as_bytes());
        key
    }

    pub fn get_feed(&self, user_id: &str, feed_seq: u64) -> Result<Feed> {
        let db = self.db.as_ref().unwrap();
        let raw = db
            .get(Self::key_feed(user_id, feed_seq))?
            .ok_or(Error::NotFound)?;

        Ok(Feed::from_slice(&raw)?)
    }

    pub fn get_message(&self, msg_id: &str) -> Result<Message> {
        let db = self.db.as_ref().unwrap();

        let raw = db.get(Self::key_message(&msg_id))?.ok_or(Error::NotFound)?;

        let feed_ref = serde_cbor::from_slice::<FeedRef>(&raw)?;
        Ok(self
            .get_feed(&feed_ref.author, feed_ref.seq_no)?
            .into_message()?)
    }

    pub async fn append_feed(&self, msg: Message) -> Result<u64> {
        let seq_no = self.get_feed_len(msg.author())?.map_or(0, |no| no + 1);
        let author = msg.author().to_owned();
        let db = self.db.as_ref().unwrap();

        let feed_ref = serde_cbor::to_vec(&FeedRef {
            author: author.clone(),
            seq_no,
        })?;
        db.insert(Self::key_message(&msg.id().to_string()), feed_ref)?;

        let feed = Feed::new(msg.clone());
        db.insert(Self::key_feed(&author, seq_no), feed.to_string().as_bytes())?;
        db.insert(Self::key_lastfeed(&author), &seq_no.to_be_bytes()[..])?;

        self.ch_broker.as_ref().unwrap().send(BrokerEvent::Storage(StorageEvent::IdChanged(msg.author().clone()))).await.unwrap();
        Ok(seq_no)
    }
}