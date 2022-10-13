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

use crate::{address::pubkeyhash::PublicKeyHash, chain::tokens::OutputValue, primitives::Id};
use script::Script;
use serialization::{Decode, Encode};

pub use self::stakelock::StakePoolData;

pub mod stakelock;

use self::timelock::OutputTimeLock;

pub mod timelock;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Encode, Decode)]
pub enum Destination {
    #[codec(index = 0)]
    Address(PublicKeyHash), // Address type to be added
    #[codec(index = 1)]
    PublicKey(crypto::key::PublicKey), // Key type to be added
    #[codec(index = 2)]
    ScriptHash(Id<Script>),
    #[codec(index = 3)]
    AnyoneCanSpend, // zero verification; used primarily for testing. Never use this for real money
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Encode, Decode)]
pub enum OutputPurpose {
    #[codec(index = 0)]
    Transfer(Destination),
    #[codec(index = 1)]
    LockThenTransfer(Destination, OutputTimeLock),
    #[codec(index = 2)]
    // TODO(PR): remove the option, it's here only to simplify development
    StakePool(Option<Box<StakePoolData>>),
}

impl OutputPurpose {
    // TODO(PR) restore returning a reference here
    pub fn destination(&self) -> Destination {
        match self {
            OutputPurpose::Transfer(d) => d.clone(),
            OutputPurpose::LockThenTransfer(d, _) => d.clone(),
            OutputPurpose::StakePool(d) => match d {
                Some(v) => v.owner().clone(),
                None => Destination::AnyoneCanSpend,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct TxOutput {
    value: OutputValue,
    purpose: OutputPurpose,
}

impl TxOutput {
    pub fn new(value: OutputValue, purpose: OutputPurpose) -> Self {
        TxOutput { value, purpose }
    }

    pub fn value(&self) -> &OutputValue {
        &self.value
    }

    pub fn purpose(&self) -> &OutputPurpose {
        &self.purpose
    }

    pub fn has_timelock(&self) -> bool {
        match &self.purpose {
            OutputPurpose::Transfer(_) => false,
            OutputPurpose::LockThenTransfer(_, _) => true,
            OutputPurpose::StakePool(_) => false,
        }
    }
}
