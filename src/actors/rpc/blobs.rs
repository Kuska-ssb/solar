use async_std::io::{Read, Write};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use async_trait::async_trait;
use kuska_ssb::{
    api::{dto, ApiHelper, ApiMethod},
    rpc,
};

use crate::error::SolarResult;
use crate::BLOB_STORAGE;

use super::{RpcHandler, RpcInput};

pub struct BlobsHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    blob_reqs: HashSet<i32>,
    peer_wants_req_no: Option<i32>,
    my_wants_req_no: Option<i32>,
    phantom: PhantomData<(R, W)>,
}

impl<R, W> Default for BlobsHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn default() -> Self {
        Self {
            my_wants_req_no: None,
            peer_wants_req_no: None,
            blob_reqs: HashSet::new(),
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<R, W> RpcHandler<R, W> for BlobsHandler<R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "BlobsHandler"
    }

    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> SolarResult<bool> {
        match op {
            RpcInput::Network(req_no, rpc::RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::BlobsCreateWants) => {
                        self.recv_create_wants(api, *req_no, req).await
                    }
                    Some(ApiMethod::BlobsGet) => self.recv_get(api, *req_no, req).await,
                    _ => Ok(false),
                }
            }
            RpcInput::Network(req_no, rpc::RecvMsg::RpcResponse(xtype, data)) => {
                self.recv_rpc_response(api, *req_no, *xtype, data).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::CancelStreamRespose()) => {
                self.recv_cancelstream(api, *req_no).await
            }
            RpcInput::Network(req_no, rpc::RecvMsg::ErrorResponse(err)) => {
                if Some(*req_no) == self.my_wants_req_no
                    || Some(*req_no) == self.peer_wants_req_no
                    || self.blob_reqs.contains(req_no)
                {
                    warn!("BlobsHandler got error {}", err);
                }
                Ok(true)
            }
            RpcInput::Timer => {
                if self.my_wants_req_no.is_none() {
                    trace!(target: "ssb-blob", "sending create wants");
                    let req_no = api.blob_create_wants_req_send().await?;
                    self.my_wants_req_no = Some(req_no);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}

impl<R, W> BlobsHandler<R, W>
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

    async fn recv_create_wants(
        &mut self,
        _api: &mut ApiHelper<R, W>,
        req_no: i32,
        _req: &rpc::Body,
    ) -> SolarResult<bool> {
        if self.peer_wants_req_no.is_none() {
            trace!(target: "ssb-blob", "recieved create wants");
            self.peer_wants_req_no = Some(req_no);
        } else {
            trace!(target: "ssb-blob", "peer create wants already received");
        }
        Ok(true)
    }

    async fn recv_rpc_response(
        &mut self,
        api: &mut ApiHelper<R, W>,
        req_no: i32,
        _xtype: rpc::BodyType,
        data: &[u8],
    ) -> SolarResult<bool> {
        if self.my_wants_req_no != Some(req_no) {
            return Ok(false);
        }
        if let Some(peer_wants_req_no) = self.peer_wants_req_no {
            let wants: HashMap<String, i64> = serde_json::from_slice(data)?;
            let mut haves: HashMap<String, u64> = HashMap::new();

            trace!(target: "ssb-blob", "wants:{:?}",wants);
            for (want, _distance) in wants {
                if let Some(size) = BLOB_STORAGE.read().await.size_of(&want)? {
                    haves.insert(want, size);
                }
            }
            trace!(target: "ssb-blob", "haves:{:?}",haves);

            api.rpc()
                .send_response(
                    peer_wants_req_no,
                    rpc::RpcType::Source,
                    rpc::BodyType::JSON,
                    &serde_json::to_vec(&haves)?,
                )
                .await?;
        } else {
            trace!(target: "ssb-blob", "*warn cannot anwser 'wants' cause there's no peer channel yet to respond");
        }
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