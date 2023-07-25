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

use std::collections::{btree_map::Entry, BTreeMap};

use common::{
    chain::{timelock::OutputTimeLock, AccountOutPoint, AccountSpending, ChainConfig, TxOutput},
    primitives::{Amount, BlockDistance, BlockHeight},
};
use pos_accounting::PoSAccountingView;

use crate::error::ConnectTransactionError;

use super::IOPolicyError;

/// `ConstrainedValueAccumulator` helps avoiding messy inputs/outputs combinations analysis by
/// providing a set of properties that should be satisfied. For example instead of checking that
/// all outputs are timelocked when the pool is decommissioned `ConstrainedValueAccumulator` gives a way
/// to check that an accumulated output value is locked for sufficient amount of time which allows
/// using other valid inputs and outputs in the same tx.
///
/// TODO: potentially this struct can be extended to collect tokens replacing `AmountsMap`
pub struct ConstrainedValueAccumulator {
    unconstrained_value: Amount,
    timelock_constrained: BTreeMap<BlockDistance, Amount>,
}

impl ConstrainedValueAccumulator {
    pub fn new() -> Self {
        Self {
            unconstrained_value: Amount::ZERO,
            timelock_constrained: Default::default(),
        }
    }

    /// Return accumulated amounts that are left
    // TODO: for now only used in tests, but should be used to calculate fees
    #[allow(dead_code)]
    pub fn consume(self) -> Result<Amount, IOPolicyError> {
        self.timelock_constrained
            .values()
            .copied()
            .into_iter()
            .sum::<Option<Amount>>()
            .and_then(|v| v + self.unconstrained_value)
            .ok_or(IOPolicyError::AmountOverflow)
    }

    pub fn process_input_utxo(
        &mut self,
        chain_config: &ChainConfig,
        block_height: BlockHeight,
        pos_accounting_view: &impl PoSAccountingView,
        output: &TxOutput,
    ) -> Result<(), ConnectTransactionError> {
        match output {
            TxOutput::Transfer(value, _)
            | TxOutput::LockThenTransfer(value, _, _)
            | TxOutput::Burn(value) => {
                if let Some(coins) = value.coin_amount() {
                    self.unconstrained_value =
                        (self.unconstrained_value + coins).ok_or(IOPolicyError::AmountOverflow)?;
                }
            }
            TxOutput::DelegateStaking(coins, _) => {
                self.unconstrained_value =
                    (self.unconstrained_value + *coins).ok_or(IOPolicyError::AmountOverflow)?;
            }
            TxOutput::CreateDelegationId(..) => { /* do nothing */ }
            TxOutput::CreateStakePool(pool_id, _) | TxOutput::ProduceBlockFromStake(_, pool_id) => {
                let block_distance =
                    chain_config.as_ref().decommission_pool_maturity_distance(block_height);
                let pledged_amount = pos_accounting_view
                    .get_pool_data(*pool_id)
                    .map_err(|_| pos_accounting::Error::ViewFail)?
                    .ok_or(ConnectTransactionError::PoolDataNotFound(*pool_id))?
                    .pledge_amount();
                match self.timelock_constrained.entry(block_distance) {
                    Entry::Vacant(e) => {
                        e.insert(pledged_amount);
                    }
                    Entry::Occupied(mut e) => {
                        let new_balance =
                            (*e.get() + pledged_amount).ok_or(IOPolicyError::AmountOverflow)?;
                        *e.get_mut() = new_balance;
                    }
                };
            }
        };

        Ok(())
    }

    pub fn process_input_from_account(
        &mut self,
        chain_config: &ChainConfig,
        block_height: BlockHeight,
        account: &AccountOutPoint,
    ) -> Result<(), ConnectTransactionError> {
        match account.account() {
            AccountSpending::Delegation(_, spend_amount) => {
                let block_distance =
                    chain_config.as_ref().spend_share_maturity_distance(block_height);
                match self.timelock_constrained.entry(block_distance) {
                    Entry::Vacant(e) => {
                        e.insert(*spend_amount);
                    }
                    Entry::Occupied(mut e) => {
                        let new_balance =
                            (*e.get() + *spend_amount).ok_or(IOPolicyError::AmountOverflow)?;
                        *e.get_mut() = new_balance;
                    }
                };
            }
        };
        Ok(())
    }

    pub fn process_output(&mut self, output: &TxOutput) -> Result<(), ConnectTransactionError> {
        match output {
            TxOutput::Transfer(value, _) | TxOutput::Burn(value) => {
                if let Some(coins) = value.coin_amount() {
                    self.unconstrained_value =
                        (self.unconstrained_value - coins).ok_or(IOPolicyError::MoneyPrinting)?;
                }
            }
            TxOutput::DelegateStaking(coins, _) => {
                self.unconstrained_value =
                    (self.unconstrained_value - *coins).ok_or(IOPolicyError::MoneyPrinting)?;
            }
            TxOutput::CreateStakePool(_, data) => {
                self.unconstrained_value = (self.unconstrained_value - data.value())
                    .ok_or(IOPolicyError::MoneyPrinting)?;
            }
            TxOutput::ProduceBlockFromStake(_, _) | TxOutput::CreateDelegationId(_, _) => {
                /* do nothing */
            }
            TxOutput::LockThenTransfer(value, _, timelock) => match timelock {
                OutputTimeLock::UntilHeight(_)
                | OutputTimeLock::UntilTime(_)
                | OutputTimeLock::ForSeconds(_) => { /* do nothing */ }
                OutputTimeLock::ForBlockCount(block_count) => {
                    if let Some(mut coins) = value.coin_amount() {
                        let block_count: i64 = (*block_count)
                            .try_into()
                            .map_err(|_| ConnectTransactionError::BlockHeightArithmeticError)?;
                        let distance = BlockDistance::from(block_count);

                        // find max value that can be saturated with the current timelock
                        let range = self.timelock_constrained.range_mut((
                            std::ops::Bound::Unbounded,
                            std::ops::Bound::Included(distance),
                        ));

                        let mut range_iter = range.rev().peekable();

                        // subtract output coins from constrained values, starting from max until
                        // all coins are used
                        while coins > Amount::ZERO {
                            match range_iter.peek_mut() {
                                Some((_, locked_coins)) => {
                                    if coins > **locked_coins {
                                        coins = (coins - **locked_coins).expect("cannot fail");
                                        **locked_coins = Amount::ZERO;
                                        range_iter.next();
                                    } else {
                                        **locked_coins =
                                            (**locked_coins - coins).expect("cannot fail");
                                        coins = Amount::ZERO;
                                    }
                                }
                                None => {
                                    self.unconstrained_value =
                                        (self.unconstrained_value - coins)
                                            .ok_or(IOPolicyError::MoneyPrinting)?;
                                    coins = Amount::ZERO;
                                }
                            };
                        }
                    }
                }
            },
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use common::{
        chain::{
            config::ChainType, stakelock::StakePoolData, timelock::OutputTimeLock,
            tokens::OutputValue, AccountNonce, AccountOutPoint, AccountSpending, ConsensusUpgrade,
            DelegationId, Destination, NetUpgrades, PoSChainConfig, PoolId, TxOutput,
            UpgradeVersion,
        },
        primitives::{per_thousand::PerThousand, Amount, H256},
        Uint256,
    };
    use crypto::{
        random::Rng,
        vrf::{VRFKeyKind, VRFPrivateKey},
    };
    use rstest::rstest;
    use test_utils::random::{make_seedable_rng, Seed};

    fn create_stake_pool_data(atoms_to_stake: u128) -> StakePoolData {
        let (_, vrf_pub_key) = VRFPrivateKey::new_from_entropy(VRFKeyKind::Schnorrkel);
        StakePoolData::new(
            Amount::from_atoms(atoms_to_stake),
            Destination::AnyoneCanSpend,
            vrf_pub_key,
            Destination::AnyoneCanSpend,
            PerThousand::new(0).unwrap(),
            Amount::ZERO,
        )
    }

    #[rstest]
    #[trace]
    #[case(Seed::from_entropy())]
    fn allow_fees_from_decommission(#[case] seed: Seed) {
        let mut rng = make_seedable_rng(seed);

        let chain_config = common::chain::config::Builder::new(ChainType::Mainnet)
            .net_upgrades(NetUpgrades::regtest_with_pos())
            .build();
        let required_maturity_distance =
            chain_config.decommission_pool_maturity_distance(BlockHeight::new(1));

        let pool_id = PoolId::new(H256::zero());
        let staked_atoms = rng.gen_range(100..1000);
        let fee_atoms = rng.gen_range(1..100);
        let stake_pool_data = create_stake_pool_data(staked_atoms);

        let pos_store = pos_accounting::InMemoryPoSAccounting::from_values(
            BTreeMap::from([(pool_id, stake_pool_data.clone().into())]),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let pos_db = pos_accounting::PoSAccountingDB::new(&pos_store);

        let input_utxos = vec![TxOutput::CreateStakePool(pool_id, Box::new(stake_pool_data))];

        let outputs = vec![TxOutput::LockThenTransfer(
            OutputValue::Coin(Amount::from_atoms(staked_atoms - fee_atoms)),
            Destination::AnyoneCanSpend,
            OutputTimeLock::ForBlockCount(required_maturity_distance.into_int() as u64),
        )];

        let mut constraints_accumulator = ConstrainedValueAccumulator::new();

        for input in input_utxos {
            constraints_accumulator
                .process_input_utxo(&chain_config, BlockHeight::new(1), &pos_db, &input)
                .unwrap();
        }

        for output in outputs {
            constraints_accumulator.process_output(&output).unwrap();
        }

        assert_eq!(
            constraints_accumulator.consume().unwrap().into_atoms(),
            fee_atoms
        );
    }

    #[rstest]
    #[trace]
    #[case(Seed::from_entropy())]
    fn allow_fees_from_spend_share(#[case] seed: Seed) {
        let mut rng = make_seedable_rng(seed);

        let chain_config = common::chain::config::Builder::new(ChainType::Mainnet)
            .net_upgrades(NetUpgrades::regtest_with_pos())
            .build();
        let required_maturity_distance =
            chain_config.spend_share_maturity_distance(BlockHeight::new(1));

        let delegation_id = DelegationId::new(H256::zero());
        let delegated_atoms = rng.gen_range(1..1000);
        let fee_atoms = rng.gen_range(1..100);

        let input_account = AccountOutPoint::new(
            AccountNonce::new(0),
            AccountSpending::Delegation(delegation_id, Amount::from_atoms(delegated_atoms)),
        );

        let outputs = vec![TxOutput::LockThenTransfer(
            OutputValue::Coin(Amount::from_atoms(delegated_atoms - fee_atoms)),
            Destination::AnyoneCanSpend,
            OutputTimeLock::ForBlockCount(required_maturity_distance.into_int() as u64),
        )];

        let mut constraints_accumulator = ConstrainedValueAccumulator::new();

        constraints_accumulator
            .process_input_from_account(&chain_config, BlockHeight::new(1), &input_account)
            .unwrap();

        for output in outputs {
            constraints_accumulator.process_output(&output).unwrap();
        }

        assert_eq!(
            constraints_accumulator.consume().unwrap().into_atoms(),
            fee_atoms
        );
    }

    #[rstest]
    #[trace]
    #[case(Seed::from_entropy())]
    fn try_to_unlocked_coins(#[case] seed: Seed) {
        let mut rng = make_seedable_rng(seed);

        let chain_config = common::chain::config::Builder::new(ChainType::Mainnet)
            .net_upgrades(NetUpgrades::regtest_with_pos())
            .build();
        let required_maturity_distance =
            chain_config.decommission_pool_maturity_distance(BlockHeight::new(1));

        let pool_id = PoolId::new(H256::zero());
        let staked_atoms = rng.gen_range(100..1000);
        let stake_pool_data = create_stake_pool_data(staked_atoms);

        let pos_store = pos_accounting::InMemoryPoSAccounting::from_values(
            BTreeMap::from([(pool_id, stake_pool_data.clone().into())]),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let pos_db = pos_accounting::PoSAccountingDB::new(&pos_store);

        let input_utxos = vec![
            TxOutput::CreateStakePool(pool_id, Box::new(stake_pool_data)),
            TxOutput::Transfer(
                OutputValue::Coin(Amount::from_atoms(100)),
                Destination::AnyoneCanSpend,
            ),
        ];

        let outputs = vec![
            TxOutput::LockThenTransfer(
                OutputValue::Coin(Amount::from_atoms(staked_atoms - 10)),
                Destination::AnyoneCanSpend,
                OutputTimeLock::ForBlockCount(required_maturity_distance.into_int() as u64),
            ),
            TxOutput::LockThenTransfer(
                OutputValue::Coin(Amount::from_atoms(10)),
                Destination::AnyoneCanSpend,
                OutputTimeLock::ForBlockCount(required_maturity_distance.into_int() as u64 - 1),
            ),
            TxOutput::Transfer(
                OutputValue::Coin(Amount::from_atoms(100)),
                Destination::AnyoneCanSpend,
            ),
        ];

        let mut constraints_accumulator = ConstrainedValueAccumulator::new();

        for input in input_utxos {
            constraints_accumulator
                .process_input_utxo(&chain_config, BlockHeight::new(1), &pos_db, &input)
                .unwrap();
        }

        constraints_accumulator.process_output(&outputs[0]).unwrap();
        constraints_accumulator.process_output(&outputs[1]).unwrap();
        let result = constraints_accumulator.process_output(&outputs[2]).unwrap_err();
        assert_eq!(
            result,
            ConnectTransactionError::IOPolicyError(IOPolicyError::MoneyPrinting)
        );
    }

    #[rstest]
    #[trace]
    #[case(Seed::from_entropy())]
    fn check_timelock_saturation(#[case] seed: Seed) {
        let mut rng = make_seedable_rng(seed);

        let required_decommission_maturity = 100;
        let required_spend_share_maturity = 200;
        let upgrades = vec![(
            BlockHeight::new(0),
            UpgradeVersion::ConsensusUpgrade(ConsensusUpgrade::PoS {
                initial_difficulty: Uint256::MAX.into(),
                config: PoSChainConfig::new(
                    Uint256::MAX,
                    1,
                    required_decommission_maturity.into(),
                    required_spend_share_maturity.into(),
                    2,
                    PerThousand::new(0).unwrap(),
                )
                .unwrap(),
            }),
        )];
        let net_upgrades = NetUpgrades::initialize(upgrades).expect("valid net-upgrades");
        let chain_config = common::chain::config::Builder::new(ChainType::Mainnet)
            .net_upgrades(net_upgrades)
            .build();

        let pool_id = PoolId::new(H256::zero());
        let staked_atoms = rng.gen_range(100..1000);
        let stake_pool_data = create_stake_pool_data(staked_atoms);

        let delegation_id = DelegationId::new(H256::zero());
        let delegated_atoms = rng.gen_range(1..1000);

        let transferred_atoms = rng.gen_range(100..1000);

        let pos_store = pos_accounting::InMemoryPoSAccounting::from_values(
            BTreeMap::from([(pool_id, stake_pool_data.clone().into())]),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(delegation_id, Amount::from_atoms(delegated_atoms))]),
            BTreeMap::new(),
        );
        let pos_db = pos_accounting::PoSAccountingDB::new(&pos_store);

        let input_utxos = vec![
            TxOutput::CreateStakePool(pool_id, Box::new(stake_pool_data)),
            TxOutput::Transfer(
                OutputValue::Coin(Amount::from_atoms(transferred_atoms)),
                Destination::AnyoneCanSpend,
            ),
        ];

        let input_account = AccountOutPoint::new(
            AccountNonce::new(0),
            AccountSpending::Delegation(delegation_id, Amount::from_atoms(delegated_atoms)),
        );

        let outputs = vec![
            TxOutput::LockThenTransfer(
                OutputValue::Coin(Amount::from_atoms(staked_atoms + delegated_atoms)),
                Destination::AnyoneCanSpend,
                OutputTimeLock::ForBlockCount(
                    required_decommission_maturity as u64 + required_spend_share_maturity as u64,
                ),
            ),
            TxOutput::Transfer(
                OutputValue::Coin(Amount::from_atoms(transferred_atoms)),
                Destination::AnyoneCanSpend,
            ),
        ];

        let mut constraints_accumulator = ConstrainedValueAccumulator::new();

        for input in input_utxos {
            constraints_accumulator
                .process_input_utxo(&chain_config, BlockHeight::new(1), &pos_db, &input)
                .unwrap();
        }

        constraints_accumulator
            .process_input_from_account(&chain_config, BlockHeight::new(1), &input_account)
            .unwrap();

        constraints_accumulator.process_output(&outputs[0]).unwrap();
        constraints_accumulator.process_output(&outputs[1]).unwrap();

        assert_eq!(constraints_accumulator.consume().unwrap(), Amount::ZERO);
    }
}
