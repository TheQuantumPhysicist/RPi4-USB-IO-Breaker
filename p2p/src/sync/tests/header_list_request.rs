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

use std::{iter, sync::Arc};

use chainstate::{ban_score::BanScore, Locator};
use chainstate_test_framework::TestFramework;
use common::{chain::config::create_unit_test_config, primitives::Idable};
use p2p_test_utils::chainstate_subsystem;
use test_utils::random::Seed;

use crate::{
    error::ProtocolError,
    message::{HeaderListRequest, SyncMessage},
    sync::tests::helpers::SyncManagerHandle,
    types::peer_id::PeerId,
    P2pError,
};

#[tokio::test]
#[should_panic = "Received a message from unknown peer"]
async fn nonexistent_peer() {
    let mut handle = SyncManagerHandle::start().await;

    let peer = PeerId::new();
    handle.send_message(
        peer,
        SyncMessage::HeaderListRequest(HeaderListRequest::new(Locator::new(Vec::new()))),
    );

    handle.resume_panic().await;
}

#[rstest::rstest]
#[trace]
#[case(Seed::from_entropy())]
#[tokio::test]
async fn max_locator_size_exceeded(#[case] seed: Seed) {
    let mut rng = test_utils::random::make_seedable_rng(seed);

    let chain_config = Arc::new(create_unit_test_config());
    let mut tf = TestFramework::builder(&mut rng)
        .with_chain_config(chain_config.as_ref().clone())
        .build();
    let block = tf.make_block_builder().build();
    let chainstate = chainstate_subsystem(tf.into_chainstate()).await;

    let mut handle = SyncManagerHandle::builder()
        .with_chain_config(chain_config)
        .with_chainstate(chainstate)
        .build()
        .await;

    let peer = PeerId::new();
    handle.connect_peer(peer).await;

    let headers = iter::repeat(block.get_id().into()).take(102).collect();
    handle.send_message(
        peer,
        SyncMessage::HeaderListRequest(HeaderListRequest::new(Locator::new(headers))),
    );

    let (adjusted_peer, score) = handle.adjust_peer_score_event().await;
    assert_eq!(peer, adjusted_peer);
    assert_eq!(
        score,
        P2pError::ProtocolError(ProtocolError::LocatorSizeExceeded(0, 0)).ban_score()
    );
    handle.assert_no_event().await;
}

#[rstest::rstest]
#[trace]
#[case(Seed::from_entropy())]
#[tokio::test]
async fn valid_request(#[case] seed: Seed) {
    let mut rng = test_utils::random::make_seedable_rng(seed);

    let chain_config = Arc::new(create_unit_test_config());
    let mut tf = TestFramework::builder(&mut rng)
        .with_chain_config(chain_config.as_ref().clone())
        .build();
    // Process a block to finish the initial block download.
    tf.make_block_builder().build_and_process().unwrap().unwrap();
    let chainstate = chainstate_subsystem(tf.into_chainstate()).await;

    let mut handle = SyncManagerHandle::builder()
        .with_chain_config(chain_config)
        .with_chainstate(chainstate)
        .build()
        .await;

    let peer = PeerId::new();
    handle.connect_peer(peer).await;

    handle.send_message(
        peer,
        SyncMessage::HeaderListRequest(HeaderListRequest::new(Locator::new(Vec::new()))),
    );

    let (sent_to, message) = handle.message().await;
    assert_eq!(peer, sent_to);
    assert!(matches!(message, SyncMessage::HeaderListResponse(_)));
    handle.assert_no_error().await;
}