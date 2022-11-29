use crate::broker::{BrokerEvent, ChBrokerSend, Destination};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Result, Write},
    path::{Path, PathBuf},
};

use futures::SinkExt;

pub enum StoBlobEvent {
    Added(String),
}

#[derive(Default)]
pub struct BlobStorage {
    path: Option<PathBuf>,
    ch_broker: Option<ChBrokerSend>,
}

pub trait ToBlobHashId {
    fn blob_hash_id(&self) -> String;
}

impl ToBlobHashId for &[u8] {
    fn blob_hash_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.input(self);
        format!("&{}.sha256", base64::encode(&hasher.result()))
    }
}

impl BlobStorage {
    pub fn open(&mut self, path: PathBuf, ch_broker: ChBrokerSend) {
        self.path = Some(path);
        self.ch_broker = Some(ch_broker);
    }
    fn path_of(&self, id: &str) -> PathBuf {
        let id = id.replace('&', "").replace('/', "_");
        [self.path.as_ref().unwrap(), Path::new(&id)]
            .iter()
            .collect()
    }
    pub fn size_of(&self, id: &str) -> Result<Option<u64>> {
        if let Ok(metadata) = std::fs::metadata(self.path_of(id)) {
            Ok(Some(metadata.len()))
        } else {
            Ok(None)
        }
    }
    pub async fn insert<D: AsRef<[u8]>>(&self, content: D) -> Result<String> {
        let id = content.as_ref().blob_hash_id();
        File::create(self.path_of(&id))?.write_all(content.as_ref())?;

        let broker_msg = BrokerEvent::new(Destination::Broadcast, StoBlobEvent::Added(id.clone()));

        self.ch_broker
            .as_ref()
            .unwrap()
            .send(broker_msg)
            .await
            .unwrap();

        Ok(id)
    }
    pub fn get(&self, id: &str) -> Result<Vec<u8>> {
        let mut file = File::open(self.path_of(id))?;
        let mut content = Vec::with_capacity(file.metadata()?.len() as usize);
        file.read_to_end(&mut content)?;
        Ok(content)
    }
    pub fn exists(&self, id: &str) -> bool {
        self.path_of(id).exists()
    }
}
