// Copyright (c) 2023 RBB S.r.l
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

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use super::*;
use accounting::{DataDelta, DeltaAmountCollection, DeltaDataCollection};
use chainstate_storage::{inmemory::Store, BlockchainStorageRead, Transactional};
use chainstate_test_framework::{
    anyonecanspend_address, empty_witness, TestFramework, TransactionBuilder,
};
use common::{
    chain::{
        config::Builder as ConfigBuilder, stakelock::StakePoolData, tokens::OutputValue, OutPoint,
        OutPointSourceId, PoolId, SignedTransaction, Transaction, TxInput, TxOutput,
    },
    primitives::{signed_amount::SignedAmount, Amount, Id, Idable},
};
use crypto::{
    key::{KeyKind, PrivateKey, PublicKey},
    random::CryptoRng,
    vrf::{VRFKeyKind, VRFPrivateKey},
};
use pos_accounting::PoolData;
use utxo::UtxosStorageRead;

fn make_tx_with_stake_pool_from_genesis(
    rng: &mut (impl Rng + CryptoRng),
    tf: &mut TestFramework,
    amount_to_stake: Amount,
    pub_key: &PublicKey,
) -> (SignedTransaction, PoolId) {
    let outpoint_id = OutPointSourceId::BlockReward(tf.genesis().get_id().into());
    make_tx_with_stake_pool(rng, outpoint_id, amount_to_stake, pub_key)
}

fn make_tx_with_stake_pool_from_tx(
    rng: &mut (impl Rng + CryptoRng),
    tx_id: Id<Transaction>,
    amount_to_stake: Amount,
    pub_key: &PublicKey,
) -> (SignedTransaction, PoolId) {
    let outpoint_id = OutPointSourceId::Transaction(tx_id);
    make_tx_with_stake_pool(rng, outpoint_id, amount_to_stake, pub_key)
}

fn make_tx_with_stake_pool(
    rng: &mut (impl Rng + CryptoRng),
    outpoint_id: OutPointSourceId,
    amount_to_stake: Amount,
    pub_key: &PublicKey,
) -> (SignedTransaction, PoolId) {
    let (_, vrf_pub_key) = VRFPrivateKey::new_from_rng(rng, VRFKeyKind::Schnorrkel);
    let tx_output = TxOutput::new(
        OutputValue::Coin(amount_to_stake),
        OutputPurpose::StakePool(Box::new(StakePoolData::new(
            anyonecanspend_address(),
            None,
            vrf_pub_key,
            pub_key.clone(),
            0,
            Amount::ZERO,
        ))),
    );

    let input0_outpoint = OutPoint::new(outpoint_id, 0);
    let pool_id = pos_accounting::make_pool_id(&input0_outpoint);
    let tx = TransactionBuilder::new()
        .add_input(
            TxInput::new(input0_outpoint.tx_id(), input0_outpoint.output_index()),
            empty_witness(rng),
        )
        .add_output(tx_output)
        .build();
    (tx, pool_id)
}

// Process a tx with a stake pool. Check that new pool balance and data are stored
#[rstest]
#[trace]
#[case(Seed::from_entropy())]
fn store_pool_data_and_balance(#[case] seed: Seed) {
    utils::concurrency::model(move || {
        let storage = Store::new_empty().unwrap();
        let mut rng = make_seedable_rng(seed);
        let mut tf = TestFramework::builder(&mut rng).with_storage(storage.clone()).build();
        let amount_to_stake = Amount::from_atoms(100);
        let (_, pub_key) = PrivateKey::new_from_rng(&mut rng, KeyKind::Secp256k1Schnorr);

        let (tx, pool_id) =
            make_tx_with_stake_pool_from_genesis(&mut rng, &mut tf, amount_to_stake, &pub_key);
        let tx_utxo_outpoint =
            OutPoint::new(OutPointSourceId::Transaction(tx.transaction().get_id()), 0);

        let block = tf.make_block_builder().add_transaction(tx).build();
        let block_id = block.get_id();
        tf.process_block(block, BlockSource::Local).unwrap();

        // check that result is stored
        let db_tx = storage.transaction_ro().unwrap();

        // utxo is stored
        db_tx.get_utxo(&tx_utxo_outpoint).expect("ok").expect("some");
        assert_eq!(
            db_tx.get_undo_data(block_id).expect("ok").expect("some").tx_undos().len(),
            1
        );

        let expected_tip_storage_data = pos_accounting::PoSAccountingData {
            pool_data: BTreeMap::from([(pool_id, PoolData::new(pub_key, amount_to_stake))]),
            pool_balances: BTreeMap::from([(pool_id, amount_to_stake)]),
            delegation_balances: Default::default(),
            delegation_data: Default::default(),
            pool_delegation_shares: Default::default(),
        };

        assert_eq!(
            storage.read_accounting_data_tip().unwrap(),
            expected_tip_storage_data
        );

        assert!(storage.read_accounting_data_sealed().unwrap().is_empty());
    });
}

// Create block1 from genesis and block2 from block1 using chain config
// that will put them in the same epoch.
// Every block creates a pool.
// Check that block1 and block2 belong to the same epochs and no epoch was sealed.
// Check that accounting info from both blocks got into tip and not into sealed storage.
// Check that deltas from both blocks is stored.
#[rstest]
#[trace]
#[case(Seed::from_entropy())]
fn accounting_storage_two_blocks_one_epoch_no_seal(#[case] seed: Seed) {
    utils::concurrency::model(move || {
        let storage = Store::new_empty().unwrap();
        let mut rng = make_seedable_rng(seed);
        let chain_config = ConfigBuilder::test_chain()
            .epoch_length(NonZeroU64::new(3).unwrap())
            .sealed_epoch_distance_from_tip(2)
            .build();
        let mut tf = TestFramework::builder(&mut rng)
            .with_storage(storage.clone())
            .with_chain_config(chain_config)
            .build();
        let amount_to_stake = Amount::from_atoms(100);
        let (_, pub_key) = PrivateKey::new_from_rng(&mut rng, KeyKind::Secp256k1Schnorr);
        let expected_epoch_index = 0;

        let (tx1, pool_id1) =
            make_tx_with_stake_pool_from_genesis(&mut rng, &mut tf, amount_to_stake, &pub_key);

        let (tx2, pool_id2) = make_tx_with_stake_pool_from_tx(
            &mut rng,
            tx1.transaction().get_id(),
            amount_to_stake,
            &pub_key,
        );

        let block1_index = tf
            .make_block_builder()
            .add_transaction(tx1)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block1_index.block_height()),
            expected_epoch_index
        );
        let block2_index = tf
            .make_block_builder()
            .add_transaction(tx2)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block2_index.block_height()),
            expected_epoch_index
        );

        // check that result is stored to tip
        let expected_tip_storage_data = pos_accounting::PoSAccountingData {
            pool_data: BTreeMap::from([
                (pool_id1, PoolData::new(pub_key.clone(), amount_to_stake)),
                (pool_id2, PoolData::new(pub_key.clone(), amount_to_stake)),
            ]),
            pool_balances: BTreeMap::from([
                (pool_id1, amount_to_stake),
                (pool_id2, amount_to_stake),
            ]),
            delegation_balances: Default::default(),
            delegation_data: Default::default(),
            pool_delegation_shares: Default::default(),
        };

        assert_eq!(
            storage.read_accounting_data_tip().unwrap(),
            expected_tip_storage_data
        );

        // check that result is not stored to sealed
        assert!(storage.read_accounting_data_sealed().unwrap().is_empty());

        // check that delta for epoch is stored
        let pool_data = PoolData::new(pub_key.clone(), amount_to_stake);
        let expected_epoch_delta = pos_accounting::PoSAccountingDeltaData {
            pool_data: DeltaDataCollection::from_iter(
                [
                    (pool_id1, DataDelta::new(None, Some(pool_data.clone()))),
                    (pool_id2, DataDelta::new(None, Some(pool_data))),
                ]
                .into_iter(),
            ),
            pool_balances: DeltaAmountCollection::from_iter(
                [
                    (pool_id1, amount_to_stake.into_signed().unwrap()),
                    (pool_id2, amount_to_stake.into_signed().unwrap()),
                ]
                .into_iter(),
            ),
            pool_delegation_shares: DeltaAmountCollection::new(),
            delegation_balances: DeltaAmountCollection::new(),
            delegation_data: DeltaDataCollection::new(),
        };

        let epoch_delta = storage
            .get_accounting_epoch_delta(expected_epoch_index)
            .expect("ok")
            .expect("some");
        assert_eq!(epoch_delta, expected_epoch_delta);

        assert!(storage.get_accounting_epoch_undo_delta(0).unwrap().is_none());
    });
}

// Config chain to seal an epoch every block.
// Create block1 from genesis and block2 from block1.
// Every block creates a stake pool.
// Check that block1 and block2 belong to different epochs, but no epoch was sealed.
// Check that accounting info from both blocks got into tip and but not into sealed storage.
// Check that deltas from both blocks is stored.
#[rstest]
#[trace]
#[case(Seed::from_entropy())]
fn accounting_storage_two_epochs_no_seal(#[case] seed: Seed) {
    utils::concurrency::model(move || {
        let storage = Store::new_empty().unwrap();
        let mut rng = make_seedable_rng(seed);
        let chain_config =
            ConfigBuilder::test_chain().epoch_length(NonZeroU64::new(1).unwrap()).build();
        let mut tf = TestFramework::builder(&mut rng)
            .with_storage(storage.clone())
            .with_chain_config(chain_config)
            .build();
        let amount_to_stake = Amount::from_atoms(100);
        let (_, pub_key) = PrivateKey::new_from_rng(&mut rng, KeyKind::Secp256k1Schnorr);
        // genesis block takes epoch 0, so new blocks start from epoch 1
        let block1_epoch_index = 1;
        let block2_epoch_index = 2;

        let (tx1, pool_id1) =
            make_tx_with_stake_pool_from_genesis(&mut rng, &mut tf, amount_to_stake, &pub_key);

        let (tx2, pool_id2) = make_tx_with_stake_pool_from_tx(
            &mut rng,
            tx1.transaction().get_id(),
            amount_to_stake,
            &pub_key,
        );

        let block1_index = tf
            .make_block_builder()
            .add_transaction(tx1)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block1_index.block_height()),
            block1_epoch_index
        );
        let block2_index = tf
            .make_block_builder()
            .add_transaction(tx2)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block2_index.block_height()),
            block2_epoch_index
        );

        // check that result is stored to tip
        let expected_tip_storage_data = pos_accounting::PoSAccountingData {
            pool_data: BTreeMap::from([
                (pool_id1, PoolData::new(pub_key.clone(), amount_to_stake)),
                (pool_id2, PoolData::new(pub_key.clone(), amount_to_stake)),
            ]),
            pool_balances: BTreeMap::from([
                (pool_id1, amount_to_stake),
                (pool_id2, amount_to_stake),
            ]),
            delegation_balances: Default::default(),
            delegation_data: Default::default(),
            pool_delegation_shares: Default::default(),
        };

        assert_eq!(
            storage.read_accounting_data_tip().unwrap(),
            expected_tip_storage_data
        );

        // check that result is not stored to sealed
        assert!(storage.read_accounting_data_sealed().unwrap().is_empty());

        // check that deltas per block are stored
        let pool_data = PoolData::new(pub_key.clone(), amount_to_stake);
        let expected_epoch1_delta = pos_accounting::PoSAccountingDeltaData {
            pool_data: DeltaDataCollection::from_iter(
                [(pool_id1, DataDelta::new(None, Some(pool_data.clone())))].into_iter(),
            ),
            pool_balances: DeltaAmountCollection::from_iter(
                [(pool_id1, SignedAmount::from_atoms(100))].into_iter(),
            ),
            pool_delegation_shares: DeltaAmountCollection::new(),
            delegation_balances: DeltaAmountCollection::new(),
            delegation_data: DeltaDataCollection::new(),
        };

        let epoch1_delta = storage
            .get_accounting_epoch_delta(block1_epoch_index)
            .expect("ok")
            .expect("some");
        assert_eq!(epoch1_delta, expected_epoch1_delta);

        let expected_epoch2_delta = pos_accounting::PoSAccountingDeltaData {
            pool_data: DeltaDataCollection::from_iter(
                [(pool_id2, DataDelta::new(None, Some(pool_data)))].into_iter(),
            ),
            pool_balances: DeltaAmountCollection::from_iter(
                [(pool_id2, amount_to_stake.into_signed().unwrap())].into_iter(),
            ),
            pool_delegation_shares: DeltaAmountCollection::new(),
            delegation_balances: DeltaAmountCollection::new(),
            delegation_data: DeltaDataCollection::new(),
        };

        let epoch2_delta = storage
            .get_accounting_epoch_delta(block2_epoch_index)
            .expect("ok")
            .expect("some");
        assert_eq!(epoch2_delta, expected_epoch2_delta);

        assert!(storage.get_accounting_epoch_undo_delta(0).unwrap().is_none());
    });
}

// Config chain to seal an epoch every block and the distance between tip and sealed to 1.
// Create block1 from genesis and block2 from block1.
// Every block creates a stake pool.
// Check that block1 and block2 belong to different epochs, and that epoch1 was sealed.
// Check that accounting info from both blocks got into tip.
// Check that only accounting info from block1 got into sealed storage.
// Check that deltas from both blocks is stored.
#[rstest]
#[trace]
#[case(Seed::from_entropy())]
fn accounting_storage_seal_one_epoch(#[case] seed: Seed) {
    utils::concurrency::model(move || {
        let storage = Store::new_empty().unwrap();
        let mut rng = make_seedable_rng(seed);
        let chain_config = ConfigBuilder::test_chain()
            .epoch_length(NonZeroU64::new(1).unwrap())
            .sealed_epoch_distance_from_tip(1)
            .build();
        let mut tf = TestFramework::builder(&mut rng)
            .with_storage(storage.clone())
            .with_chain_config(chain_config)
            .build();
        let amount_to_stake = Amount::from_atoms(100);
        let (_, pub_key) = PrivateKey::new_from_rng(&mut rng, KeyKind::Secp256k1Schnorr);
        // genesis block takes epoch 0, so new blocks start from epoch 1
        let block1_epoch_index = 1;
        let block2_epoch_index = 2;

        let (tx1, pool_id1) =
            make_tx_with_stake_pool_from_genesis(&mut rng, &mut tf, amount_to_stake, &pub_key);

        let (tx2, pool_id2) = make_tx_with_stake_pool_from_tx(
            &mut rng,
            tx1.transaction().get_id(),
            amount_to_stake,
            &pub_key,
        );

        let block1_index = tf
            .make_block_builder()
            .add_transaction(tx1)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block1_index.block_height()),
            block1_epoch_index
        );
        let block2_index = tf
            .make_block_builder()
            .add_transaction(tx2)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block2_index.block_height()),
            block2_epoch_index
        );

        // check that result is stored to tip
        let expected_tip_storage_data = pos_accounting::PoSAccountingData {
            pool_data: BTreeMap::from([
                (pool_id1, PoolData::new(pub_key.clone(), amount_to_stake)),
                (pool_id2, PoolData::new(pub_key.clone(), amount_to_stake)),
            ]),
            pool_balances: BTreeMap::from([
                (pool_id1, amount_to_stake),
                (pool_id2, amount_to_stake),
            ]),
            delegation_balances: Default::default(),
            delegation_data: Default::default(),
            pool_delegation_shares: Default::default(),
        };
        assert_eq!(
            storage.read_accounting_data_tip().unwrap(),
            expected_tip_storage_data
        );

        // check that epoch1 is stored to sealed
        let expected_sealed_storage_data = pos_accounting::PoSAccountingData {
            pool_data: BTreeMap::from([(
                pool_id1,
                PoolData::new(pub_key.clone(), amount_to_stake),
            )]),
            pool_balances: BTreeMap::from([(pool_id1, amount_to_stake)]),
            delegation_balances: Default::default(),
            delegation_data: Default::default(),
            pool_delegation_shares: Default::default(),
        };
        assert_eq!(
            storage.read_accounting_data_sealed().unwrap(),
            expected_sealed_storage_data
        );

        // check that deltas per block are stored
        let pool_data = PoolData::new(pub_key.clone(), Amount::from_atoms(100));
        let expected_epoch1_delta = pos_accounting::PoSAccountingDeltaData {
            pool_data: DeltaDataCollection::from_iter(
                [(pool_id1, DataDelta::new(None, Some(pool_data.clone())))].into_iter(),
            ),
            pool_balances: DeltaAmountCollection::from_iter(
                [(pool_id1, amount_to_stake.into_signed().unwrap())].into_iter(),
            ),
            pool_delegation_shares: DeltaAmountCollection::new(),
            delegation_balances: DeltaAmountCollection::new(),
            delegation_data: DeltaDataCollection::new(),
        };

        let epoch1_delta = storage
            .get_accounting_epoch_delta(block1_epoch_index)
            .expect("ok")
            .expect("some");
        assert_eq!(epoch1_delta, expected_epoch1_delta);

        let expected_epoch2_delta = pos_accounting::PoSAccountingDeltaData {
            pool_data: DeltaDataCollection::from_iter(
                [(pool_id2, DataDelta::new(None, Some(pool_data)))].into_iter(),
            ),
            pool_balances: DeltaAmountCollection::from_iter(
                [(pool_id2, amount_to_stake.into_signed().unwrap())].into_iter(),
            ),
            pool_delegation_shares: DeltaAmountCollection::new(),
            delegation_balances: DeltaAmountCollection::new(),
            delegation_data: DeltaDataCollection::new(),
        };

        let epoch2_delta = storage
            .get_accounting_epoch_delta(block2_epoch_index)
            .expect("ok")
            .expect("some");
        assert_eq!(epoch2_delta, expected_epoch2_delta);

        assert!(storage.get_accounting_epoch_undo_delta(0).unwrap().is_none());
        assert!(storage.get_accounting_epoch_undo_delta(1).unwrap().is_some());
        assert!(storage.get_accounting_epoch_undo_delta(2).unwrap().is_none());
    });
}

// Config chain to seal an epoch every block and the distance between tip and sealed to 0
// (meaning every block is sealed thus tip == sealed).
// Create block1 from genesis that creates a stake pool.
// Check that the info is stored to the tip and sealed storage.
// Check that delta from block is stored.
#[rstest]
#[trace]
#[case(Seed::from_entropy())]
fn accounting_storage_seal_every_block(#[case] seed: Seed) {
    utils::concurrency::model(move || {
        let storage = Store::new_empty().unwrap();
        let mut rng = make_seedable_rng(seed);
        let chain_config = ConfigBuilder::test_chain()
            .epoch_length(NonZeroU64::new(1).unwrap())
            .sealed_epoch_distance_from_tip(0)
            .build();
        let mut tf = TestFramework::builder(&mut rng)
            .with_storage(storage.clone())
            .with_chain_config(chain_config)
            .build();
        let amount_to_stake = Amount::from_atoms(100);
        let (_, pub_key) = PrivateKey::new_from_rng(&mut rng, KeyKind::Secp256k1Schnorr);
        // genesis block takes epoch 0, so new blocks start from epoch 1
        let block1_epoch_index = 1;

        let (tx1, pool_id1) =
            make_tx_with_stake_pool_from_genesis(&mut rng, &mut tf, amount_to_stake, &pub_key);

        let block1_index = tf
            .make_block_builder()
            .add_transaction(tx1)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block1_index.block_height()),
            block1_epoch_index
        );

        // check that result is stored to tip and sealed
        let expected_storage_data = pos_accounting::PoSAccountingData {
            pool_data: BTreeMap::from([(
                pool_id1,
                PoolData::new(pub_key.clone(), amount_to_stake),
            )]),
            pool_balances: BTreeMap::from([(pool_id1, amount_to_stake)]),
            delegation_balances: Default::default(),
            delegation_data: Default::default(),
            pool_delegation_shares: Default::default(),
        };
        assert_eq!(
            storage.read_accounting_data_tip().unwrap(),
            expected_storage_data
        );
        assert_eq!(
            storage.read_accounting_data_sealed().unwrap(),
            expected_storage_data
        );

        // check that deltas per block are stored
        let pool_data = PoolData::new(pub_key.clone(), Amount::from_atoms(100));
        let expected_epoch1_delta = pos_accounting::PoSAccountingDeltaData {
            pool_data: DeltaDataCollection::from_iter(
                [(pool_id1, DataDelta::new(None, Some(pool_data)))].into_iter(),
            ),
            pool_balances: DeltaAmountCollection::from_iter(
                [(pool_id1, amount_to_stake.into_signed().unwrap())].into_iter(),
            ),
            pool_delegation_shares: DeltaAmountCollection::new(),
            delegation_balances: DeltaAmountCollection::new(),
            delegation_data: DeltaDataCollection::new(),
        };

        let epoch1_delta = storage
            .get_accounting_epoch_delta(block1_epoch_index)
            .expect("ok")
            .expect("some");
        assert_eq!(epoch1_delta, expected_epoch1_delta);

        assert!(storage.get_accounting_epoch_undo_delta(0).unwrap().is_none());
        assert!(storage.get_accounting_epoch_undo_delta(1).unwrap().is_some());
    });
}

// Config chain to seal an epoch every block and the distance between tip and sealed to 0
// (meaning every block is sealed thus tip == sealed).
// Create block1 from genesis that spend a coin (no accounting data).
// Check that epoch is changed, but tip and sealed storages are empty.
// Check that deltas per block and undo per epoch are empty.
#[rstest]
#[trace]
#[case(Seed::from_entropy())]
fn accounting_storage_no_accounting_data(#[case] seed: Seed) {
    utils::concurrency::model(move || {
        let storage = Store::new_empty().unwrap();
        let mut rng = make_seedable_rng(seed);
        let chain_config = ConfigBuilder::test_chain()
            .epoch_length(NonZeroU64::new(1).unwrap())
            .sealed_epoch_distance_from_tip(0)
            .build();
        let mut tf = TestFramework::builder(&mut rng)
            .with_storage(storage.clone())
            .with_chain_config(chain_config)
            .build();
        // genesis block takes epoch 0, so new blocks start from epoch 1
        let block1_epoch_index = 1;

        let tx1 = TransactionBuilder::new()
            .add_input(
                TxInput::new(
                    OutPointSourceId::BlockReward(tf.genesis().get_id().into()),
                    0,
                ),
                empty_witness(&mut rng),
            )
            .add_output(TxOutput::new(
                OutputValue::Coin(Amount::from_atoms(100)),
                OutputPurpose::Transfer(anyonecanspend_address()),
            ))
            .build();

        let block1_index = tf
            .make_block_builder()
            .add_transaction(tx1)
            .build_and_process()
            .expect("ok")
            .expect("some");
        assert_eq!(
            tf.chainstate
                .get_chain_config()
                .epoch_index_from_height(&block1_index.block_height()),
            block1_epoch_index
        );

        // check that result is stored to tip and sealed
        assert_eq!(
            storage.read_accounting_data_tip().unwrap(),
            pos_accounting::PoSAccountingData::new()
        );
        assert_eq!(
            storage.read_accounting_data_sealed().unwrap(),
            pos_accounting::PoSAccountingData::new()
        );

        // check that deltas per epoch are not stored
        assert!(storage.get_accounting_epoch_delta(block1_epoch_index).unwrap().is_none());

        // check that undo per epoch are not stored
        assert!(storage.get_accounting_epoch_undo_delta(0).unwrap().is_none());
        assert!(storage.get_accounting_epoch_undo_delta(1).unwrap().is_none());
    });
}