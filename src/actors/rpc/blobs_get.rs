use async_std::io::{Read, Write};
use std::collections::HashSet;
use std::marker::PhantomData;

use async_trait::async_trait;
use kuska_ssb::{
    api::{dto, ApiHelper, ApiMethod},
    rpc,
};

use crate::error::SolarResult;
use crate::BLOB_STORAGE;
use crate::broker::ChBrokerSend;
use super::{RpcHandler, RpcInput};

pub struct BlobsGetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    blob_reqs: HashSet<i32>,
    phantom: PhantomData<(R, W)>,
}

impl<R, W> Default for BlobsGetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn default() -> Self {
        Self {
            blob_reqs: HashSet::new(),
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<R, W> RpcHandler<R, W> for BlobsGetHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "BlobsGetHandler"
    }

    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput, _ch_broker: &mut ChBrokerSend) -> SolarResult<bool> {
        match op {
            RpcInput::Network(req_no, rpc::RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::BlobsGet) => self.recv_get(api, *req_no, req).await,
                    _ => Ok(false),
                }
            }
            RpcInput::Network(req_no, rpc::RecvMsg::CancelStreamRespose()) => {
                self.recv_cancelstream(api, *req_no).await
            }
            _ => Ok(false),
        }
    }
}

impl<R, W> BlobsGetHandler<R, W>
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
        let mut args: Vec<dto::BlobsGetIn> = serde_json::from_value(req.args.clone())?;
        let args = args.pop().unwrap();

        trace!(target: "ssb-blob", "requested blob {}",args.key);

        let data = BLOB_STORAGE.read().await.get(&args.key)?;
        if let Some(expected_size) = args.size {
            if data.len() != expected_size as usize {
                trace!(target: "ssb-blob", "not sending blob: blob.len != expected");
                api.rpc()
                    .send_error(req_no, req.rpc_type, "blob.len != expected")
                    .await?;
                return Ok(true);
            }
        }
        if let Some(max) = args.max {
            if data.len() > max as usize {
                trace!(target: "ssb-blob", "not sending blob: blob.len > max");
                api.rpc()
                    .send_error(req_no, req.rpc_type, "blob.len > max")
                    .await?;
                return Ok(true);
            }
        }
        api.blobs_get_res_send(req_no, &data).await?;
        self.blob_reqs.insert(req_no);

        info!("Sent blob {}", args.key);

        Ok(true)
    }


    async fn recv_cancelstream(
        &mut self,
        _api: &mut ApiHelper<R, W>,
        req_no: i32,
    ) -> SolarResult<bool> {
        Ok(self.blob_reqs.remove(&req_no))
    }
}