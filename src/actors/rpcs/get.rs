use async_std::{
    io::{Read, Write},
};

use kuska_ssb::{
    api::{ApiHelper, ApiMethod},
    rpc::RecvMsg,
};
use async_trait::async_trait;

use crate::registry::*;
use crate::storage::DB;

use super::{RpcHandler,RpcInput};
pub struct GetHandler{}

impl Default for GetHandler {
    fn default() -> Self {
        Self{}
    }
}

#[async_trait]
impl<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync> RpcHandler<R,W> for GetHandler {
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool> {
        match op {
            RpcInput::Network(req_no, RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::Get) => {
                        let args: Vec<String> = serde_json::from_value(req.args.clone())?;
                        let msg = DB.read().await.get_message(&args[0]);
                        match msg {
                            Ok(msg) => api.get_res_send(*req_no, &msg).await?,
                            Err(err) => {
                                let msg = format!("{}", err);
                                api.rpc().send_error(*req_no, req.rpc_type, &msg).await?
                            }
                        };
                        Ok(true)
                    }
                    _ => Ok(false)
                }
            }
            _ => Ok(false)
        }
    }
}

