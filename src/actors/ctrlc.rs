use crate::registry::*;

use futures::{SinkExt};
use async_ctrlc::CtrlC;

pub async fn actor(mut registry_ch: RegistryCh) -> AnyResult<()> {
    let ctrlc = CtrlC::new().expect("cannot create Ctrl+C handler?");
    ctrlc.await;
    println!("Got CTRL-C, sending termination signal to jobs...");
    let _ = registry_ch.send(Event::Terminate).await;
    Ok(())
}
