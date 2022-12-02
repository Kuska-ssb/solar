use once_cell::sync::Lazy;
use std::{collections::HashSet, time::Duration};

use async_std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, RwLock},
    task,
};
use futures::stream::StreamExt;

use futures::{FutureExt, SinkExt};

use kuska_ssb::{
    api::ApiCaller,
    crypto::{ed25519, ToSsbId},
    discovery::ssb_net_id,
    handshake::{
        async_std::{handshake_client, handshake_server, BoxStream},
        HandshakeComplete,
    },
    keystore::OwnedIdentity,
    rpc::{RpcReader, RpcWriter},
};

use crate::{broker::*, Result};

use super::rpc::{
    BlobsGetHandler, BlobsWantsHandler, GetHandler, HistoryStreamHandler, RpcHandler, RpcInput,
    WhoAmIHandler,
};

pub enum Connect {
    TcpServer {
        server: String,
        port: u32,
        peer_pk: ed25519::PublicKey,
    },
    ClientStream {
        stream: TcpStream,
    },
}

/// A list (`HashSet`) of public keys representing peers to whom we are
/// currently connected.
pub static CONNECTED_PEERS: Lazy<Arc<RwLock<HashSet<ed25519::PublicKey>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashSet::new())));

pub async fn actor(id: OwnedIdentity, connect: Connect) -> Result<()> {
    if let Err(err) = actor_inner(id, connect).await {
        warn!("peer failed: {:?}", err);
    }
    Ok(())
}

/// Handle a TCP connection, update the list of connected peers, register the
/// peer actor endpoint, spawn the peer loop and report on the connection
/// outcome.
pub async fn actor_inner(id: OwnedIdentity, connect: Connect) -> Result<()> {
    // Parse the public key and secret key from the identity.
    let OwnedIdentity { pk, sk, .. } = id;

    // Handle a TCP connection event (inbound or outbound).
    let (stream, handshake) = match connect {
        // Handle an outgoing TCP connection event.
        Connect::TcpServer {
            server,
            port,
            peer_pk,
        } => {
            // First check if we are already connected to the selected peer.
            // If yes, return immediately.
            // If no, continue with the connection attempt.
            if CONNECTED_PEERS.read().await.contains(&peer_pk) {
                return Ok(());
            }

            // Define the server address and port.
            let server_port = format!("{}:{}", server, port);
            // Attempt a TCP connection.
            let mut stream = TcpStream::connect(server_port).await?;
            // Attempt a secret handshake.
            let handshake = handshake_client(&mut stream, ssb_net_id(), pk, sk, peer_pk).await?;

            info!("💃 connected to peer {}", handshake.peer_pk.to_ssb_id());

            (stream, handshake)
        }
        // Handle an incoming TCP connection event.
        Connect::ClientStream { mut stream } => {
            // Attempt a secret handshake.
            let handshake = handshake_server(&mut stream, ssb_net_id(), pk, sk).await?;
            // Check if we are already connected to the selected peer.
            // If yes, return immediately.
            // If no, return the stream and handshake.
            if CONNECTED_PEERS.read().await.contains(&handshake.peer_pk) {
                return Ok(());
            }

            info!(
                "💃 received connection from peer {}",
                handshake.peer_pk.to_ssb_id()
            );

            (stream, handshake)
        }
    };

    // Register the "peer" actor endpoint with the broker.
    let ActorEndpoint {
        ch_terminate,
        mut ch_broker,
        ch_msg,
        actor_id,
        ..
    } = BROKER.lock().await.register("peer", true).await?;

    // Parse the peer public key from the handshake.
    let peer_pk = handshake.peer_pk;

    // Add the peer to the list of connected peers.
    CONNECTED_PEERS.write().await.insert(peer_pk);

    // Spawn the peer loop (responsible for negotiating RPC requests).
    let res = peer_loop(
        actor_id,
        &stream,
        &stream,
        handshake,
        ch_terminate,
        ch_msg.unwrap(),
    )
    .await;

    // Remove the peer from the list of connected peers.
    CONNECTED_PEERS.write().await.remove(&peer_pk);

    if let Err(err) = res {
        warn!("💀 client terminated with error {:?}", err);
    } else {
        info!("👋 finished connection with {}", &peer_pk.to_ssb_id());
    }

    let _ = ch_broker.send(BrokerEvent::Disconnect { actor_id }).await;

    Ok(())
}

async fn peer_loop<R: Read + Unpin + Send + Sync, W: Write + Unpin + Send + Sync>(
    actor_id: usize,
    reader: R,
    writer: W,
    handshake: HandshakeComplete,
    ch_terminate: ChSigRecv,
    mut ch_msg: ChMsgRecv,
) -> Result<()> {
    // Parse the peer public key from the handshake.
    let peer_ssb_id = handshake.peer_pk.to_ssb_id();

    // Instantiate a box stream and split it into reader and writer streams.
    let (box_stream_read, box_stream_write) =
        BoxStream::from_handshake(reader, writer, handshake, 0x8000).split_read_write();

    // Instantiate RPC reader and writer using the box streams.
    let rpc_reader = RpcReader::new(box_stream_read);
    let rpc_writer = RpcWriter::new(box_stream_write);
    let mut api = ApiCaller::new(rpc_writer);

    // Instantiate the MUXRPC handlers.
    let mut history_stream_handler = HistoryStreamHandler::new(actor_id);
    let mut whoami_handler = WhoAmIHandler::new(&peer_ssb_id);
    let mut get_handler = GetHandler::default();
    let mut blobs_get_handler = BlobsGetHandler::default();
    let mut blobs_wants_handler = BlobsWantsHandler::default();

    let mut handlers: Vec<&mut dyn RpcHandler<W>> = vec![
        &mut history_stream_handler,
        &mut whoami_handler,
        &mut get_handler,
        &mut blobs_get_handler,
        &mut blobs_wants_handler,
    ];

    // Create channel to send messages to broker.
    let mut ch_broker = BROKER.lock().await.create_sender();
    // Fuse internal termination channel with external channel.
    // This allows termination of the peer loop to be initiated from outside
    // this function.
    let mut ch_terminate_fuse = ch_terminate.fuse();

    // Convert the box stream reader into a stream.
    let rpc_recv_stream = rpc_reader.into_stream().fuse();
    pin_mut!(rpc_recv_stream);

    loop {
        // Poll multiple futures and streams simultaneously, executing the
        // branch for the future that finishes first. If multiple futures are
        // ready, one will be selected in order of declaration.
        let input = select_biased! {
            _value = ch_terminate_fuse =>  {
                break;
            },
            packet = rpc_recv_stream.select_next_some() => {
                let (rpc_id, packet) = packet;
                RpcInput::Network(rpc_id,packet)
            },
            msg = ch_msg.next().fuse() => {
                if let Some(msg) = msg {
                    RpcInput::Message(msg)
                } else {
                    RpcInput::None
                }
            },
            _ = task::sleep(Duration::from_secs(1)).fuse() => {
                RpcInput::Timer
            },
        };

        let mut handled = false;
        for handler in handlers.iter_mut() {
            match handler.handle(&mut api, &input, &mut ch_broker).await {
                Ok(has_been_handled) => {
                    if has_been_handled {
                        handled = true;
                        break;
                    }
                }
                Err(err) => {
                    error!("handler {} failed with {:?}", handler.name(), err);
                }
            }
        }
        if !handled {
            trace!("message not processed: {:?}", input);
        }
    }

    Ok(())
}
