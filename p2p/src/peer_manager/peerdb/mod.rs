// Copyright (c) 2022 RBB S.r.l
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

//! Peer database
//!
//! The peer database stores:
//! - connected peers
//! - available (discovered) addresses
//! - banned addresses
//!
//! Connected peers are those peers that the [`crate::peer_manager::PeerManager`] has an active
//! connection with. Available addresses are discovered through various peer discovery mechanisms and they are
//! used by [`crate::peer_manager::PeerManager::heartbeat()`] to establish new outbound connections
//! if the actual number of active connection is less than the desired number of connections.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crypto::random::{make_pseudo_rng, SliceRandom};
use logging::log;

use crate::{
    config,
    error::{ConversionError, P2pError},
    interface::types::ConnectedPeer,
    net::{
        types::{self, Role},
        AsBannableAddress, NetworkingService,
    },
};

#[derive(Debug)]
pub struct PeerContext<T: NetworkingService> {
    /// Peer information
    pub info: types::PeerInfo<T::PeerId>,

    /// Peer's address
    pub address: T::Address,

    /// Peer's role (inbound or outbound)
    pub role: Role,

    /// Peer score
    pub score: u32,
}

impl<T: NetworkingService> From<&PeerContext<T>> for ConnectedPeer {
    fn from(context: &PeerContext<T>) -> Self {
        ConnectedPeer {
            peer_id: context.info.peer_id.to_string(),
            address: context.address.to_string(),
            inbound: context.role == Role::Inbound,
            ban_score: context.score,
        }
    }
}

// TODO: Store available addresses in a binary heap (sorting by their availability).
// TODO: Find a way to persist this data in some database for when the node is restarted
// (banned, available, and at-least-once used should be restored)
pub struct PeerDb<T: NetworkingService> {
    /// P2P configuration
    p2p_config: Arc<config::P2pConfig>,

    /// Set of active peers
    peers: BTreeMap<T::PeerId, PeerContext<T>>,

    /// Set of currently connected addresses
    connected_addresses: BTreeSet<T::Address>,

    /// Set of all known addresses
    known_addresses: BTreeSet<T::Address>,

    /// Banned addresses along with the duration of the ban.
    ///
    /// The duration represents the `UNIX_EPOCH + duration` time point, so the ban should end
    /// when `current_time > ban_duration`.
    banned: BTreeMap<T::BannableAddress, Duration>,
}

impl<T: NetworkingService> PeerDb<T> {
    pub fn new(p2p_config: Arc<config::P2pConfig>) -> crate::Result<Self> {
        let added_nodes = p2p_config
            .added_nodes
            .iter()
            .map(|addr| {
                addr.parse::<T::Address>().map_err(|_err| {
                    P2pError::ConversionError(ConversionError::InvalidAddress(addr.clone()))
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(Self {
            peers: Default::default(),
            connected_addresses: Default::default(),
            // TODO: We need to handle added nodes differently from ordinary nodes.
            // There are peers that we want to persistently have, and others that we want to just give a "shot" at connecting at.
            known_addresses: added_nodes,
            banned: Default::default(),
            p2p_config,
        })
    }

    /// Get the number of idle (available) addresses
    pub fn available_addresses_count(&self) -> usize {
        self.known_addresses.len()
    }

    /// Get the number of active peers
    pub fn active_peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Returns short info about all connected peers
    pub fn get_connected_peers(&self) -> Vec<ConnectedPeer> {
        self.peers.values().map(Into::into).collect()
    }

    /// Checks if the given address is already connected.
    pub fn is_address_connected(&self, address: &T::Address) -> bool {
        self.connected_addresses.contains(address)
    }

    /// Selects requested count of peer addresses from the DB randomly.
    ///
    /// Result could be shared with remote peers over network.
    pub fn random_known_addresses(&self, count: usize) -> Vec<T::Address> {
        // TODO: Use something more efficient (without iterating over the all addresses first)
        let all_addresses = self.known_addresses.iter().cloned().collect::<Vec<_>>();
        all_addresses
            .choose_multiple(&mut make_pseudo_rng(), count)
            .cloned()
            .collect::<Vec<_>>()
    }

    /// Selects requested count of connected peer ids randomly.
    ///
    /// It can be used to distribute data in the gossip protocol
    /// (for example, to relay announced addresses to a small group of peers).
    pub fn random_peer_ids(&self, count: usize) -> Vec<T::PeerId> {
        // There are normally not many connected peers, so iterating over the whole list should be OK
        let all_peer_ids = self.peers.keys().cloned().collect::<Vec<_>>();
        all_peer_ids
            .choose_multiple(&mut make_pseudo_rng(), count)
            .cloned()
            .collect::<Vec<_>>()
    }

    /// Checks if the given address is banned.
    pub fn is_address_banned(&mut self, address: &T::BannableAddress) -> bool {
        if let Some(banned_till) = self.banned.get(address) {
            // Check if the ban has expired.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                // This can fail only if `SystemTime::now()` returns the time before `UNIX_EPOCH`.
                .expect("Invalid system time");
            if now > *banned_till {
                self.banned.remove(address);
            } else {
                return true;
            }
        }

        false
    }

    /// Checks if the peer is active
    pub fn is_active_peer(&self, peer_id: &T::PeerId) -> bool {
        self.peers.get(peer_id).is_some()
    }

    /// Get socket address of the next best peer (TODO: in terms of peer score).
    // TODO: Rewrite this.
    pub fn get_best_peer_addr(&mut self) -> Option<T::Address> {
        self.random_known_addresses(1).into_iter().next()
    }

    /// Add new peer addresses
    pub fn peer_discovered(&mut self, address: &T::Address) {
        self.known_addresses.insert(address.clone());
    }

    /// Report outbound connection failure
    ///
    /// When [`crate::peer_manager::PeerManager::heartbeat()`] has initiated an outbound connection
    /// and the connection is refused, it's reported back to the `PeerDb` so it marks the address as unreachable.
    pub fn report_outbound_failure(&mut self, _address: T::Address) {
        // TODO: implement
    }

    /// Mark peer as connected
    ///
    /// After `PeerManager` has established either an inbound or an outbound connection,
    /// it informs the `PeerDb` about it.
    pub fn peer_connected(
        &mut self,
        address: T::Address,
        role: Role,
        info: types::PeerInfo<T::PeerId>,
    ) {
        log::info!(
            "peer connected, peer_id: {}, address: {address:?}, {:?}",
            info.peer_id,
            role
        );

        let old_value = self.peers.insert(
            info.peer_id,
            PeerContext {
                info,
                address: address.clone(),
                role,
                score: 0,
            },
        );
        assert!(old_value.is_none());

        let old_value = self.connected_addresses.insert(address);
        assert!(old_value);
    }

    /// Handle peer disconnection event
    ///
    /// Close the connection to an active peer.
    pub fn peer_disconnected(&mut self, peer_id: &T::PeerId) {
        let removed = self.peers.remove(peer_id);
        let peer = removed.expect("peer must be known");

        let removed = self.connected_addresses.remove(&peer.address);
        assert!(removed);

        log::info!(
            "peer disconnected, peer_id: {}, address: {:?}",
            peer.info.peer_id,
            peer.address
        );
    }

    /// Changes the peer state to `Peer::Banned` and bans it for 24 hours.
    fn ban_peer(&mut self, peer_id: &T::PeerId) {
        if let Some(peer) = self.peers.remove(peer_id) {
            let bannable_address = peer.address.as_bannable();
            let ban_till = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                // This can fail only if `SystemTime::now()` returns the time before `UNIX_EPOCH`.
                .expect("Invalid system time")
                + *self.p2p_config.ban_duration;
            self.banned.insert(bannable_address, ban_till);
        } else {
            log::error!("Failed to get address for peer {}", peer_id);
        }
    }

    /// Adjust peer score
    ///
    /// If the peer is known, update its existing peer score and report
    /// if it should be disconnected when score reached the threshold.
    /// Unknown peers are reported as to be disconnected.
    ///
    /// If peer is banned, it is removed from the connected peers
    /// and its address is marked as banned.
    pub fn adjust_peer_score(&mut self, peer_id: &T::PeerId, score: u32) -> bool {
        let peer = match self.peers.get_mut(peer_id) {
            Some(peer) => peer,
            None => return true,
        };

        peer.score = peer.score.saturating_add(score);

        if peer.score >= *self.p2p_config.ban_threshold {
            self.ban_peer(peer_id);
            return true;
        }

        false
    }

    pub fn peer_address(&self, id: &T::PeerId) -> Option<&T::Address> {
        self.peers.get(id).map(|c| &c.address)
    }
}