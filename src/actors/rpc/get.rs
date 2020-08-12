use async_std::io::Write;
use std::marker::PhantomData;

use async_trait::async_trait;
use kuska_ssb::{
    api::{ApiCaller, ApiMethod},
    rpc,
};

use crate::{broker::ChBrokerSend, KV_STORAGE};
use anyhow::Result;

use super::{RpcHandler, RpcInput};

pub struct GetHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    phantom: PhantomData<W>,
}

impl<W> Default for GetHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    fn default() -> Self {
        Self {
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<W> RpcHandler<W> for GetHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "GetHandler"
    }

    async fn handle(
        &mut self,
        api: &mut ApiCaller<W>,
        op: &RpcInput,
        _ch_broker: &mut ChBrokerSend,
    ) -> Result<bool> {
        match op {
            RpcInput::Network(req_no, rpc::RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::Get) => self.recv_get(api, *req_no, req).await,
                    _ => Ok(false),
                }
            }
            _ => Ok(false),
        }
    }
}

impl<W> GetHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    async fn recv_get(
        &mut self,
        api: &mut ApiCaller<W>,
        req_no: i32,
        req: &rpc::Body,
    ) -> Result<bool> {
        let args: Vec<String> = serde_json::from_value(req.args.clone())?;
        let msg = KV_STORAGE.read().await.get_message(&args[0]);
        match msg {
            Ok(Some(msg)) => api.get_res_send(req_no, &msg).await?,
            Ok(None) => {
                api.rpc()
                    .send_error(req_no, req.rpc_type, "not found")
                    .await?
            }
            Err(err) => {
                let msg = format!("{}", err);
                api.rpc().send_error(req_no, req.rpc_type, &msg).await?
            }
        };
        Ok(true)
    }
}
