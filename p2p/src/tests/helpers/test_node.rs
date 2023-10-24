// Copyright (c) 2021-2023 RBB S.r.l
// opensource@mintlayer.org
// SPDX-License-Identifier: MIT
// Licensed under the MIT License;
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://github.com/mintlayer/mintlayer-core/blob/master/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::{Arc, Mutex};

use chainstate::{
    make_chainstate, ChainstateConfig, ChainstateHandle, DefaultTransactionVerificationStrategy,
};
use p2p_test_utils::SHORT_TIMEOUT;
use p2p_types::{p2p_event::P2pEventHandler, socket_address::SocketAddress};
use storage_inmemory::InMemory;
use subsystem::ShutdownTrigger;
use tokio::{
    sync::{
        mpsc::{self},
        oneshot,
    },
    task::JoinHandle,
    time,
};

use crate::{
    config::P2pConfig,
    error::P2pError,
    net::{
        default_backend::{transport::TransportSocket, DefaultNetworkingService},
        ConnectivityService,
    },
    peer_manager::{
        peerdb::storage_impl::PeerDbStorageImpl, PeerManager, PeerManagerQueryInterface,
    },
    protocol::ProtocolVersion,
    sync::BlockSyncManager,
    testing_utils::peerdb_inmemory_store,
    types::ip_or_socket_address::IpOrSocketAddress,
    utils::oneshot_nofail,
    PeerManagerEvent,
};
use common::{chain::ChainConfig, time_getter::TimeGetter};
use utils::atomics::SeqCstAtomicBool;

use super::{PeerManagerNotification, PeerManagerObserver, TestDnsSeed, TestPeersInfo};

type PeerMgr<Transport> =
    PeerManager<DefaultNetworkingService<Transport>, PeerDbStorageImpl<InMemory>>;

pub struct TestNode<Transport>
where
    Transport: TransportSocket,
{
    peer_mgr_event_tx: mpsc::UnboundedSender<PeerManagerEvent>,
    local_address: SocketAddress,
    shutdown: Arc<SeqCstAtomicBool>,
    backend_shutdown_sender: oneshot::Sender<()>,
    _subscribers_sender: mpsc::UnboundedSender<P2pEventHandler>,
    backend_join_handle: JoinHandle<()>,
    peer_mgr_join_handle: JoinHandle<(PeerMgr<Transport>, P2pError)>,
    sync_mgr_join_handle: JoinHandle<P2pError>,
    shutdown_trigger: ShutdownTrigger,
    subsystem_mgr_join_handle: subsystem::ManagerJoinHandle,
    peer_mgr_notification_rx: mpsc::UnboundedReceiver<PeerManagerNotification>,
    chainstate: ChainstateHandle,
    dns_seed_addresses: Arc<Mutex<Vec<SocketAddress>>>,
}

// This is what's left of a test node after it has been stopped.
// TODO: it should be possible to use PeerManagerEvent::GenericQuery to examine peer manager's
// internals on the fly.
pub struct TestNodeRemnants<Transport>
where
    Transport: TransportSocket,
{
    pub peer_mgr: PeerMgr<Transport>,
    pub peer_mgr_error: P2pError,
    pub sync_mgr_error: P2pError,
}

impl<Transport> TestNode<Transport>
where
    Transport: TransportSocket,
{
    pub async fn start(
        time_getter: TimeGetter,
        chain_config: Arc<ChainConfig>,
        p2p_config: Arc<P2pConfig>,
        transport: Transport,
        bind_address: SocketAddress,
        protocol_version: ProtocolVersion,
    ) -> Self {
        let chainstate = make_chainstate(
            Arc::clone(&chain_config),
            ChainstateConfig::new(),
            chainstate_storage::inmemory::Store::new_empty().unwrap(),
            DefaultTransactionVerificationStrategy::new(),
            None,
            time_getter.clone(),
        )
        .unwrap();
        let (chainstate, mempool, shutdown_trigger, subsystem_mgr_join_handle) =
            p2p_test_utils::start_subsystems_with_chainstate(
                chainstate,
                Arc::clone(&chain_config),
                time_getter.clone(),
            );

        let (peer_mgr_event_tx, peer_mgr_event_rx) = mpsc::unbounded_channel();
        let shutdown = Arc::new(SeqCstAtomicBool::new(false));
        let (backend_shutdown_sender, backend_shutdown_receiver) = oneshot::channel();
        let (subscribers_sender, subscribers_receiver) = mpsc::unbounded_channel();

        let (conn_handle, messaging_handle, syncing_event_rx, backend_join_handle) =
            DefaultNetworkingService::<Transport>::start_with_version(
                transport,
                vec![bind_address],
                Arc::clone(&chain_config),
                Arc::clone(&p2p_config),
                time_getter.clone(),
                Arc::clone(&shutdown),
                backend_shutdown_receiver,
                subscribers_receiver,
                protocol_version,
            )
            .await
            .unwrap();

        let local_address = conn_handle.local_addresses()[0];

        let (peer_mgr_notification_tx, peer_mgr_notification_rx) = mpsc::unbounded_channel();
        let peer_mgr_observer = Box::new(PeerManagerObserver::new(peer_mgr_notification_tx));
        let dns_seed_addresses = Arc::new(Mutex::new(Vec::new()));

        let peer_mgr = PeerMgr::<Transport>::new_generic(
            Arc::clone(&chain_config),
            Arc::clone(&p2p_config),
            conn_handle,
            peer_mgr_event_rx,
            time_getter.clone(),
            peerdb_inmemory_store(),
            Some(peer_mgr_observer),
            Box::new(TestDnsSeed::new(dns_seed_addresses.clone())),
        )
        .unwrap();
        let peer_mgr_join_handle = logging::spawn_in_current_span(async move {
            let mut peer_mgr = peer_mgr;
            let err = match peer_mgr.run_without_consuming_self().await {
                Err(err) => err,
                Ok(never) => match never {},
            };

            (peer_mgr, err)
        });

        let sync_mgr = BlockSyncManager::<DefaultNetworkingService<Transport>>::new(
            Arc::clone(&chain_config),
            Arc::clone(&p2p_config),
            messaging_handle,
            syncing_event_rx,
            chainstate.clone(),
            mempool,
            peer_mgr_event_tx.clone(),
            time_getter.clone(),
        );
        let sync_mgr_join_handle = logging::spawn_in_current_span(async move {
            match sync_mgr.run().await {
                Err(err) => err,
                Ok(never) => match never {},
            }
        });

        TestNode {
            peer_mgr_event_tx,
            local_address,
            shutdown,
            backend_shutdown_sender,
            _subscribers_sender: subscribers_sender,
            backend_join_handle,
            peer_mgr_join_handle,
            sync_mgr_join_handle,
            shutdown_trigger,
            subsystem_mgr_join_handle,
            peer_mgr_notification_rx,
            chainstate,
            dns_seed_addresses,
        }
    }

    pub fn local_address(&self) -> &SocketAddress {
        &self.local_address
    }

    pub fn chainstate(&self) -> &ChainstateHandle {
        &self.chainstate
    }

    // Note: the returned receiver will become readable only after the handshake is finished.
    pub fn start_connecting(
        &self,
        address: SocketAddress,
    ) -> oneshot_nofail::Receiver<Result<(), P2pError>> {
        let (connect_result_tx, connect_result_rx) = oneshot_nofail::channel();
        self.peer_mgr_event_tx
            .send(PeerManagerEvent::Connect(
                IpOrSocketAddress::Socket(address.socket_addr()),
                connect_result_tx,
            ))
            .unwrap();

        connect_result_rx
    }

    pub async fn expect_no_banning(&mut self) {
        time::timeout(SHORT_TIMEOUT, async {
            loop {
                match self.peer_mgr_notification_rx.recv().await.unwrap() {
                    PeerManagerNotification::BanScoreAdjustment {
                        address: _,
                        new_score: _,
                    }
                    | PeerManagerNotification::Ban { address: _ } => {
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_err();
    }

    pub async fn wait_for_ban_score_adjustment(&mut self) -> (SocketAddress, u32) {
        loop {
            if let PeerManagerNotification::BanScoreAdjustment { address, new_score } =
                self.peer_mgr_notification_rx.recv().await.unwrap()
            {
                return (address, new_score);
            }
        }
    }

    pub async fn get_peers_info(&self) -> TestPeersInfo {
        let (tx, mut rx) = mpsc::unbounded_channel();

        self.peer_mgr_event_tx
            .send(PeerManagerEvent::GenericQuery(Box::new(
                move |mgr: &dyn PeerManagerQueryInterface| {
                    tx.send(TestPeersInfo::from_peer_mgr_peer_contexts(mgr.peers())).unwrap();
                },
            )))
            .unwrap();

        rx.recv().await.unwrap()
    }

    pub fn set_dns_seed_addresses(&self, addresses: Vec<SocketAddress>) {
        *self.dns_seed_addresses.lock().unwrap() = addresses;
    }

    pub async fn join(self) -> TestNodeRemnants<Transport> {
        self.shutdown.store(true);
        let _ = self.backend_shutdown_sender.send(());
        let (peer_mgr, peer_mgr_error) = self.peer_mgr_join_handle.await.unwrap();
        let sync_mgr_error = self.sync_mgr_join_handle.await.unwrap();
        self.backend_join_handle.await.unwrap();
        self.shutdown_trigger.initiate();
        self.subsystem_mgr_join_handle.join().await;

        TestNodeRemnants {
            peer_mgr,
            peer_mgr_error,
            sync_mgr_error,
        }
    }
}