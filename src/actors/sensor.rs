use std::fs::File;
use std::io::Read;
use std::time::Duration;

use async_std::task;
use futures::FutureExt;
use plotters::prelude::*;
use slice_deque::SliceDeque;

use kuska_ssb::{api::dto::content::Post, feed::Message, keystore::OwnedIdentity};

use crate::broker::*;
use crate::error::SolarResult;
use crate::{BLOB_STORAGE, KV_STORAGE};

pub async fn actor(server_id: OwnedIdentity) -> SolarResult<()> {
    let broker = BROKER.lock().await.register("sensor", false).await?;
    let mut ch_terminate = broker.ch_terminate.fuse();
    let mut data: SliceDeque<u64> = SliceDeque::with_capacity(20);
    loop {
        select_biased! {
          _ = ch_terminate => break,
          _ = task::sleep(Duration::from_secs(5)).fuse() => {
              if let Err(err) = sensor_proc(&server_id,&mut data).await {
                warn!("sensor proc finished with error {:?}",err);
              }
            }
        }
    }
    let _ = broker.ch_terminated.send(Void {});
    Ok(())
}

async fn sensor_proc(server_id: &OwnedIdentity, data: &mut SliceDeque<u64>) -> SolarResult<()> {
    let feed_storage = KV_STORAGE.write().await;

    let last_msg = if let Some(last_id) = feed_storage.get_feed_len(&server_id.id)? {
        Some(
            feed_storage
                .get_feed(&server_id.id, last_id)?
                .unwrap()
                .into_message()?
        )
    } else {
        None
    };

    let mem_available = procfs::Meminfo::new()?.mem_available.unwrap();
    if data.len() == 20 {
        data.pop_front();
    }
    data.push_back(mem_available % 20);

    let png = create_graphics(data.as_slice())?;
    let blob_id = BLOB_STORAGE.write().await.insert(&png).await?;

    let markdown = format!(
        "Sensor recording at {:?} ... current temperature is {:?}
      ![graphics.png]({})",
        std::time::SystemTime::now(),
        mem_available,
        blob_id
    );
    let post = Post::new(markdown, None).to_msg()?;
    let msg = Message::sign(last_msg.as_ref(), &server_id, post)?;
    let next_id = feed_storage.append_feed(msg).await?;

    info!("Recoding sensor data {} ...", next_id);

    Ok(())
}

fn create_graphics(data: &[u64]) -> SolarResult<Vec<u8>> {
    let mut image_path = std::env::temp_dir();
    image_path.push("sonarsewarnnsor");
    image_path.set_extension("png");

    let root = BitMapBackend::new(&image_path, (300, 200)).into_drawing_area();

    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .x_label_area_size(10)
        .y_label_area_size(10)
        .margin(5)
        .caption(
            format!("{:?}", std::time::SystemTime::now()),
            ("sans-serif", 10.0).into_font(),
        )
        .build_ranged(0u32..10u32, 0u32..10u32)?;

    chart
        .configure_mesh()
        .disable_x_mesh()
        .line_style_1(&WHITE.mix(0.3))
        .x_label_offset(30)
        .y_desc("Count")
        .x_desc("Bucket")
        .axis_desc_style(("sans-serif", 15).into_font())
        .draw()?;

    chart.draw_series(
        Histogram::vertical(&chart)
            .style(RED.mix(0.5).filled())
            .data(data.iter().map(|x: &u64| (*x as u32, 1))),
    )?;
    drop(chart);

    let mut file = File::open(&image_path)?;
    let len = file.metadata()?.len() as usize;
    let mut content = Vec::with_capacity(len);
    file.read_to_end(&mut content)?;

    Ok(content)
}
