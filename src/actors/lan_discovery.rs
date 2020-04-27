use get_if_addrs::{get_if_addrs, IfAddr};

use std::string::ToString;
use std::time::Duration;

use async_std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    task,
};
use futures::FutureExt;

use regex::Regex;

use kuska_ssb::{
    crypto::ToSodiumObject,
    keystore::OwnedIdentity,
    crypto::{
        ed25519
    }    
};

use crate::broker::*;
use crate::error::{SolarError,SolarResult};

pub async fn actor(server_id: OwnedIdentity, rpc_port: u16) -> SolarResult<()> {
    let broadcaster = Broadcaster::collect_interfaces(&server_id.pk, rpc_port).await?;

    let broker = BROKER.lock().await.register("lan_discover", false).await?;    
    let mut ch_terminate = broker.ch_terminate.fuse();

    loop {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}",rpc_port)).await?;
        socket.set_broadcast(true)?;
        let mut buf = [0; 256];

        select_biased! {
            _ = ch_terminate => break,
            recv = socket.recv_from(&mut buf).fuse() => {
                if let Ok((amt, _)) = recv {
                    if let Err(err) = process_broadcast(&server_id,&buf[..amt]).await {
                        warn!("failed to process brodcast: {:?}",err);
                    }
                }
            }
            _ = task::sleep(Duration::from_secs(5)).fuse() => {}    
        }
        drop(socket);
        broadcaster.broadcast().await;
    }
    let _ = broker.ch_terminated.send(Void {});
    Ok(())
}

struct Broadcaster {
    destination: String,
    packets : Vec<(SocketAddr,SocketAddr,String)>
}
impl Broadcaster {
    pub async fn collect_interfaces(id: &ed25519::PublicKey, rpc_port: u16) -> SolarResult<Self> {
        let server_pk = base64::encode(&id);
    
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
                let msg = format!("net:{}:{}~shs:{}", local, rpc_port, server_pk);
                match UdpSocket::bind(SocketAddr::new(local, rpc_port)).await {
                    Ok(_) => packets.push((local_addr, broadcast_addr, msg)),
                    Err(err) => warn!("cannot broadcast to {:?} {:?}", local_addr, err),
                };
            }
        }
        let destination = format!("255.255.255.255:{}",rpc_port);
        Ok(Broadcaster{packets,destination})
    }  
    pub async fn broadcast(&self) {
        for msg in &self.packets {
            if let Ok(socket) = UdpSocket::bind(msg.0).await {
                let _ = socket.set_broadcast(true);
                match socket.send_to(msg.2.as_bytes(),&self.destination).await {
                    Err(err) => warn!(target:"solar", "Error broadcasting {}",err),
                    _ => {},
                }
            }
        }        
    }  
}

fn parse_broadcast(msg: &str) -> Option<(String,u32,ed25519::PublicKey)> {

    let broadcast_shs_regexp =
        r"net:([0-9]+\.[0-9]+\.[0-9]+\.[0-9]+):([0-9]+)~shs:([0-9a-zA-Z=/]+)";
    let parse_shs = |addr: &str| -> SolarResult<_> {
        let captures = Regex::new(broadcast_shs_regexp)?
            .captures(&addr)
            .ok_or_else(|| SolarError::new("cannot parse broadcast"))?;

        let ip = captures[1].to_string();
        let port = captures[2].parse::<u32>()?;
        let server_pk = captures[3].to_ed25519_pk_no_suffix()?;

        Ok((ip,port,server_pk))
    };

    for addr in msg.split(';') {
        if let Ok(shs) = parse_shs(addr) {
            return Some(shs)
        }
    }

    None
}

async fn process_broadcast(server_id: &OwnedIdentity, buff: &[u8]) -> SolarResult<()>{
    let msg = String::from_utf8_lossy(buff);

    if let Some((server,port,peer_pk)) = parse_broadcast(&msg) {   
        Broker::spawn(super::peer::actor(server_id.clone(),super::peer::Connect::TcpServer{server,port,peer_pk}));
    } else {
        warn!("failed to parse broadcast {}",msg);
    }

    Ok(())
}
