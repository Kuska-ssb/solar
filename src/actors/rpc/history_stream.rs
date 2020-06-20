use std::collections::HashMap;
use std::marker::PhantomData;
use std::string::ToString;

use crate::futures::SinkExt;
use async_std::io::Write;
use async_trait::async_trait;
use regex::Regex;

use kuska_ssb::{
    api::{dto, ApiCaller, ApiMethod},
    feed::{Feed, Message},
    rpc,
};

use once_cell::sync::Lazy;

use super::{RpcHandler, RpcInput};
use crate::broker::ChBrokerSend;
use crate::broker::{BrokerEvent, Destination};
use crate::storage::kv::StoKvEvent;
use crate::CONFIG;
use crate::{BLOB_STORAGE, KV_STORAGE};
use anyhow::Result;

pub static BLOB_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(&[0-9A-Za-z/+=]*.sha256)").unwrap());

#[derive(Debug)]
struct HistoryStreamRequest {
    req_no: i32,
    args: dto::CreateHistoryStreamIn,
    from: u64, // check, not sure if ok
}

pub struct HistoryStreamHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    initialized: bool,
    _actor_id: usize,
    reqs: HashMap<String, HistoryStreamRequest>,
    friends: HashMap<i32, String>,
    phantom: PhantomData<W>,
}

#[async_trait]
impl<W> RpcHandler<W> for HistoryStreamHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "HistoryStreamHandler"
    }

    async fn handle(
        &mut self,
        api: &mut ApiCaller<W>,
        op: &RpcInput,
        ch_broker: &mut ChBrokerSend,
    ) -> Result<bool> {
        match op {
            RpcInput::Network(req_no, rpc::RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::CreateHistoryStream) => {
                        self.recv_createhistorystream(api, *req_no, req).await
                    }
                    _ => Ok(false),
                }
            }
            RpcInput::Network(req_no, rpc::RecvMsg::RpcResponse(_type, res)) => {
                self.recv_rpc_response(api, ch_broker, *req_no, &res).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::CancelStreamRespose()) => {
                self.recv_cancelstream(api, *req_no).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::ErrorResponse(err)) => {
                self.recv_error_response(api, *req_no, err).await
            }
            RpcInput::Message(msg) => {
                if let Some(kv_event) = msg.downcast_ref::<StoKvEvent>() {
                    match kv_event {
                        StoKvEvent::IdChanged(id) => {
                            return self.recv_storageevent_idchanged(api, id).await
                        }
                    }
                }
                Ok(false)
            }
            RpcInput::Timer => self.on_timer(api).await,
            _ => Ok(false),
        }
    }
}

impl<W> HistoryStreamHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    pub fn new(actor_id: usize) -> Self {
        Self {
            _actor_id: actor_id,
            initialized: false,
            friends: HashMap::new(),
            reqs: HashMap::new(),
            phantom: PhantomData,
        }
    }

    async fn on_timer(&mut self, api: &mut ApiCaller<W>) -> Result<bool> {
        if !self.initialized {
            debug!(target: "solar", "initializing historystreamhandler");
            let args = dto::CreateHistoryStreamIn::new(CONFIG.get().unwrap().id.clone());
            let _ = api.create_history_stream_req_send(&args).await?;
            for friend in &CONFIG.get().unwrap().friends {
                let mut args = dto::CreateHistoryStreamIn::new(friend.to_string()).live(true);
                if let Some(last_feed) = KV_STORAGE.read().await.get_last_feed_no(&friend)? {
                    args = args.after_seq(last_feed);
                }
                let id = api.create_history_stream_req_send(&args).await?;
                self.friends.insert(id, friend.to_string());
                debug!(target: "solar", "Requesting feeds from friend {} starting with {:?}" ,friend,args.seq);
            }
            self.initialized = true;
        }
        Ok(false)
    }

    fn extract_blob_refs(&mut self, msg: &Message) -> Vec<String> {
        let mut refs = Vec::new();
        let msg: Result<dto::content::TypedMessage, _> =
            serde_json::from_value(msg.content().clone());
        if let Ok(dto::content::TypedMessage::Post { text, .. }) = msg {
            for cap in BLOB_REGEX.captures_iter(&text) {
                let key = cap.get(0).unwrap().as_str().to_owned();
                refs.push(key);
            }
        }
        refs
    }

    async fn recv_rpc_response(
        &mut self,
        _api: &mut ApiCaller<W>,
        ch_broker: &mut ChBrokerSend,
        req_no: i32,
        res: &[u8],
    ) -> Result<bool> {
        if self.friends.contains_key(&req_no) {
            let msg = Feed::from_slice(res)?.into_message()?;
            let last_feed = KV_STORAGE
                .read()
                .await
                .get_last_feed_no(&msg.author().to_string())?
                .unwrap_or(0);
            if msg.sequence() == last_feed + 1 {
                KV_STORAGE.write().await.append_feed(msg.clone()).await?;
                info!("Recieved {} msg no {}", msg.author(), msg.sequence());
                for key in self.extract_blob_refs(&msg) {
                    if !BLOB_STORAGE.read().await.exists(&key) {
                        let event = super::blobs_get::RpcBlobsGetEvent::Get(dto::BlobsGetIn {
                            key,
                            size: None,
                            max: None,
                        });
                        let broker_msg = BrokerEvent::new(Destination::Broadcast, event);
                        ch_broker.send(broker_msg).await.unwrap();
                    }
                }
            } else {
                warn!(
                    "Recieved message out of order {} recv:{} db:{}",
                    &msg.author().to_string(),
                    msg.sequence(),
                    last_feed
                );
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn recv_createhistorystream(
        &mut self,
        api: &mut ApiCaller<W>,
        req_no: i32,
        req: &rpc::Body,
    ) -> Result<bool> {
        let mut args: Vec<dto::CreateHistoryStreamIn> = serde_json::from_value(req.args.clone())?;

        let args = args.pop().unwrap();
        let from = args.seq.unwrap_or(1u64);

        let mut req = HistoryStreamRequest { args, from, req_no };

        self.send_history(api, &mut req).await?;

        if req.args.live.unwrap_or(false) {
            self.reqs.insert(req.args.id.clone(), req);
        } else {
            api.rpc().send_stream_eof(req_no).await?;
        }

        Ok(true)
    }

    async fn recv_cancelstream(&mut self, api: &mut ApiCaller<W>, req_no: i32) -> Result<bool> {
        if let Some(key) = self.find_key_by_req_no(req_no) {
            api.rpc().send_stream_eof(-req_no).await?;
            self.reqs.remove(&key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn recv_error_response(
        &mut self,
        _api: &mut ApiCaller<W>,
        req_no: i32,
        error_msg: &str,
    ) -> Result<bool> {
        if let Some(key) = self.find_key_by_req_no(req_no) {
            warn!("error {}", error_msg);
            self.reqs.remove(&key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn recv_storageevent_idchanged(
        &mut self,
        api: &mut ApiCaller<W>,
        id: &str,
    ) -> Result<bool> {
        if let Some(mut req) = self.reqs.remove(id) {
            self.send_history(api, &mut req).await?;
            self.reqs.insert(id.to_string(), req);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn find_key_by_req_no(&self, req_no: i32) -> Option<String> {
        self.reqs
            .iter()
            .find(|(_, v)| v.req_no == req_no)
            .map(|(k, _)| k.clone())
    }

    async fn send_history(
        &mut self,
        api: &mut ApiCaller<W>,
        req: &mut HistoryStreamRequest,
    ) -> Result<()> {
        let req_id = if req.args.id.starts_with('@') {
            req.args.id.clone()
        } else {
            format!("@{}", req.args.id).to_string()
        };

        let last = KV_STORAGE
            .read()
            .await
            .get_last_feed_no(&req_id)?
            .map_or(0, |x| x + 1);
        let with_keys = req.args.keys.unwrap_or(true);

        info!(
            "Sending history stream {} ({}..{})",
            req.args.id, req.from, last
        );
        for n in req.from..last {
            let data = KV_STORAGE.read().await.get_feed(&req_id, n)?.unwrap();
            let data = if with_keys {
                data.to_string()
            } else {
                data.value.to_string()
            };
            api.feed_res_send(req.req_no, &data).await?;
        }

        req.from = last;
        Ok(())
    }
}
