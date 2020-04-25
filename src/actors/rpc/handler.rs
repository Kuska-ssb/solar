use async_std::io::{Read, Write};

use async_trait::async_trait;
use kuska_ssb::{api::ApiHelper, rpc::RecvMsg};

use crate::error::SolarResult;
use crate::storage::kv::StorageEvent;

#[derive(Debug)]
pub enum RpcInput {
    None,
    Timer,
    Network(i32, RecvMsg),
    Storage(StorageEvent),
}

#[async_trait]
pub trait RpcHandler<R, W>: Send + Sync
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str;
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput) -> SolarResult<bool>;
}
