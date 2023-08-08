// Copyright (c) 2021-2022 RBB S.r.l
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

pub mod backend;
pub mod peer;
pub mod transport;
pub mod types;

use std::{marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use common::time_getter::TimeGetter;
use p2p_types::socket_address::SocketAddress;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use logging::log;
use utils::atomics::SeqCstAtomicBool;

use crate::{
    error::P2pError,
    message::{PeerManagerMessage, SyncMessage},
    net::{
        default_backend::transport::{TransportListener, TransportSocket},
        types::{ConnectivityEvent, SyncingEvent},
        ConnectivityService, MessagingService, NetworkingService, SyncingEventReceiver,
    },
    types::peer_id::PeerId,
    P2pConfig, P2pEventHandler,
};

#[derive(Debug)]
pub struct DefaultNetworkingService<T: TransportSocket>(PhantomData<T>);

#[derive(Debug)]
pub struct ConnectivityHandle<S: NetworkingService> {
    /// The local addresses of a network service provider.
    local_addresses: Vec<SocketAddress>,

    /// TX channel for sending commands to default_backend backend
    cmd_tx: mpsc::UnboundedSender<types::Command>,

    /// RX channel for receiving connectivity events from default_backend backend
    conn_rx: mpsc::UnboundedReceiver<ConnectivityEvent>,

    _marker: PhantomData<fn() -> S>,
}

impl<S: NetworkingService> ConnectivityHandle<S> {
    pub fn new(
        local_addresses: Vec<SocketAddress>,
        cmd_tx: mpsc::UnboundedSender<types::Command>,
        conn_rx: mpsc::UnboundedReceiver<ConnectivityEvent>,
    ) -> Self {
        Self {
            local_addresses,
            cmd_tx,
            conn_rx,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug)]
pub struct MessagingHandle {
    command_sender: mpsc::UnboundedSender<types::Command>,
}

impl MessagingHandle {
    pub fn new(command_sender: mpsc::UnboundedSender<types::Command>) -> Self {
        Self { command_sender }
    }
}

impl Clone for MessagingHandle {
    fn clone(&self) -> Self {
        Self {
            command_sender: self.command_sender.clone(),
        }
    }
}

#[derive(Debug)]
pub struct SyncingReceiver {
    sync_rx: mpsc::UnboundedReceiver<SyncingEvent>,
}

#[async_trait]
impl<T: TransportSocket> NetworkingService for DefaultNetworkingService<T> {
    type Transport = T;
    type ConnectivityHandle = ConnectivityHandle<Self>;
    type MessagingHandle = MessagingHandle;
    type SyncingEventReceiver = SyncingReceiver;

    async fn start(
        transport: Self::Transport,
        bind_addresses: Vec<SocketAddress>,
        chain_config: Arc<common::chain::ChainConfig>,
        p2p_config: Arc<P2pConfig>,
        time_getter: TimeGetter,
        shutdown: Arc<SeqCstAtomicBool>,
        shutdown_receiver: oneshot::Receiver<()>,
        subscribers_receiver: mpsc::UnboundedReceiver<P2pEventHandler>,
    ) -> crate::Result<(
        Self::ConnectivityHandle,
        Self::MessagingHandle,
        Self::SyncingEventReceiver,
        JoinHandle<()>,
    )> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        let (sync_tx, sync_rx) = mpsc::unbounded_channel();
        let socket = transport.bind(bind_addresses).await?;
        let local_addresses = socket.local_addresses().expect("to have bind address available");

        let backend = backend::Backend::<T>::new(
            transport,
            socket,
            chain_config,
            Arc::clone(&p2p_config),
            time_getter.clone(),
            cmd_rx,
            conn_tx,
            sync_tx,
            Arc::clone(&shutdown),
            shutdown_receiver,
            subscribers_receiver,
        );
        let backend_task = tokio::spawn(async move {
            match backend.run().await {
                Ok(never) => match never {},
                Err(P2pError::ChannelClosed) if shutdown.load() => {
                    log::info!("Backend is shut down");
                }
                Err(e) => {
                    shutdown.store(true);
                    log::error!("Failed to run backend: {e}");
                }
            }
        });

        Ok((
            ConnectivityHandle::new(local_addresses, cmd_tx.clone(), conn_rx),
            MessagingHandle::new(cmd_tx),
            Self::SyncingEventReceiver { sync_rx },
            backend_task,
        ))
    }
}

#[async_trait]
impl<S> ConnectivityService<S> for ConnectivityHandle<S>
where
    S: NetworkingService + Send,
{
    fn connect(&mut self, address: SocketAddress) -> crate::Result<()> {
        log::debug!(
            "try to establish outbound connection, address {:?}",
            address
        );

        Ok(self.cmd_tx.send(types::Command::Connect { address })?)
    }

    fn accept(&mut self, peer_id: PeerId) -> crate::Result<()> {
        log::debug!("accept new peer, peer_id: {peer_id}");

        Ok(self.cmd_tx.send(types::Command::Accept { peer_id })?)
    }

    fn disconnect(&mut self, peer_id: PeerId) -> crate::Result<()> {
        log::debug!("close connection with remote, peer_id: {peer_id}");

        Ok(self.cmd_tx.send(types::Command::Disconnect { peer_id })?)
    }

    fn send_message(&mut self, peer: PeerId, message: PeerManagerMessage) -> crate::Result<()> {
        Ok(self.cmd_tx.send(types::Command::SendMessage {
            peer,
            message: message.into(),
        })?)
    }

    fn local_addresses(&self) -> &[SocketAddress] {
        &self.local_addresses
    }

    async fn poll_next(&mut self) -> crate::Result<ConnectivityEvent> {
        self.conn_rx.recv().await.ok_or(P2pError::ChannelClosed)
    }
}

impl MessagingService for MessagingHandle {
    fn send_message(&mut self, peer: PeerId, message: SyncMessage) -> crate::Result<()> {
        Ok(self.command_sender.send(types::Command::SendMessage {
            peer,
            message: message.into(),
        })?)
    }
}

#[async_trait]
impl SyncingEventReceiver for SyncingReceiver {
    async fn poll_next(&mut self) -> crate::Result<SyncingEvent> {
        self.sync_rx.recv().await.ok_or(P2pError::ChannelClosed)
    }
}

#[cfg(test)]
mod tests;
