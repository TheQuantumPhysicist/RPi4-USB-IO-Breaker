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

use wallet_types::{AccountWalletTxId, WalletTx};

/// Callbacks that are invoked when the database is updated and the UI should be re-rendered
pub trait WalletEvents {
    fn new_block(&mut self);
    fn set_transaction(&mut self, id: &AccountWalletTxId, tx: &WalletTx);
    fn del_transaction(&mut self, id: &AccountWalletTxId);
}

pub struct WalletEventsNoOp;

impl WalletEvents for WalletEventsNoOp {
    fn new_block(&mut self) {}
    fn set_transaction(&mut self, _id: &AccountWalletTxId, _tx: &WalletTx) {}
    fn del_transaction(&mut self, _id: &AccountWalletTxId) {}
}
