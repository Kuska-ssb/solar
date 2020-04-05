use async_std::{
    io::{Read, Write},
};
use async_trait::async_trait;

use kuska_ssb::{
    api::{ApiHelper, ApiMethod},
    rpc::RecvMsg,
};

use crate::registry::*;
use super::{RpcHandler,RpcInput};

pub struct WhoAmIHandler<'a> {
    peer_ssb_id: &'a String
}

impl<'a> WhoAmIHandler<'a> {
    pub fn new(peer_ssb_id : &'a String) -> Self {
        Self { peer_ssb_id }
    }
}

#[async_trait]
impl<'a,R: Read + Unpin+ Send + Sync , W: Write + Unpin+ Send + Sync > RpcHandler<R,W> for WhoAmIHandler<'a> {
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool> {
        match op {
            RpcInput::Network(req_no, RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::WhoAmI) => {
                        api.whoami_res_send(*req_no, self.peer_ssb_id.clone()).await?;
                        Ok(true)
                    }
                    _ => Ok(false)
                }
            }
            _ => Ok(false)
        }
    }
}

