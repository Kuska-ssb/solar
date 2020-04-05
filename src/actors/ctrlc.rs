use async_ctrlc::CtrlC;
use futures::SinkExt;

use crate::broker::*;
use crate::error::Result;

pub async fn actor() -> Result<()> {
    let mut broker = BROKER.lock().await.register("crtlc",false).await?;

    let ctrlc = CtrlC::new().expect("cannot create Ctrl+C handler?");
    ctrlc.await;
    println!("Got CTRL-C, sending termination signal to jobs...");
    let _ = broker.ch_broker.send(BrokerEvent::Terminate).await;
    Ok(())
}