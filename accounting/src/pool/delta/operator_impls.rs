use std::collections::BTreeMap;

use common::{
    chain::OutPoint,
    primitives::{Amount, H256},
};
use crypto::key::PublicKey;

use crate::{
    error::Error,
    pool::{
        delegation::DelegationData,
        helpers::make_pool_id,
        operations::{
            CreateDelegationIdUndo, CreatePoolUndo, DecommissionPoolUndo, DelegateStakingUndo,
            PoSAccountingOperatorRead, PoSAccountingOperatorWrite, PoSAccountingUndo,
        },
        pool_data::PoolData,
    },
};

use super::{combine::combine_amount_delta, sum_maps, PoSAccountingDelta};

impl<'a> PoSAccountingOperatorWrite for PoSAccountingDelta<'a> {
    fn create_pool(
        &mut self,
        input0_outpoint: &OutPoint,
        pledge_amount: Amount,
        decommission_key: PublicKey,
    ) -> Result<PoSAccountingUndo, Error> {
        let pool_id = make_pool_id(input0_outpoint);

        {
            let current_amount = self.get_pool_balance(pool_id)?;
            if current_amount.is_some() {
                // This should never happen since it's based on an unspent input
                return Err(Error::InvariantErrorPoolBalanceAlreadyExists);
            }
        }

        {
            let current_data = self.get_pool_data(pool_id)?;
            if current_data.is_some() {
                // This should never happen since it's based on an unspent input
                return Err(Error::InvariantErrorPoolDataAlreadyExists);
            }
        }

        let pledge_amount_delta =
            pledge_amount.into_signed().ok_or(Error::PledgeValueToSignedError)?;

        self.pool_balances.insert(pool_id, pledge_amount_delta);
        self.pool_data.insert(
            pool_id,
            super::PoolDataDelta::CreatePool(PoolData::new(decommission_key)),
        );

        Ok(PoSAccountingUndo::CreatePool(CreatePoolUndo {
            input0_outpoint: input0_outpoint.clone(),
            pledge_amount,
        }))
    }

    fn undo_create_pool(&mut self, _undo_data: CreatePoolUndo) -> Result<(), Error> {
        todo!()
    }

    fn decommission_pool(&mut self, _pool_id: H256) -> Result<PoSAccountingUndo, Error> {
        todo!()
    }

    fn undo_decommission_pool(&mut self, _undo_data: DecommissionPoolUndo) -> Result<(), Error> {
        todo!()
    }

    fn create_delegation_id(
        &mut self,
        _target_pool: H256,
        _spend_key: PublicKey,
        _input0_outpoint: &OutPoint,
    ) -> Result<(H256, PoSAccountingUndo), Error> {
        todo!()
    }

    fn undo_create_delegation_id(
        &mut self,
        _undo_data: CreateDelegationIdUndo,
    ) -> Result<(), Error> {
        todo!()
    }

    fn delegate_staking(
        &mut self,
        _delegation_target: H256,
        _amount_to_delegate: Amount,
    ) -> Result<PoSAccountingUndo, Error> {
        todo!()
    }

    fn undo_delegate_staking(&mut self, _undo_data: DelegateStakingUndo) -> Result<(), Error> {
        todo!()
    }
}

impl<'a> PoSAccountingOperatorRead for PoSAccountingDelta<'a> {
    fn pool_exists(&self, pool_id: H256) -> Result<bool, Error> {
        Ok(self.parent.get_pool_data(pool_id)?.is_some())
    }

    fn get_delegation_shares(
        &self,
        pool_id: H256,
    ) -> Result<Option<BTreeMap<H256, Amount>>, Error> {
        let parent_shares = self.parent.get_pool_delegations_shares(pool_id)?.unwrap_or_default();
        let local_shares = self.get_cached_delegations_shares(pool_id).unwrap_or_default();
        if parent_shares.is_empty() && local_shares.is_empty() {
            Ok(None)
        } else {
            Ok(Some(sum_maps(parent_shares, local_shares)?))
        }
    }

    fn get_delegation_share(
        &self,
        pool_id: H256,
        delegation_id: H256,
    ) -> Result<Option<Amount>, Error> {
        let parent_share = self.parent.get_pool_delegation_share(pool_id, delegation_id)?;
        let local_share = self.pool_delegation_shares.get(&(pool_id, delegation_id));
        combine_amount_delta(&parent_share, &local_share.copied())
    }

    fn get_pool_balance(&self, pool_id: H256) -> Result<Option<Amount>, Error> {
        let parent_amount = self.parent.get_pool_balance(pool_id)?;
        let local_amount = self.pool_balances.get(&pool_id);
        combine_amount_delta(&parent_amount, &local_amount.copied())
    }

    fn get_delegation_id_balance(&self, delegation_id: H256) -> Result<Option<Amount>, Error> {
        let parent_amount = self.parent.get_delegation_balance(delegation_id)?;
        let local_amount = self.delegation_balances.get(&delegation_id);
        combine_amount_delta(&parent_amount, &local_amount.copied())
    }

    fn get_delegation_id_data(&self, delegation_id: H256) -> Result<Option<DelegationData>, Error> {
        let parent_data = self.parent.get_delegation_data(delegation_id)?;
        let local_data = self.delegation_data.get(&delegation_id);
        match (parent_data, local_data) {
            (None, None) => Ok(None),
            (None, Some(d)) => match d {
                super::DelegationDataDelta::Add(d) => Ok(Some(*d.clone())),
                super::DelegationDataDelta::Remove => Err(Error::RemovingNonexistingDelegationData),
            },
            (Some(p), None) => Ok(Some(p)),
            (Some(_), Some(d)) => match d {
                super::DelegationDataDelta::Add(_) => {
                    Err(Error::DelegationDataCreatedMultipleTimes)
                }
                super::DelegationDataDelta::Remove => Ok(None),
            },
        }
    }

    fn get_pool_data(&self, pool_id: H256) -> Result<Option<PoolData>, Error> {
        let parent_data = self.parent.get_pool_data(pool_id)?;
        let local_data = self.pool_data.get(&pool_id);
        match (parent_data, local_data) {
            (None, None) => Ok(None),
            (None, Some(d)) => match d {
                super::PoolDataDelta::CreatePool(d) => Ok(Some(d.clone())),
                super::PoolDataDelta::DecommissionPool => Err(Error::RemovingNonexistingPoolData),
            },
            (Some(p), None) => Ok(Some(p)),
            (Some(_), Some(d)) => match d {
                super::PoolDataDelta::CreatePool(_) => Err(Error::PoolCreatedMultipleTimes),
                super::PoolDataDelta::DecommissionPool => Ok(None),
            },
        }
    }
}
