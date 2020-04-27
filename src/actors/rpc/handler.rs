use async_std::io::{Read, Write};

use async_trait::async_trait;
use kuska_ssb::{api::ApiHelper, rpc::RecvMsg};
use crate::broker::{ChBrokerSend,BrokerMessage};
use crate::error::SolarResult;

#[derive(Debug)]
pub enum RpcInput {
    None,
    Timer,
    Network(i32, RecvMsg),
    Message(BrokerMessage),
}

#[async_trait]
pub trait RpcHandler<R, W>: Send + Sync
where
    R: Read + Unpin + Send + Sync,
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str;
    async fn handle(&mut self, api: &mut ApiHelper<R, W>, op: &RpcInput,ch_broker: &mut ChBrokerSend) -> SolarResult<bool>;
}
