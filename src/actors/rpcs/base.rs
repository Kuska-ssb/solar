use async_std::{
    io::{Read, Write},
};

use kuska_ssb::{
    api::ApiHelper,
    rpc::RecvMsg,
};
use async_trait::async_trait;

use crate::registry::*;

#[derive(Debug)]
pub enum RpcInput {
    None,
    Network(i32,RecvMsg),
    Storage(String)
}

#[async_trait]
pub trait RpcHandler<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync> : Send + Sync {
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool>;
}

