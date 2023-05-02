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

use std::{path::PathBuf, str::FromStr, sync::Arc};

use clap::Parser;
use common::primitives::{BlockHeight, H256};
use serialization::hex::HexEncode;
use wallet::Wallet;
use wallet_controller::{PeerId, RpcController};

use crate::errors::WalletCliError;

#[derive(Debug, Parser)]
#[clap(rename_all = "lower")]
pub enum WalletCommand {
    /// Create new wallet
    NewWallet {
        /// File path
        wallet_path: PathBuf,

        // Mnemonic
        mnemonic: Option<String>,
    },

    /// Open exiting wallet
    OpenWallet {
        /// File path
        wallet_path: PathBuf,
    },

    /// Close wallet file
    CloseWallet,

    /// Returns the node chainstate
    ChainstateInfo,

    /// Returns the current best block hash
    BestBlock,

    /// Returns the current best block height
    BestBlockHeight,

    /// Get a block hash at height
    BlockHash {
        /// Block height
        height: BlockHeight,
    },

    /// Get a block by its hash
    GetBlock {
        /// Block hash
        hash: String,
    },

    /// Submit a block to be included in the chain
    ///
    /// More information about block submits.
    /// More information about block submits.
    ///
    /// Even more information about block submits.
    /// Even more information about block submits.
    /// Even more information about block submits.
    /// Even more information about block submits.
    SubmitBlock {
        /// Hex encoded block
        block: String,
    },

    /// Submits a transaction to mempool, and if it is valid, broadcasts it to the network
    SubmitTransaction {
        /// Hex encoded transaction
        transaction: String,
    },

    /// Rescan
    Rescan,

    /// Node version
    NodeVersion,

    /// Node shutdown
    NodeShutdown,

    /// Connect to the remote peer
    Connect { address: String },

    /// Disconnected the remote peer
    Disconnect { peer_id: PeerId },

    /// Get connected peer count
    PeerCount,

    /// Get connected peers
    ConnectedPeers,

    /// Add reserved peer
    AddReservedPeer { address: String },

    /// Remove reserved peer
    RemoveReservedPeer { address: String },

    /// Quit the REPL
    Exit,

    /// Print history
    History,

    /// Clear screen
    #[clap(name = "clear")]
    ClearScreen,

    /// Clear history
    ClearHistory,
}

#[derive(Debug)]
pub enum ConsoleCommand {
    Print(String),
    ClearScreen,
    PrintHistory,
    ClearHistory,
    Exit,
}

pub async fn handle_wallet_command(
    controller: &mut RpcController,
    command: WalletCommand,
) -> Result<ConsoleCommand, WalletCliError> {
    match command {
        WalletCommand::NewWallet {
            wallet_path,
            mnemonic,
        } => {
            utils::ensure!(
                controller.wallets_len() == 0,
                WalletCliError::WalletFileAlreadyOpen
            );
            utils::ensure!(
                !wallet_path.exists(),
                WalletCliError::FileAlreadyExists(wallet_path.clone())
            );

            // TODO: Support other languages
            let language = wallet::wallet::Language::English;
            let need_mnemonic_backup = mnemonic.is_none();
            let mnemonic = match &mnemonic {
                Some(mnemonic) => wallet_controller::mnemonic::parse_mnemonic(language, mnemonic)
                    .map_err(WalletCliError::InvalidMnemonic)?,
                None => wallet_controller::mnemonic::generate_new_mnemonic(language),
            };

            let db = wallet::wallet::open_or_create_wallet_file(&wallet_path)
                .map_err(WalletCliError::WalletError)?;
            let wallet = Wallet::new_wallet(
                Arc::clone(controller.chain_config()),
                db,
                &mnemonic.to_string(),
                None,
            )
            .map_err(WalletCliError::WalletError)?;
            controller.add_wallet(wallet);

            let msg = if need_mnemonic_backup {
                format!(
                    "New wallet created successfully\nYour mnemonic: {}\nPlease write it somewhere safe to be able to restore your wallet."
                , mnemonic)
            } else {
                "New wallet created successfully".to_owned()
            };
            Ok(ConsoleCommand::Print(msg))
        }

        WalletCommand::OpenWallet { wallet_path } => {
            utils::ensure!(
                controller.wallets_len() == 0,
                WalletCliError::WalletFileAlreadyOpen
            );
            utils::ensure!(
                wallet_path.exists(),
                WalletCliError::FileDoesNotExist(wallet_path.clone())
            );

            let db = wallet::wallet::open_or_create_wallet_file(&wallet_path)
                .map_err(WalletCliError::WalletError)?;
            let wallet = Wallet::load_wallet(Arc::clone(controller.chain_config()), db)
                .map_err(WalletCliError::WalletError)?;
            controller.add_wallet(wallet);

            Ok(ConsoleCommand::Print(
                "Wallet loaded successfully".to_owned(),
            ))
        }

        WalletCommand::CloseWallet => {
            utils::ensure!(
                controller.wallets_len() != 0,
                WalletCliError::NoWalletIsOpened
            );
            controller.del_wallet(0);
            Ok(ConsoleCommand::Print("Success".to_owned()))
        }

        WalletCommand::ChainstateInfo => {
            let info = controller.chainstate_info().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(format!("{info:?}")))
        }

        WalletCommand::BestBlock => {
            let id = controller.get_best_block_id().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(id.hex_encode()))
        }

        WalletCommand::BestBlockHeight => {
            let height =
                controller.get_best_block_height().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(height.to_string()))
        }

        WalletCommand::BlockHash { height } => {
            let hash = controller
                .get_block_id_at_height(height)
                .await
                .map_err(WalletCliError::Controller)?;
            match hash {
                Some(id) => Ok(ConsoleCommand::Print(id.hex_encode())),
                None => Ok(ConsoleCommand::Print("Not found".to_owned())),
            }
        }

        WalletCommand::GetBlock { hash } => {
            let hash =
                H256::from_str(&hash).map_err(|e| WalletCliError::InvalidInput(e.to_string()))?;
            let hash =
                controller.get_block(hash.into()).await.map_err(WalletCliError::Controller)?;
            match hash {
                Some(block) => Ok(ConsoleCommand::Print(block.hex_encode())),
                None => Ok(ConsoleCommand::Print("Not found".to_owned())),
            }
        }

        WalletCommand::SubmitBlock { block } => {
            controller.submit_block(block).await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(
                "The block was submitted successfully".to_owned(),
            ))
        }

        WalletCommand::SubmitTransaction { transaction } => {
            controller
                .submit_transaction(transaction)
                .await
                .map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(
                "The transaction was submitted successfully".to_owned(),
            ))
        }

        WalletCommand::Rescan => Ok(ConsoleCommand::Print("Not implemented".to_owned())),

        WalletCommand::NodeVersion => {
            let version = controller.node_version().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(version))
        }

        WalletCommand::NodeShutdown => {
            controller.node_shutdown().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print("Success".to_owned()))
        }

        WalletCommand::Connect { address } => {
            controller.p2p_connect(address).await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print("Success".to_owned()))
        }
        WalletCommand::Disconnect { peer_id } => {
            controller.p2p_disconnect(peer_id).await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print("Success".to_owned()))
        }
        WalletCommand::PeerCount => {
            let peer_count =
                controller.p2p_get_peer_count().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(peer_count.to_string()))
        }
        WalletCommand::ConnectedPeers => {
            let peers =
                controller.p2p_get_connected_peers().await.map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print(format!("{peers:?}")))
        }
        WalletCommand::AddReservedPeer { address } => {
            controller
                .p2p_add_reserved_node(address)
                .await
                .map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print("Success".to_owned()))
        }
        WalletCommand::RemoveReservedPeer { address } => {
            controller
                .p2p_remove_reserved_node(address)
                .await
                .map_err(WalletCliError::Controller)?;
            Ok(ConsoleCommand::Print("Success".to_owned()))
        }

        WalletCommand::Exit => Ok(ConsoleCommand::Exit),
        WalletCommand::History => Ok(ConsoleCommand::PrintHistory),
        WalletCommand::ClearScreen => Ok(ConsoleCommand::ClearScreen),
        WalletCommand::ClearHistory => Ok(ConsoleCommand::ClearHistory),
    }
}
