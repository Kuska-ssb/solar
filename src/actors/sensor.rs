use std::time::Duration;

use async_std::task;
use futures::FutureExt;

use kuska_ssb::{
    api::{msgs::Post},
    feed::Message,
    keystore::OwnedIdentity,
};

use crate::storage::DB;
use crate::registry::*;

const ACTOR_GENERATOR: usize = 2;

pub async fn actor(mut registry_ch: RegistryCh, server_id: OwnedIdentity) -> AnyResult<()> {
    let (terminate, terminated) = Registry::register(&mut registry_ch, ACTOR_GENERATOR).await?;
    let mut terminate = terminate.fuse();

    loop {
        select_biased! {
          _ = terminate => break,
          _ = task::sleep(Duration::from_secs(5)).fuse() => {
              let db = DB.write().await;

              let last_msg =  if let Some(last_id) = db.get_feed_len(&server_id.id)? {
                Some(db.get_feed(&server_id.id, last_id)?.into_message()?)
              } else {
                None
              };

              let markdown = format!("Sensor recording... current temperature is {:?}",std::time::SystemTime::now()); 
              let post = Post::new(markdown, Vec::new()).to_msg()?;
              let msg = Message::sign(last_msg.as_ref(), &server_id, post)?;
              info!("Adding {:?}",msg);
              let next_id = db.append_feed(msg)?;

              println!("Recoding sensor data {} ...",next_id);

              drop(db)
          }
        }
    }
    let _ = terminated.send(Void {});
    Ok(())
}
