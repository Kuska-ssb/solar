use std::time::Duration;

use async_std::task;
use futures::FutureExt;

use kuska_ssb::{api::msgs::Post, feed::Message, keystore::OwnedIdentity};

use crate::registry::*;
use crate::storage::DB;

pub async fn actor(server_id: OwnedIdentity) -> AnyResult<()> {
    let reg = REGISTRY.lock().await.register("sensor").await?;
    let mut terminate = reg.terminate.fuse();

    loop {
        select_biased! {
          _ = terminate => break,
          _ = task::sleep(Duration::from_secs(5000)).fuse() => {
              let db = DB.write().await;

              let last_msg =  if let Some(last_id) = db.get_feed_len(&server_id.id)? {
                Some(db.get_feed(&server_id.id, last_id)?.into_message()?)
              } else {
                None
              };

              let markdown = format!("Sensor recording... current temperature is {:?}",std::time::SystemTime::now());
              let post = Post::new(markdown, None).to_msg()?;
              let msg = Message::sign(last_msg.as_ref(), &server_id, post)?;
              info!("Adding {:?}",msg);
              let next_id = db.append_feed(msg)?;

              println!("Recoding sensor data {} ...",next_id);

              drop(db)
          }
        }
    }
    let _ = reg.terminated.send(Void {});
    Ok(())
}
