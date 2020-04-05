use async_std::io::{Read, Write};

use async_trait::async_trait;
use kuska_ssb::{api::ApiHelper, rpc::RecvMsg};

use crate::error::AnyResult;
use crate::storage::StorageEvent;

#[derive(Debug)]
pub enum RpcInput {
    None,
    Network(i32, RecvMsg),
    Storage(StorageEvent),
}

#[async_trait]
pub trait RpcHandler<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync>:
    Send + Sync
{
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> AnyResult<bool>;
}
