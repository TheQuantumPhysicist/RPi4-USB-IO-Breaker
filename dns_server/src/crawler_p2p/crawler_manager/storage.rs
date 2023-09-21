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

use p2p::decl_storage_trait;

pub trait DnsServerStorageRead {
    fn get_version(&self) -> Result<Option<u32>, storage::Error>;

    fn get_addresses(&self) -> Result<Vec<String>, storage::Error>;
}

pub trait DnsServerStorageWrite {
    fn set_version(&mut self, version: u32) -> Result<(), storage::Error>;

    fn add_address(&mut self, address: &str) -> Result<(), storage::Error>;

    fn del_address(&mut self, address: &str) -> Result<(), storage::Error>;
}

decl_storage_trait!(
    DnsServerStorage,
    DnsServerStorageRead,
    DnsServerStorageWrite
);
