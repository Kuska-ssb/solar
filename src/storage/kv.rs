use futures::SinkExt;
use serde::{Deserialize, Serialize};

use crate::broker::{BrokerEvent, Destination, ChBrokerSend};
use kuska_ssb::feed::{Feed, Message};

const PREFIX_LASTFEED: u8 = 0u8;
const PREFIX_FEED: u8 = 1u8;
const PREFIX_MESSAGE: u8 = 2u8;
const PREFIX_BLOB: u8 = 3u8;

#[derive(Debug, Clone)]
pub enum StoKvEvent {
    IdChanged(String),
}

pub struct KvStorage {
    db: Option<sled::Db>,
    ch_broker: Option<ChBrokerSend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    retrieved: bool,
    users: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FeedRef {
    author: String,
    seq_no: u64,
}

#[derive(Debug)]
pub enum Error {
    InvalidSequence,
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

impl Default for KvStorage {
    fn default() -> Self {
        Self {
            db: None,
            ch_broker: None,
        }
    }
}

impl KvStorage {
    pub fn open(&mut self, path: &std::path::Path, ch_broker: ChBrokerSend) -> Result<()> {
        self.db = Some(sled::open(path)?);
        self.ch_broker = Some(ch_broker);
        Ok(())
    }

    pub fn get_last_feed_no(&self, user_id: &str) -> Result<Option<u64>> {
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

    fn key_blob(blob_hash: &str) -> Vec<u8> {
        let mut key = Vec::new();
        key.push(PREFIX_BLOB);
        key.extend_from_slice(blob_hash.as_bytes());
        key
    }

    pub fn get_blob(&self, blob_hash: &str) -> Result<Option<Blob>> {
        let db = self.db.as_ref().unwrap();
        if let Some(raw) = db.get(Self::key_blob(blob_hash))? {
            Ok(serde_cbor::from_slice(&raw)?)
        } else {
            Ok(None)
        }
    }

    pub fn set_blob(&self, blob_hash: &str, blob: &Blob) -> Result<()> {
        let db = self.db.as_ref().unwrap();
        let raw = serde_cbor::to_vec(blob)?;
        db.insert(Self::key_blob(blob_hash), raw)?;

        Ok(())
    }

    pub fn get_pending_blobs(&self) -> Result<Vec<String>> {
        let mut list = Vec::new();
 
        let db = self.db.as_ref().unwrap();
        let scan_key : &[u8] = &[PREFIX_BLOB];
        for item in db.range(scan_key..) {
            let (k,v) = item?;
            let blob : Blob = serde_cbor::from_slice(&v)?;
            if !blob.retrieved {
                list.push(String::from_utf8_lossy(&k[1..]).to_string());
            }
        }
        Ok(list)
    }
    
    pub fn get_feed(&self, user_id: &str, feed_seq: u64) -> Result<Option<Feed>> {
        let db = self.db.as_ref().unwrap();
        if let Some(raw) = db.get(Self::key_feed(user_id, feed_seq))? {
            Ok(Some(Feed::from_slice(&raw)?))
        } else { 
            Ok(None)
        }
    }

    pub fn get_message(&self, msg_id: &str) -> Result<Option<Message>> {
        let db = self.db.as_ref().unwrap();

        if let Some(raw) = db.get(Self::key_message(&msg_id))? {
            let feed_ref = serde_cbor::from_slice::<FeedRef>(&raw)?;
            let msg = self.get_feed(&feed_ref.author, feed_ref.seq_no)?.unwrap().into_message()?;
            Ok(Some(msg))
        } else {
            Ok(None)
        }
    }

    pub async fn append_feed(&self, msg: Message) -> Result<u64> {
        let seq_no = self.get_last_feed_no(msg.author())?.map_or(0, |no| no ) + 1;
        
        if msg.sequence() != seq_no {
            return Err(Error::InvalidSequence);
        }

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

        db.flush_async().await?;

        let broker_msg = BrokerEvent::new(Destination::Broadcast,StoKvEvent::IdChanged(msg.author().clone()));

        self.ch_broker
            .as_ref()
            .unwrap()
            .send(broker_msg)
            .await
            .unwrap();
        Ok(seq_no)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_blobs() -> Result<()> {
        let mut kv = KvStorage::default();
        let (sender, _) = futures::channel::mpsc::unbounded();
        let path = tempdir::TempDir::new("solardb").unwrap();
        kv.open(path.path(), sender).unwrap();
        assert_eq!(true,kv.get_blob("1").unwrap().is_none());
        kv.set_blob("b1", &Blob {
            retrieved : true,
            users : ["u1".to_string()].to_vec(),
        }).unwrap();
        kv.set_blob("b2", &Blob {
            retrieved : false,
            users : ["u2".to_string()].to_vec(),
        }).unwrap();
        let blob = kv.get_blob("b1").unwrap().unwrap();
        assert_eq!(blob.retrieved,true);
        assert_eq!(blob.users,["u1".to_string()].to_vec());
        assert_eq!(kv.get_pending_blobs().unwrap(),["b2".to_string()].to_vec());
              
        kv.set_blob("b1", &Blob {
            retrieved : false,
            users : ["u7".to_string()].to_vec(),
        }).unwrap();
        let blob = kv.get_blob("b1").unwrap().unwrap();
        assert_eq!(blob.retrieved,false);
        assert_eq!(blob.users,["u7".to_string()].to_vec());
        assert_eq!(kv.get_pending_blobs().unwrap(),["b1".to_string(),"b2".to_string()].to_vec());

        Ok(())
    }
}