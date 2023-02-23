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

use crate::primitives::merkle::tree::Node;

#[derive(Debug, Clone, Eq)]
pub struct NodesWithAbsOrder<'a> {
    node: Node<'a>,
}

impl<'a> NodesWithAbsOrder<'a> {
    pub fn get(&self) -> &Node<'a> {
        &self.node
    }
}

impl<'a> From<Node<'a>> for NodesWithAbsOrder<'a> {
    fn from(node: Node<'a>) -> Self {
        Self { node }
    }
}

impl<'a> From<NodesWithAbsOrder<'a>> for Node<'a> {
    fn from(node: NodesWithAbsOrder<'a>) -> Self {
        node.node
    }
}

impl Ord for NodesWithAbsOrder<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.node.abs_index().cmp(&other.node.abs_index())
    }
}

impl PartialOrd for NodesWithAbsOrder<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.node.abs_index().partial_cmp(&other.node.abs_index())
    }
}

impl PartialEq for NodesWithAbsOrder<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.node.abs_index() == other.node.abs_index()
    }
}
