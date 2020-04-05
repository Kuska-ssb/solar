use std::collections::HashMap;
use std::string::ToString;
use std::marker::PhantomData;

use async_std::io::{Read, Write};
use async_trait::async_trait;

use kuska_ssb::{
    api::{ApiHelper, ApiMethod, CreateHistoryStreamArgs},
    rpc,
};

use crate::error::AnyResult;
use crate::storage::StorageEvent;
use crate::storage::DB;

use super::{RpcHandler, RpcInput};

struct HistoryStreamRequest {
    req_no: i32,
    args: CreateHistoryStreamArgs,
    from: u64,
}

pub struct HistoryStreamHandler<R,W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync
{
    reqs: HashMap<String, HistoryStreamRequest>,
    phantom : PhantomData<(R,W)>,
}

impl<R,W> Default for HistoryStreamHandler<R,W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync
{
    fn default() -> Self {
        Self {
            reqs: HashMap::new(),
            phantom : PhantomData,
        }
    }
}

#[async_trait]
impl<R, W> RpcHandler<R, W> for HistoryStreamHandler<R,W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync
{
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool> {
        match op {
            RpcInput::Network(req_no, rpc::RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::CreateHistoryStream) => {
                        self.recv_createhistorystream(api, *req_no, req).await
                    }
                    _ => Ok(false),
                }
            }
            RpcInput::Network(req_no, rpc::RecvMsg::CancelStreamRespose()) => {
                self.recv_cancelstream(api,*req_no).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::ErrorResponse(err)) => {
                self.recv_error_response(api,*req_no,err).await
            }
            RpcInput::Storage(StorageEvent::IdChanged(id)) => {
                self.recv_storageevent_idchanged(api,id).await
            }
            _ => Ok(false),
        }
    }
}

impl<R,W> HistoryStreamHandler<R,W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync
 {
    async fn recv_createhistorystream(
        &mut self,
        api: &mut ApiHelper<R, W>,
        req_no: i32, req: &rpc::Body
    ) -> AnyResult<bool> {
        let mut args: Vec<CreateHistoryStreamArgs> =
        serde_json::from_value(req.args.clone())?;

        let args = args.pop().unwrap();
        let from = args.seq.unwrap_or(1u64);

        let mut req = HistoryStreamRequest {
            args,
            from,
            req_no,
        };

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
        req_no: i32
    ) -> AnyResult<bool> {
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
        error_msg : &str,
    ) -> AnyResult<bool> {

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
        id : &str,
    ) -> AnyResult<bool> {
        if let Some(mut req) = self.reqs.remove(id) {
            self.send_history(api, &mut req).await?;
            self.reqs.insert(id.to_string(), req);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn find_key_by_req_no(&self, req_no: i32) -> Option<String> {
        self
            .reqs
            .iter()
            .find(|(_, v)| v.req_no == req_no)
            .map(|(k, _)| k.clone())
    }

    async fn send_history(
        &mut self,
        api: &mut ApiHelper<R, W>,
        req: &mut HistoryStreamRequest,
    ) -> AnyResult<()> {
        let req_id = if req.args.id.starts_with('@') {
            req.args.id.clone()
        } else {
            format!("@{}", req.args.id).to_string()
        };

        let last = DB.read().await.get_feed_len(&req_id)?.map_or(0, |x| x + 1);
        let with_keys = req.args.keys.unwrap_or(true);

        info!(
            "Sending history stream of {} ({}..{})",
            req.args.id, req.from, last
        );
        for n in req.from..last {
            let data = DB.read().await.get_feed(&req_id, n - 1)?;
            let data = if with_keys {
                data.to_string()
            } else {
                data.value.to_string()
            };
            info!(" - [with_keys={}]{}", with_keys, &data.to_string());
            api.feed_res_send(req.req_no, &data).await?;
        }

        req.from = last;
        Ok(())
    }
}
