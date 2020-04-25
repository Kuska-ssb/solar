use std::collections::HashMap;
use std::marker::PhantomData;
use std::string::ToString;

use async_std::io::{Read, Write};
use async_trait::async_trait;
use regex::Regex;

use kuska_ssb::{
    api::{dto, ApiHelper, ApiMethod},
    feed::Feed,
    rpc,
};

use crate::error::SolarResult;
use crate::storage::kv::StorageEvent;
use crate::KV_STORAGE;
use crate::CONFIG;    
use super::{RpcHandler, RpcInput};

struct HistoryStreamRequest {
    req_no: i32,
    args: dto::CreateHistoryStreamIn,
    from: u64,
}

pub struct HistoryStreamHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    initialized: bool,
    reqs: HashMap<String, HistoryStreamRequest>,
    friends: HashMap<i32,String>, 
    phantom: PhantomData<(R, W)>,
}

impl<R, W> Default for HistoryStreamHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn default() -> Self {
        Self {
            initialized : false,
            friends: HashMap::new(),
            reqs: HashMap::new(),
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<R, W> RpcHandler<R, W> for HistoryStreamHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "HistoryStreamHandler"
    }

    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> SolarResult<bool> {
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
                self.recv_rpc_response(api, *req_no, &res).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::CancelStreamRespose()) => {
                self.recv_cancelstream(api, *req_no).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::ErrorResponse(err)) => {
                self.recv_error_response(api, *req_no, err).await
            }
            RpcInput::Storage(StorageEvent::IdChanged(id)) => {
                self.recv_storageevent_idchanged(api, id).await
            }
            RpcInput::Timer => {
                self.on_timer(api).await
            }
            _ => Ok(false),
        }
    }
}

impl<R, W> HistoryStreamHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    async fn on_timer(&mut self,api: &mut ApiHelper<R, W>) -> SolarResult<bool> {
        if !self.initialized {
            let args = dto::CreateHistoryStreamIn::new(CONFIG.get().unwrap().id.clone());
            let _ = api.create_history_stream_req_send(&args).await?;            
            for friend in &CONFIG.get().unwrap().friends {
                let args = dto::CreateHistoryStreamIn::new(friend.to_string());
                let id =  api.create_history_stream_req_send(&args).await?;
                self.friends.insert(id, friend.to_string());
            }
        }
        Ok(false)
    }

    async fn recv_rpc_response(
        &mut self,
        _api: &mut ApiHelper<R,W>,
        req_no: i32,
        res: &[u8]
    ) -> SolarResult<bool> {
        if self.friends.contains_key(&req_no) {
            let msg = Feed::from_slice(res)?.into_message()?;
            if Some(msg.sequence()) == KV_STORAGE.read().await.get_feed_len(&msg.id().to_string())? {
                KV_STORAGE.write().await.append_feed(msg.clone()).await?;
                let msg : Result<dto::content::TypedMessage,_> = serde_json::from_value(msg.value.clone());
                if let Ok(dto::content::TypedMessage::Post{post}) = msg {
                    let re = Regex::new(r"%[0-9A-Za-z/+=]*.sha256").unwrap();
                    for cap in re.captures_iter(&post.text) {
                        // if let Some(blob) = KV_STORAGE.get_blob(cap) {}
                        println!("{:?}", &cap);
                    }    
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn recv_createhistorystream(
        &mut self,
        api: &mut ApiHelper<R, W>,
        req_no: i32,
        req: &rpc::Body,
    ) -> SolarResult<bool> {
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

    async fn recv_cancelstream(
        &mut self,
        api: &mut ApiHelper<R, W>,
        req_no: i32,
    ) -> SolarResult<bool> {
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
        _api: &mut ApiHelper<R, W>,
        req_no: i32,
        error_msg: &str,
    ) -> SolarResult<bool> {
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
        api: &mut ApiHelper<R, W>,
        id: &str,
    ) -> SolarResult<bool> {
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
        api: &mut ApiHelper<R, W>,
        req: &mut HistoryStreamRequest,
    ) -> SolarResult<()> {
        let req_id = if req.args.id.starts_with('@') {
            req.args.id.clone()
        } else {
            format!("@{}", req.args.id).to_string()
        };

        let last = KV_STORAGE
            .read()
            .await
            .get_feed_len(&req_id)?
            .map_or(0, |x| x + 1);
        let with_keys = req.args.keys.unwrap_or(true);

        info!(
            "Sending history stream {} ({}..{})",
            req.args.id, req.from, last
        );
        for n in req.from..last {
            let data = KV_STORAGE.read().await.get_feed(&req_id, n - 1)?.unwrap();
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
