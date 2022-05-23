#[cfg(any(test, feature = "mock"))]
pub mod mock;

use std::sync::Arc;

use common::{
    chain::block::Block,
    primitives::{BlockHeight, Id},
};

use crate::{detail::BlockSource, ConsensusError, ConsensusEvent};

pub trait ConsensusInterface: Send {
    fn subscribe_to_events(&mut self, handler: Arc<dyn Fn(ConsensusEvent) + Send + Sync>);
    fn process_block(&mut self, block: Block, source: BlockSource) -> Result<(), ConsensusError>;
    fn get_best_block_id(&self) -> Result<Id<Block>, ConsensusError>;
    fn is_block_in_main_chain(&self, block_id: &Id<Block>) -> Result<bool, ConsensusError>;
    fn get_block_height_in_main_chain(
        &self,
        block_id: &Id<Block>,
    ) -> Result<Option<BlockHeight>, ConsensusError>;
    fn get_block_id_from_height(
        &self,
        height: &BlockHeight,
    ) -> Result<Option<Id<Block>>, ConsensusError>;
    fn get_block(&self, block_id: Id<Block>) -> Result<Option<Block>, ConsensusError>;
}