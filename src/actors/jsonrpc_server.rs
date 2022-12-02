// src/actors/json_rpc_server.rs

use async_std::task;
use futures::FutureExt;
use jsonrpc_ws_server::jsonrpc_core::*;
use jsonrpc_ws_server::ServerBuilder;

use kuska_ssb::keystore::OwnedIdentity;

use crate::{broker::*, Result};

/// Register the JSON-RPC server endpoint, define the JSON-RPC methods
/// and spawn the server.
///
/// Listens for a termination signal from the broker. When received, the
/// JSON-RPC server is closed and a terminated signal is sent to the broker.
pub async fn actor(server_id: OwnedIdentity, port: u16) -> Result<()> {
    let broker = BROKER
        .lock()
        .await
        .register("jsonrpc-listener", false)
        .await?;

    let ch_terminate = broker.ch_terminate.fuse();

    let mut io = IoHandler::default();

    // Simple `ping` endpoint.
    io.add_sync_method("ping", |_| Ok(Value::String("pong!".to_owned())));

    // Return the public key of the local SSB server.
    io.add_sync_method("whoami", move |_| {
        Ok(Value::String(server_id.id.to_owned()))
    });

    let server_addr = format!("0.0.0.0:{}", port);
    let server = ServerBuilder::new(io).start(&server_addr.parse()?)?;

    // Create a close handle to be used when the termination signal is
    // received.
    let close_handle = server.close_handle();

    // Start the JSON-RPC server in a task.
    // This allows us to listen for the termination signal (without blocking).
    task::spawn(async {
        server.wait().expect("json-rpc server startup failed");
    });

    // Listen for termination signal from broker.
    if let Err(err) = ch_terminate.await {
        warn!("ch_terminate sender dropped: {}", err)
    }

    // When received, close (stop) the server.
    close_handle.close();

    // Then send terminated signal back to broker.
    let _ = broker.ch_terminated.send(Void {});

    Ok(())
}
