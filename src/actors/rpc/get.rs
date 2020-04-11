use async_std::io::{Read, Write};
use std::marker::PhantomData;

use async_trait::async_trait;
use kuska_ssb::{
    api::{ApiHelper, ApiMethod},
    rpc,
};

use crate::error::SolarResult;
use crate::FEED_STORAGE;

use super::{RpcHandler, RpcInput};

pub struct GetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    phantom: PhantomData<(R, W)>,
}

impl<R, W> Default for GetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn default() -> Self {
        Self {
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<R, W> RpcHandler<R, W> for GetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "GetHandler"
    }

    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> SolarResult<bool> {
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

impl<R, W> GetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    async fn recv_get(
        &mut self,
        api: &mut ApiHelper<R, W>,
        req_no: i32,
        req: &rpc::Body,
    ) -> SolarResult<bool> {
        let args: Vec<String> = serde_json::from_value(req.args.clone())?;
        let msg = FEED_STORAGE.read().await.get_message(&args[0]);
        match msg {
            Ok(msg) => api.get_res_send(req_no, &msg).await?,
            Err(err) => {
                let msg = format!("{}", err);
                api.rpc().send_error(req_no, req.rpc_type, &msg).await?
            }
        };
        Ok(true)
    }
}
