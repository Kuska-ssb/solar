use std::marker::PhantomData;

use async_std::io::{Read, Write};
use async_trait::async_trait;

use kuska_ssb::{
    api::{ApiHelper, ApiMethod},
    rpc::RecvMsg,
};

use super::{RpcHandler, RpcInput};
use crate::error::AnyResult;

pub struct WhoAmIHandler<'a, R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    peer_ssb_id: &'a str,
    phantom: PhantomData<(R, W)>,
}

impl<'a, R, W> WhoAmIHandler<'a, R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    pub fn new(peer_ssb_id: &'a str) -> Self {
        Self {
            peer_ssb_id,
            phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<'a, R, W> RpcHandler<R, W> for WhoAmIHandler<'a, R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool> {
        match op {
            RpcInput::Network(req_no, RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::WhoAmI) => self.recv_whoami(api, *req_no).await,
                    _ => Ok(false),
                }
            }
            _ => Ok(false),
        }
    }
}

impl<'a, R, W> WhoAmIHandler<'a, R, W>
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    async fn recv_whoami(&mut self, api: &mut ApiHelper<R, W>, req_no: i32) -> AnyResult<bool> {
        api.whoami_res_send(req_no, self.peer_ssb_id.to_string())
            .await?;
        Ok(true)
    }
}
