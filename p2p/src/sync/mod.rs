// Copyright (c) 2022 RBB S.r.l
// opensource@mintlayer.org
// SPDX-License-Identifier: MIT
// Licensed under the MIT License;
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://spdx.org/licenses/MIT
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Author(s): A. Altonen
#![allow(unused)]

use crate::{
    error::{self, P2pError},
    event,
    net::{self, NetworkService, PubSubService},
};
use common::chain::ChainConfig;
use futures::FutureExt;
use logging::log;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc;

/// State of the peer
enum PeerState {
    /// No activity with the peer
    Idle,
}

struct PeerSyncState<T>
where
    T: NetworkService,
{
    /// Unique peer ID
    peer_id: T::PeerId,

    // State of the peer
    state: PeerState,

    /// TX channel for sending syncing messages to remote peer
    tx: mpsc::Sender<event::PeerEvent<T>>,
}

/// Sync manager is responsible for syncing the local blockchain to the chain with most trust
/// and keeping up with updates to different branches of the blockchain.
///
/// It keeps track of the state of each individual peer and holds an intermediary block index
/// which represents the local block index of every peer it's connected to.
///
/// Currently its only mode of operation is greedy so it will download all changes from every
/// peer it's connected to and actively keep track of the peer's state.
pub struct SyncManager<T>
where
    T: NetworkService,
{
    /// Chain config
    config: Arc<ChainConfig>,

    /// Handle for sending/receiving connectivity events
    handle: T::PubSubHandle,

    /// RX channel for receiving syncing-related control events
    rx_sync: mpsc::Receiver<event::SyncControlEvent<T>>,

    /// RX channel for receiving syncing events from peers
    rx_peer: mpsc::Receiver<event::PeerSyncEvent<T>>,

    /// Hashmap of connected peers
    peers: HashMap<T::PeerId, PeerSyncState<T>>,
}

impl<T> SyncManager<T>
where
    T: NetworkService,
    T::PubSubHandle: PubSubService<T>,
{
    pub fn new(
        config: Arc<ChainConfig>,
        handle: T::PubSubHandle,
        rx_sync: mpsc::Receiver<event::SyncControlEvent<T>>,
        rx_peer: mpsc::Receiver<event::PeerSyncEvent<T>>,
    ) -> Self {
        Self {
            config,
            handle,
            rx_sync,
            rx_peer,
            peers: Default::default(),
        }
    }

    /// Handle pubsub event
    fn on_pubsub_event(&mut self, event: net::PubSubEvent<T>) -> error::Result<()> {
        let net::PubSubEvent::MessageReceived {
            peer_id: _,
            topic,
            message,
            ..
        } = event;

        match topic {
            net::PubSubTopic::Transactions => {
                log::debug!("received new transaction: {:#?}", message);
            }
            net::PubSubTopic::Blocks => {
                log::debug!("received new block: {:#?}", message);
            }
        }

        Ok(())
    }

    /// Handle control-related sync event from P2P/SwarmManager
    async fn on_sync_event(&mut self, event: event::SyncControlEvent<T>) -> error::Result<()> {
        match event {
            event::SyncControlEvent::Connected { peer_id, tx } => {
                log::debug!("create new entry for peer {:?}", peer_id);

                if let std::collections::hash_map::Entry::Vacant(e) = self.peers.entry(peer_id) {
                    e.insert(PeerSyncState {
                        peer_id,
                        state: PeerState::Idle,
                        tx,
                    });
                } else {
                    log::error!("peer {:?} already known by sync manager", peer_id);
                }
            }
            event::SyncControlEvent::Disconnected { peer_id } => {
                self.peers
                    .remove(&peer_id)
                    .ok_or_else(|| P2pError::Unknown("Peer does not exist".to_string()))
                    .map(|_| log::debug!("remove peer {:?}", peer_id))
                    .map_err(|_| log::error!("peer {:?} not known by sync manager", peer_id));
            }
        }

        Ok(())
    }

    /// Handle syncing-related event received from a remote peer
    async fn on_peer_event(&mut self, event: event::PeerSyncEvent<T>) -> error::Result<()> {
        match event {
            event::PeerSyncEvent::Dummy { peer_id } => {
                dbg!(peer_id);
            }
        }

        Ok(())
    }

    /// Run SyncManager event loop
    pub async fn run(&mut self) -> error::Result<()> {
        log::info!("starting sync manager event loop");

        loop {
            tokio::select! {
                res = self.handle.poll_next() => {
                    self.on_pubsub_event(res?)?;
                }
                res = self.rx_sync.recv().fuse() => {
                    self.on_sync_event(res.ok_or(P2pError::ChannelClosed)?).await?;
                }
                res = self.rx_peer.recv().fuse() => {
                    self.on_peer_event(res.ok_or(P2pError::ChannelClosed)?).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{mock::MockService, PubSubService};
    use common::chain::config;
    use std::net::SocketAddr;

    async fn make_sync_manager<T>(
        addr: T::Address,
    ) -> (
        SyncManager<T>,
        mpsc::Sender<event::SyncControlEvent<T>>,
        mpsc::Sender<event::PeerSyncEvent<T>>,
    )
    where
        T: NetworkService,
        T::PubSubHandle: PubSubService<T>,
    {
        let config = Arc::new(config::create_mainnet());
        let (_, flood) =
            T::start(addr, &[], &[], std::time::Duration::from_secs(10)).await.unwrap();
        let (tx_sync, rx_sync) = tokio::sync::mpsc::channel(16);
        let (tx_peer, rx_peer) = tokio::sync::mpsc::channel(16);

        (
            SyncManager::<T>::new(Arc::clone(&config), flood, rx_sync, rx_peer),
            tx_sync,
            tx_peer,
        )
    }

    // handle peer connection event
    #[tokio::test]
    async fn test_peer_connected() {
        let addr: SocketAddr = test_utils::make_address("[::1]:");
        let (mut mgr, mut tx_sync, mut tx_peer) = make_sync_manager::<MockService>(addr).await;

        // send Connected event to SyncManager
        let (tx, rx) = mpsc::channel(1);
        let peer_id: SocketAddr = test_utils::make_address("[::1]:");

        assert_eq!(
            mgr.on_sync_event(event::SyncControlEvent::Connected { peer_id, tx }).await,
            Ok(())
        );
        assert_eq!(mgr.peers.len(), 1);
    }

    // handle peer disconnection event
    #[tokio::test]
    async fn test_peer_disconnected() {
        let addr: SocketAddr = test_utils::make_address("[::1]:");
        let (mut mgr, mut tx_sync, mut tx_peer) = make_sync_manager::<MockService>(addr).await;

        // send Connected event to SyncManager
        let (tx, rx) = mpsc::channel(1);
        let peer_id: SocketAddr = test_utils::make_address("[::1]:");

        assert_eq!(
            mgr.on_sync_event(event::SyncControlEvent::Connected { peer_id, tx }).await,
            Ok(())
        );
        assert_eq!(mgr.peers.len(), 1);

        // no peer with this id exist, nothing happens
        assert_eq!(
            mgr.on_sync_event(event::SyncControlEvent::Disconnected { peer_id: addr }).await,
            Ok(())
        );
        assert_eq!(mgr.peers.len(), 1);

        assert_eq!(
            mgr.on_sync_event(event::SyncControlEvent::Disconnected { peer_id }).await,
            Ok(())
        );
        assert!(mgr.peers.is_empty());
    }
}
