use get_if_addrs::{get_if_addrs, IfAddr};

use std::{string::ToString};
use std::time::Duration;

use async_std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    task,
};
use futures::{FutureExt};

use crate::registry::*;

const ACTOR_BROADCAST_PEERS: usize = 1;

pub async fn actor(mut registry_ch: RegistryCh, server_pk: String, rpc_port: u16) -> AnyResult<()> {
    let mut packets = Vec::new();

    for if_addr in get_if_addrs()? {
        let addrs = match if_addr.addr {
            IfAddr::V4(v4) if !v4.is_loopback() && v4.broadcast.is_some() => {
                Some((IpAddr::V4(v4.ip), IpAddr::V4(v4.broadcast.unwrap())))
            }
            IfAddr::V6(v6) if !v6.is_loopback() && v6.broadcast.is_some() => {
                Some((IpAddr::V6(v6.ip), IpAddr::V6(v6.broadcast.unwrap())))
            }
            _ => None,
        };

        if let Some((local, broadcast)) = addrs {
            let local_addr = SocketAddr::new(local, rpc_port);
            let broadcast_addr = SocketAddr::new(broadcast, rpc_port);
            let msg = format!("net:{}:8008~shs:{}", local, server_pk);
            match UdpSocket::bind(SocketAddr::new(local, rpc_port)).await {
                Ok(_) => packets.push((local_addr, broadcast_addr, msg)),
                Err(err) => warn!("cannot broadcast to {:?} {:?}", local_addr, err),
            };
        }
    }
    let broadcast_list = packets.iter().map(|(_,broadcast,_)| broadcast.to_string()).collect::<Vec<_>>().join(",");
    info!("broadcast will be sent to {}",broadcast_list);

    let (terminate, terminated) = Registry::register(&mut registry_ch, ACTOR_BROADCAST_PEERS).await?;
    let mut terminate = terminate.fuse();

    loop {
        select_biased! {
          _ = terminate => break,
          _ = task::sleep(Duration::from_secs(1)).fuse() => {
            debug!("sending broadcast");
            for msg in &packets {
                if let Ok(socket) = UdpSocket::bind(msg.0).await {
                    let _ = socket.set_broadcast(true);
                    match socket.send_to(msg.2.as_bytes(),"255.255.255.255:8008").await {
                        Err(err) => debug!("err {}",err),
                        _ => {},
                    }
                }
            }
          }
        }
    }
    let _ = terminated.send(Void {});
    Ok(())
}
