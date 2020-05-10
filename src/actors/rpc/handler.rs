use async_std::io::Write;

use crate::broker::{BrokerMessage, ChBrokerSend};
use crate::error::SolarResult;
use async_trait::async_trait;
use kuska_ssb::{api::ApiCaller, rpc::RecvMsg};

#[derive(Debug)]
pub enum RpcInput {
    None,
    Timer,
    Network(i32, RecvMsg),
    Message(BrokerMessage),
}

#[async_trait]
pub trait RpcHandler<W>: Send + Sync
where
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str;
    async fn handle(
        &mut self,
        api: &mut ApiCaller<W>,
        op: &RpcInput,
        ch_broker: &mut ChBrokerSend,
    ) -> SolarResult<bool>;
}
