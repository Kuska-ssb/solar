use std::marker::PhantomData;

use async_std::io::Write;
use async_trait::async_trait;

use kuska_ssb::{
    api::{ApiHelper, ApiMethod},
    rpc::RecvMsg,
};

use super::{RpcHandler, RpcInput};
use crate::error::SolarResult;
use crate::broker::ChBrokerSend;

pub struct WhoAmIHandler<'a, W>
where
    W: Write + Unpin + Send + Sync,
{
    peer_ssb_id: &'a str,
    phantom: PhantomData<W>,
}

impl<'a, W> WhoAmIHandler<'a, W>
where
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
impl<'a, W> RpcHandler<W> for WhoAmIHandler<'a,W>
where
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "WhoAmIHandler"
    }

    async fn handle(&mut self, api: &mut ApiHelper<W>, op: &RpcInput, _ch_broker: &mut ChBrokerSend) -> SolarResult<bool> {
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

impl<'a, W> WhoAmIHandler<'a, W>
where
    W: Write + Unpin + Send + Sync,
{
    async fn recv_whoami(&mut self, api: &mut ApiHelper<W>, req_no: i32) -> SolarResult<bool> {
        api.whoami_res_send(req_no, self.peer_ssb_id.to_string())
            .await?;
        Ok(true)
    }
}
