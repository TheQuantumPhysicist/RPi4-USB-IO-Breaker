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

use std::fmt::Debug;

use iced::{
    widget::{column, container, Text},
    Command, Element, Length,
};
use iced_aw::{tab_bar::TabLabel, Grid};

use crate::main_window::NodeState;

use super::{Tab, TabsMessage};

#[derive(Debug, Clone)]
pub enum NetworkMessage {}

pub struct NetworkTab {}

impl NetworkTab {
    pub fn new() -> Self {
        NetworkTab {}
    }

    pub fn update(&mut self, message: NetworkMessage) -> Command<NetworkMessage> {
        match message {}
    }
}

impl Tab for NetworkTab {
    type Message = TabsMessage;

    fn title(&self) -> String {
        String::from("Network")
    }

    fn tab_label(&self) -> TabLabel {
        TabLabel::IconText(iced_aw::Icon::Wifi.into(), self.title())
    }

    fn content(&self, node_state: &NodeState) -> Element<Self::Message> {
        let header = |text: &'static str| container(Text::new(text)).padding(5);
        let field = |text: String| container(Text::new(text)).padding(5);
        let mut peers = Grid::with_columns(5)
            .push(header("id"))
            .push(header("Socket"))
            .push(header("Inbound"))
            .push(header("User agent"))
            .push(header("Version"));
        for (peer_id, peer) in node_state.connected_peers.iter() {
            let inbound_str = if peer.inbound { "Inbound" } else { "Outbound" };
            peers = peers
                .push(field(peer_id.to_string()))
                .push(field(peer.address.clone()))
                .push(field(inbound_str.to_string()))
                .push(field(peer.user_agent.to_string()))
                .push(field(peer.version.to_string()));
        }

        column![peers]
            .padding(10)
            .spacing(15)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }
}
