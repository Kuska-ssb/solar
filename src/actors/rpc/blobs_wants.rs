#![allow(clippy::single_match)]

use async_std::io::Write;
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::futures::SinkExt;
use async_trait::async_trait;
use kuska_ssb::{
    api::{dto,ApiHelper, ApiMethod},
    rpc
};

use crate::error::SolarResult;
use crate::BLOB_STORAGE;
use crate::broker::ChBrokerSend;
use crate::storage::blob::{StoBlobEvent,ToBlobHashId};

use crate::broker::{Destination,BrokerEvent};
use super::{RpcHandler, RpcInput};

enum RpcBlobsWantsEvent {
    BroadcastWants(Vec<(String, i64)>)
}

#[derive(PartialEq)]
enum Wants {
    Pending,
    Requested(i32),
    Available,
}

/*
+-------+                 +-------------+                +---------+           +-------------+       +-------+
| peer1 |                 | actor_peer1 |                | storage |           | actor_peer2 |       | peer2 |
+-------+                 +-------------+                +---------+           +-------------+       +-------+
    |                            |                            |                       |                  |
    | recv_create_wants()        |                            |                       |                  |
    |--------------------------->|                            |                       |                  |
    |                            |                            |                       |                  |
    | recv_wants()               |                            |                       |                  |
    |--------------------------->|                            |                       |                  |
    |                            |                            |                       |                  |
    |                            | event_wants_broadcast()    |                       |                  |
    |                            |--------------------------------------------------->|                  |
    |                            |                            |                       |                  |
    |                            |                            |                       | wants            |
    |                            |                            |                       |----------------->|
    |                            |                            |                       |                  |
    |                            |                            |                       |     recv_haves() |
    |                            |                            |                       |<-----------------|
    |                            |                            |                       |                  |
    |                            |                            |                       | blobs get        |
    |                            |                            |                       |----------------->|
    |                            |                            |                       |                  |
    |                            |                            |                       | recv_blobs_get() |
    |                            |                            |                       |<-----------------|
    |                            |                            |                       |                  |
    |                            |                            |        store id2 blob |                  |
    |                            |                            |<----------------------|                  |
    |                            |                            |                       |                  |
    |                            |      event_stoblob_added() |                       |                  |
    |                            |<---------------------------|                       |                  |
    |                            |                            |                       |                  |
    |                      haves |                            |                       |                  |
    |<---------------------------|                            |                       |                  |
    |                            |                            |                       |                  |

    https://textart.io/sequence
    object peer1 actor_peer1 storage actor_peer2 peer2 
    peer1 -> actor_peer1: recv_create_wants()
    peer1 -> actor_peer1: recv_wants()
    actor_peer1 -> actor_peer2 : event_wants_broadcast()
    actor_peer2 -> peer2: wants
    peer2 -> actor_peer2: recv_haves()
    actor_peer2 -> peer2: blobs get
    peer2 -> actor_peer2: recv_blobs_get()
    actor_peer2 -> storage: store id2 blob
    storage -> actor_peer1:  event_stoblob_added()
    actor_peer1 -> peer1: haves
*/

pub struct BlobsWantsHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    initialized: bool,
    peer_wants_req_no: Option<i32>,
    my_wants_req_no: Option<i32>,
    peer_wants : HashMap<String,Wants>,
    phantom: PhantomData<W>,
}

impl<W> Default for BlobsWantsHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    fn default() -> Self {
        Self {
            initialized: false,
            my_wants_req_no: None,
            peer_wants_req_no: None,
            phantom: PhantomData,
            peer_wants : HashMap::new(),
        }
    }
}

#[async_trait]
impl<W> RpcHandler<W> for BlobsWantsHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    fn name(&self) -> &'static str {
        "BlobsWantsHandler"
    }

    async fn handle(&mut self, api: &mut ApiHelper<W>, op: &RpcInput, ch_broker: &mut ChBrokerSend) -> SolarResult<bool> {
        match op {
            RpcInput::Network(req_no, rpc::RecvMsg::RpcRequest(req)) => {
                match ApiMethod::from_rpc_body(req) {
                    Some(ApiMethod::BlobsCreateWants) => {
                        return self.recv_create_wants(api, *req_no, req).await;
                    }
                    _ => {}
                }
            }
            RpcInput::Network(req_no, rpc::RecvMsg::RpcResponse(xtype, data)) => {
                if self.my_wants_req_no == Some(*req_no) {
                    return self.recv_wants(api, *req_no, *xtype, data, ch_broker).await
                } else if self.peer_wants_req_no == Some(*req_no) {
                    return self.recv_haves(api, *req_no, *xtype, data, ch_broker).await
                } else if self.peer_wants.values().any(|v| *v == Wants::Requested(*req_no)) {
                    return self.recv_blobs_get(api, *req_no, *xtype, data, ch_broker).await
                }
            }
            RpcInput::Network(req_no, rpc::RecvMsg::ErrorResponse(err)) => {
                if Some(*req_no) == self.my_wants_req_no
                   || Some(*req_no) == self.peer_wants_req_no {
                    warn!("BlobsHandler got error {}", err);
                    return Ok(true)
                }
            }
            RpcInput::Message(msg) => {
                if let Some(wants_event) = msg.downcast_ref::<RpcBlobsWantsEvent>() {
                    match wants_event {
                        RpcBlobsWantsEvent::BroadcastWants(ids) => {
                            return self.event_wants_broadcast(api, &ids).await
                        }
                    }
                }
                else if let Some(stoblob_event) = msg.downcast_ref::<StoBlobEvent>() {
                    match stoblob_event {
                        StoBlobEvent::Added(blob_id) => {
                            return self.event_stoblob_added(api, &blob_id).await
                        }
                    }
                }
            }
            RpcInput::Timer => {
                if !self.initialized {
                    trace!(target: "ssb-blob", "sending create wants");
                    let req_no = api.blob_create_wants_req_send().await?;
                    self.my_wants_req_no = Some(req_no);
                    self.initialized = true;
                    return Ok(false)
                }
            }
            _ => {},
        };
        Ok(false)
    }
}

impl<W> BlobsWantsHandler<W>
where
    W: Write + Unpin + Send + Sync,
{
    async fn recv_create_wants(
        &mut self,
        _api: &mut ApiHelper<W>,
        req_no: i32,
        _req: &rpc::Body,
    ) -> SolarResult<bool> {
        if self.peer_wants_req_no.is_none() {
            trace!(target: "ssb-blob", "recieved create wants");
            self.peer_wants_req_no = Some(req_no);
        } else {
            trace!(target: "ssb-blob", "peer create wants already received");
        }
        Ok(true)
    }

    async fn event_wants_broadcast(
        &mut self,
        api: &mut ApiHelper<W>,

        broadcast: &[(String,i64)]
    ) -> SolarResult<bool> {
        let mut wants : HashMap<String,i64> = HashMap::new();
        for (blob_id,distance) in broadcast {
            if !self.peer_wants.contains_key(blob_id) {
                wants.insert(blob_id.clone(),*distance);
            }
        }
        api.rpc().send_response(
            self.my_wants_req_no.unwrap(),
            rpc::RpcType::Source,
            rpc::BodyType::JSON,
            &serde_json::to_vec(&wants)?,
        )
        .await?;
        Ok(true)
    }

    async fn event_stoblob_added(
        &mut self,
        api: &mut ApiHelper<W>,
        blob_id: &str,
    ) -> SolarResult<bool> {
        if self.peer_wants.contains_key(blob_id) {
            let mut haves: HashMap<String, i64> = HashMap::new();
            haves.insert(blob_id.to_string(),1);
            api.rpc().send_response(
                self.peer_wants_req_no.unwrap(),
                rpc::RpcType::Source,
                rpc::BodyType::JSON,
                &serde_json::to_vec(&haves)?,
            ).await?;    
        }
        Ok(true)
    }

    async fn recv_wants(
        &mut self,
        api: &mut ApiHelper<W>,
        _req_no: i32,
        _xtype: rpc::BodyType,
        data: &[u8],
        ch_broker: &mut ChBrokerSend
    ) -> SolarResult<bool> {

        // requested wants by self.my_wants_req_no
        // anwsering haves by self.peer_wants_req_no

        let wants: HashMap<String, i64> = serde_json::from_slice(data)?;
        let mut haves: HashMap<String, u64> = HashMap::new();
        let mut broadcast : Vec<(String,i64)> = Vec::new();
        trace!(target: "ssb-blob", "wants:{:?}",wants);
        for (want, distance) in wants {
            if let Some(size) = BLOB_STORAGE.read().await.size_of(&want)? {
                haves.insert(want, size);
            } else {
                self.peer_wants.insert(want.clone(),Wants::Pending);
                broadcast.push((want,distance+1));
            }
        }
        trace!(target: "ssb-blob", "haves:{:?}",haves);
        trace!(target: "ssb-blob", "don't-haves:{:?}",broadcast);

        // respond with the blobs that I have
        api.rpc()
            .send_response(
                self.peer_wants_req_no.unwrap(),
                rpc::RpcType::Source,
                rpc::BodyType::JSON,
                &serde_json::to_vec(&haves)?,
            )
            .await?;

        // broadcast other peers with the blobs I don't have
        let broker_msg = BrokerEvent::new(Destination::Broadcast,RpcBlobsWantsEvent::BroadcastWants(broadcast));
        ch_broker
            .send(broker_msg)
            .await
            .unwrap();

        Ok(true)
    }

    async fn recv_haves(
        &mut self,
        api: &mut ApiHelper<W>,
        _req_no: i32,
        _xtype: rpc::BodyType,
        data: &[u8],
        _ch_broker: &mut ChBrokerSend
    ) -> SolarResult<bool> {

        let haves: HashMap<String, i64> = serde_json::from_slice(data)?;
        trace!(target: "ssb-blob", "haves:{:?}",haves);
        
        for (blob_id, _) in haves {
            if let Some(wants) = self.peer_wants.get_mut(&blob_id) {
                let req_no = api.blobs_get_req_send(&dto::BlobsGetIn::new(blob_id.clone())).await?;
                *wants = Wants::Requested(req_no);
            } 
        }

        Ok(true)
    }

    async fn recv_blobs_get(
        &mut self,
        _api: &mut ApiHelper<W>,
        req_no: i32,
        _xtype: rpc::BodyType,
        data: &[u8],
        _ch_broker: &mut ChBrokerSend
    ) -> SolarResult<bool> {

        let wants = self.peer_wants.iter_mut().find(|v| *v.1 == Wants::Requested(req_no)).unwrap();
        let current_blob_id = data.blob_hash_id();

        if &current_blob_id != wants.0 {
            warn!("Recieved blob hash is not the expected current={} expected={}",wants.0,current_blob_id);
        }

        BLOB_STORAGE.write().await.insert(&data).await?;
        *wants.1 = Wants::Available;

        Ok(true)
    }
}