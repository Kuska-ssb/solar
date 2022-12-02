#![allow(clippy::single_match)]

use std::time::Duration;

use async_std::{net::UdpSocket, task};
use futures::FutureExt;

use kuska_ssb::{discovery::LanBroadcast, keystore::OwnedIdentity};

use crate::{broker::*, Result};

pub async fn actor(server_id: OwnedIdentity, rpc_port: u16) -> Result<()> {
    let broadcaster = LanBroadcast::new(&server_id.pk, rpc_port).await?;

    let broker = BROKER.lock().await.register("lan_discover", false).await?;
    let mut ch_terminate = broker.ch_terminate.fuse();

    loop {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", rpc_port)).await?;
        socket.set_broadcast(true)?;
        let mut buf = [0; 256];

        select_biased! {
            _ = ch_terminate => break,
            recv = socket.recv_from(&mut buf).fuse() => {
                if let Ok((amt, _)) = recv {
                    if let Err(err) = process_broadcast(&server_id,&buf[..amt]).await {
                        warn!("failed to process broadcast: {:?}",err);
                    }
                }
            }
            _ = task::sleep(Duration::from_secs(5)).fuse() => {}
        }
        drop(socket);
        broadcaster.send().await;
    }
    let _ = broker.ch_terminated.send(Void {});
    Ok(())
}
async fn process_broadcast(server_id: &OwnedIdentity, buff: &[u8]) -> Result<()> {
    let msg = String::from_utf8_lossy(buff);

    if let Some((server, port, peer_pk)) = LanBroadcast::parse(&msg) {
        Broker::spawn(super::peer::actor(
            server_id.clone(),
            super::peer::Connect::TcpServer {
                server,
                port,
                peer_pk,
            },
        ));
    } else {
        warn!("failed to parse broadcast {}", msg);
    }

    Ok(())
}
