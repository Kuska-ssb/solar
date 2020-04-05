use std::collections::HashMap;
use std::string::ToString;

use async_std::io::{Read, Write};
use async_trait::async_trait;

use kuska_ssb::{
    api::{ApiHelper, ApiMethod, CreateHistoryStreamArgs},
    rpc::RecvMsg,
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

pub struct HistoryStreamHandler {
    reqs: HashMap<String, HistoryStreamRequest>,
}

impl Default for HistoryStreamHandler {
    fn default() -> Self {
        Self {
            reqs: HashMap::new(),
        }
    }
}

#[async_trait]
impl<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync> RpcHandler<R, W>
    for HistoryStreamHandler
{
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool> {
        match op {
            RpcInput::Network(req_no, RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::CreateHistoryStream) => {
                        let mut args: Vec<CreateHistoryStreamArgs> =
                            serde_json::from_value(req.args.clone())?;

                        let args = args.pop().unwrap();
                        let from = args.seq.unwrap_or(1u64);

                        let mut req = HistoryStreamRequest {
                            args,
                            from,
                            req_no: *req_no,
                        };

                        self.send_history(api, &mut req).await?;

                        if req.args.live.unwrap_or(false) {
                            self.reqs.insert(req.args.id.clone(), req);
                        } else {
                            api.rpc().send_stream_eof(*req_no).await?;
                        }
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }

            RpcInput::Network(req_no, RecvMsg::CancelStreamRespose()) => {
                let key = self
                    .reqs
                    .iter()
                    .find(|(_, v)| v.req_no == *req_no)
                    .map(|(k, _)| k.clone());
                if let Some(key) = key {
                    api.rpc().send_stream_eof(-req_no).await?;
                    self.reqs.remove(&key);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }

            RpcInput::Network(req_no, RecvMsg::ErrorResponse(err)) => {
                let key = self
                    .reqs
                    .iter()
                    .find(|(_, v)| v.req_no == *req_no)
                    .map(|(k, _)| k.clone());
                if let Some(key) = key {
                    warn!("error {}", err);
                    self.reqs.remove(&key);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }

            RpcInput::Storage(StorageEvent::IdChanged(id)) => {
                if let Some(mut req) = self.reqs.remove(id) {
                    self.send_history(api, &mut req).await?;
                    self.reqs.insert(id.clone(), req);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false),
        }
    }
}

impl HistoryStreamHandler {
    async fn send_history<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync>(
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
