use crate::registry::*;

use async_ctrlc::CtrlC;
use futures::SinkExt;

pub async fn actor() -> AnyResult<()> {
    let mut reg = REGISTRY.lock().await.register("crtlc").await?;

    let ctrlc = CtrlC::new().expect("cannot create Ctrl+C handler?");
    ctrlc.await;
    println!("Got CTRL-C, sending termination signal to jobs...");
    let _ = reg.ch.send(Event::Terminate).await;
    Ok(())
}
