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

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use chainstate::BlockSource;
use common::{
    chain::{Block, ChainConfig},
    primitives::{user_agent::mintlayer_core_user_agent, Idable},
};
use logging::log;
use p2p_test_utils::P2pBasicTestTimeGetter;
use p2p_types::socket_address::SocketAddress;
use test_utils::random::Seed;

use crate::{
    config::P2pConfig,
    net::types::PeerRole,
    peer_manager::{
        self, address_groups::AddressGroup, ConnectionCountLimits, PEER_MGR_DNS_RELOAD_INTERVAL,
        PEER_MGR_HEARTBEAT_INTERVAL_MAX,
    },
    sync::test_helpers::make_new_block,
    testing_utils::{TestTransportChannel, TestTransportMaker, TEST_PROTOCOL_VERSION},
    tests::helpers::{timeout, TestNode, TestNodeGroup},
};

// In these tests we want to create nodes in different "address groups" to ensure that
// the maximum number of connections can be established (peer manager normally won't allow more
// than 1 outbound connection per address group). To do so we must use ip addresses with distinct
// higher bytes; only the channel-based transport allows to use arbitrary ip addresses, so we
// have to use it.
type Transport = <TestTransportChannel as TestTransportMaker>::Transport;

// Test scenario:
// 1) Create a set of nodes; the number of nodes is equal to the maximum number of outbound
// connections that a single node can establish plus 1.
// The nodes start with a fresh block, so they are not in IBD.
// 2) Announce nodes' addresses via the dns seed; the nodes should connect to each other.
// 3) Wait for one hour; the initial block is now stale, but the nodes are still connected
// to each other.
// 4) Start a new node that has a fresh block; announce its address via the dns seed;
// the old nodes should find the new one; some of them should establish an outbound connection
// to it; eventually, all old nodes should receive the fresh block.
#[tracing::instrument(skip(seed))]
#[rstest::rstest]
#[trace]
#[case(Seed::from_entropy())]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_discovery_on_stale_tip(#[case] seed: Seed) {
    timeout(peer_discovery_on_stale_tip_impl(seed)).await;
}

async fn peer_discovery_on_stale_tip_impl(seed: Seed) {
    let mut rng = test_utils::random::make_seedable_rng(seed);
    let time_getter = P2pBasicTestTimeGetter::new();
    let chain_config = Arc::new(common::chain::config::create_unit_test_config());
    let p2p_config = Arc::new(make_p2p_config());

    let nodes_count = p2p_config.connection_count_limits.outbound_full_and_block_relay_count() + 1;
    let mut nodes = Vec::with_capacity(nodes_count);

    let initial_block = make_new_block(
        &chain_config,
        None,
        &time_getter.get_time_getter(),
        &mut rng,
    );

    for i in 0..nodes_count {
        nodes.push(
            start_node_with_a_block(
                &time_getter,
                &chain_config,
                &p2p_config,
                i + 1,
                initial_block.clone(),
            )
            .await,
        );
    }

    let node_group = TestNodeGroup::new(nodes, time_getter.clone(), p2p_config.clone());
    let node_addresses = node_group.get_adresses();

    let address_groups: BTreeSet<_> = node_addresses
        .iter()
        .map(|addr| AddressGroup::from_peer_address(&addr.as_peer_address()))
        .collect();
    // Sanity check - all addresses belong to separate address groups
    assert_eq!(address_groups.len(), nodes_count);

    node_group.set_dns_seed_addresses(&node_addresses);

    time_getter.advance_time(PEER_MGR_DNS_RELOAD_INTERVAL);

    // Wait until the maximum number of outbound connections is established.
    wait_for_max_outbound_connections(&node_group).await;

    // Advance the time by 1 hour
    log::debug!("Advancing time by 1 hour");
    time_getter.advance_time(Duration::from_secs(60 * 60));

    // All the connections must still be in place
    assert_max_outbound_connections(&node_group).await;

    // Start a new node that would produce a block.
    let new_node_idx = node_group.nodes().len() + 1;
    let new_node = start_node_with_a_block(
        &time_getter,
        &chain_config,
        &p2p_config,
        new_node_idx,
        initial_block.clone(),
    )
    .await;
    let new_node_addr = *new_node.local_address();

    let new_block = make_new_block(
        &chain_config,
        Some(&initial_block),
        &time_getter.get_time_getter(),
        &mut rng,
    );
    let new_block_id = new_block.get_id();

    new_node
        .chainstate()
        .call_mut(move |cs| {
            cs.process_block(new_block, BlockSource::Local).unwrap();
        })
        .await
        .unwrap();

    // Announce the node through the dns seed.
    let mut node_addresses = node_addresses;
    node_addresses.push(new_node_addr);
    node_group.set_dns_seed_addresses(&node_addresses);

    // Wait for some connections to the new node to be established.
    wait_for_connections_to(&node_group, new_node_addr, nodes_count / 2).await;

    // Wait for the new block to be propagated to all the nodes.
    node_group
        .wait_for_block_propagation_advance_time(
            nodes_count,
            new_block_id,
            PEER_MGR_HEARTBEAT_INTERVAL_MAX,
        )
        .await;

    log::debug!("shutting down");

    node_group.join().await;
    new_node.join().await;
}

// Same as peer_discovery_on_stale_tip, but here the "old" nodes start without a fresh block,
// i.e. they are in IBD initially.
#[tracing::instrument(skip(seed))]
#[rstest::rstest]
#[trace]
#[case(Seed::from_entropy())]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_discovery_on_stale_tip_ibd(#[case] seed: Seed) {
    timeout(peer_discovery_on_stale_tip_ibd_impl(seed)).await;
}

async fn peer_discovery_on_stale_tip_ibd_impl(seed: Seed) {
    let mut rng = test_utils::random::make_seedable_rng(seed);
    let time_getter = P2pBasicTestTimeGetter::new();
    let chain_config = Arc::new(common::chain::config::create_unit_test_config());
    let p2p_config = Arc::new(make_p2p_config());

    let nodes_count = p2p_config.connection_count_limits.outbound_full_and_block_relay_count() + 1;
    let mut nodes = Vec::with_capacity(nodes_count);

    for i in 0..nodes_count {
        nodes.push(start_node(&time_getter, &chain_config, &p2p_config, i + 1).await);
    }

    let node_group = TestNodeGroup::new(nodes, time_getter.clone(), p2p_config.clone());
    let node_addresses = node_group.get_adresses();

    let address_groups: BTreeSet<_> = node_addresses
        .iter()
        .map(|addr| AddressGroup::from_peer_address(&addr.as_peer_address()))
        .collect();
    // Sanity check - all addresses belong to separate address groups
    assert_eq!(address_groups.len(), nodes_count);

    node_group.set_dns_seed_addresses(&node_addresses);

    time_getter.advance_time(PEER_MGR_DNS_RELOAD_INTERVAL);

    // Wait until the maximum number of outbound connections is established.
    wait_for_max_outbound_connections(&node_group).await;

    // Advance the time by 1 hour
    log::debug!("Advancing time by 1 hour");
    time_getter.advance_time(Duration::from_secs(60 * 60));

    // All the connections must still be in place
    assert_max_outbound_connections(&node_group).await;

    // Start a new node that would produce a block.
    let new_node_idx = node_group.nodes().len() + 1;
    let new_node = start_node(&time_getter, &chain_config, &p2p_config, new_node_idx).await;
    let new_node_addr = *new_node.local_address();

    let new_block = make_new_block(
        &chain_config,
        None,
        &time_getter.get_time_getter(),
        &mut rng,
    );
    let new_block_id = new_block.get_id();

    new_node
        .chainstate()
        .call_mut(move |cs| {
            cs.process_block(new_block, BlockSource::Local).unwrap();
        })
        .await
        .unwrap();

    // Announce the node through the dns seed.
    let mut node_addresses = node_addresses;
    node_addresses.push(new_node_addr);
    node_group.set_dns_seed_addresses(&node_addresses);

    // Wait for some connections to the new node to be established.
    wait_for_connections_to(&node_group, new_node_addr, nodes_count / 2).await;

    // Wait for the new block to be propagated to all the nodes.
    node_group
        .wait_for_block_propagation_advance_time(
            nodes_count,
            new_block_id,
            PEER_MGR_HEARTBEAT_INTERVAL_MAX,
        )
        .await;

    log::debug!("shutting down");

    node_group.join().await;
    new_node.join().await;
}

fn make_transport_with_local_addr_in_group(
    group_idx: u32,
) -> <TestTransportChannel as TestTransportMaker>::Transport {
    let group_bits = peer_manager::address_groups::IPV4_GROUP_BYTES * 8;

    TestTransportChannel::make_transport_with_local_addr_in_group(
        // Make sure that the most significant byte of the address is non-zero
        // (all 0.x.x.x addresses get into AddressGroup::Private, but we want all
        // addresses to be in different address groups).
        group_idx + (1 << (group_bits - 1)),
        group_bits as u32,
    )
}

fn make_p2p_config() -> P2pConfig {
    let two_hours = Duration::from_secs(60 * 60 * 2);

    P2pConfig {
        // Note: these tests move mocked time forward by 1 hour once and by smaller intervals
        // multiple times; because of this, nodes may see each other as dead or as having invalid
        // clocks and disconnect each other. To avoid this, we specify artificially large timeouts
        // and clock diff.
        ping_timeout: two_hours.into(),
        max_clock_diff: two_hours.into(),
        sync_stalling_timeout: two_hours.into(),

        connection_count_limits: ConnectionCountLimits {
            // The sum of these values plus one is the number of nodes that the tests will create.
            // We reduce the numbers to make the tests less "heavy".
            outbound_full_relay_count: 2.into(),
            outbound_block_relay_count: 1.into(),

            // These values will only matter if max_inbound_connections is low enough.
            // Also, we don't really want to make inbound peer eviction more aggressive,
            // because it may make the tests more fragile, so we use the defaults.
            preserved_inbound_count_address_group: Default::default(),
            preserved_inbound_count_ping: Default::default(),
            preserved_inbound_count_new_blocks: Default::default(),
            preserved_inbound_count_new_transactions: Default::default(),
        },
        bind_addresses: Default::default(),
        socks5_proxy: Default::default(),
        disable_noise: Default::default(),
        boot_nodes: Default::default(),
        reserved_nodes: Default::default(),
        max_inbound_connections: Default::default(),
        ban_threshold: Default::default(),
        ban_duration: Default::default(),
        outbound_connection_timeout: Default::default(),
        ping_check_period: Default::default(),
        node_type: Default::default(),
        allow_discover_private_ips: Default::default(),
        msg_header_count_limit: Default::default(),
        msg_max_locator_count: Default::default(),
        max_request_blocks_count: Default::default(),
        user_agent: mintlayer_core_user_agent(),
        max_message_size: Default::default(),
        max_peer_tx_announcements: Default::default(),
        max_singular_unconnected_headers: Default::default(),
        enable_block_relay_peers: Default::default(),
    }
}

async fn start_node(
    time_getter: &P2pBasicTestTimeGetter,
    chain_config: &Arc<ChainConfig>,
    p2p_config: &Arc<P2pConfig>,
    node_index: usize,
) -> TestNode<Transport> {
    TestNode::<Transport>::start(
        time_getter.get_time_getter(),
        Arc::clone(chain_config),
        Arc::clone(p2p_config),
        make_transport_with_local_addr_in_group(node_index as u32),
        TestTransportChannel::make_address(),
        TEST_PROTOCOL_VERSION.into(),
    )
    .await
}

async fn start_node_with_a_block(
    time_getter: &P2pBasicTestTimeGetter,
    chain_config: &Arc<ChainConfig>,
    p2p_config: &Arc<P2pConfig>,
    node_index: usize,
    block: Block,
) -> TestNode<Transport> {
    let node = start_node(time_getter, chain_config, p2p_config, node_index).await;
    node.chainstate()
        .call_mut(move |cs| {
            cs.process_block(block, BlockSource::Local).unwrap();
        })
        .await
        .unwrap();
    node
}

async fn wait_for_max_outbound_connections(node_group: &TestNodeGroup<Transport>) {
    for node in node_group.nodes() {
        let mut outbound_full_relay_peers_count = 0;
        let mut outbound_block_relay_peers_count = 0;
        while outbound_full_relay_peers_count < *node_group.p2p_config().connection_count_limits.outbound_full_relay_count
            // Note: "-1" is used because one of the block relay connections is not permanent,
            // it's dropped and re-established regularly.
            || outbound_block_relay_peers_count < *node_group.p2p_config().connection_count_limits.outbound_block_relay_count - 1
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let peers_info = node.get_peers_info().await;
            outbound_full_relay_peers_count =
                peers_info.count_peers_by_role(PeerRole::OutboundFullRelay);
            outbound_block_relay_peers_count =
                peers_info.count_peers_by_role(PeerRole::OutboundBlockRelay);

            node_group.time_getter().advance_time(PEER_MGR_HEARTBEAT_INTERVAL_MAX);
        }
    }
}

async fn assert_max_outbound_connections(node_group: &TestNodeGroup<Transport>) {
    for node in node_group.nodes() {
        let peers_info = node.get_peers_info().await;
        let outbound_full_relay_peers_count =
            peers_info.count_peers_by_role(PeerRole::OutboundFullRelay);
        let outbound_block_relay_peers_count =
            peers_info.count_peers_by_role(PeerRole::OutboundBlockRelay);

        assert!(
            outbound_full_relay_peers_count
                >= *node_group.p2p_config().connection_count_limits.outbound_full_relay_count
        );
        assert!(
            outbound_block_relay_peers_count
                >= *node_group.p2p_config().connection_count_limits.outbound_block_relay_count - 1
        );
    }
}

async fn wait_for_connections_to(
    node_group: &TestNodeGroup<Transport>,
    address: SocketAddress,
    nodes_count: usize,
) {
    let mut connected_nodes_count = 0;
    loop {
        for node in node_group.nodes() {
            let peers_info = node.get_peers_info().await;
            if peers_info.info.contains_key(&address) {
                connected_nodes_count += 1;
            }
        }

        if connected_nodes_count >= nodes_count {
            break;
        }

        node_group.time_getter().advance_time(PEER_MGR_HEARTBEAT_INTERVAL_MAX);
    }
}