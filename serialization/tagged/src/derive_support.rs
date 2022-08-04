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
//
// Author(s): L. Kuklinek

//! Implementation tools used by code generated by derive macros
#![doc(hidden)]

pub use static_assertions as sa;

use crate::Input;

/// Input byte stream with a one-byte lookahead
pub struct Peekable<'a, I> {
    init: Option<u8>,
    inner: &'a mut I,
}

impl<'a, I: Input> Peekable<'a, I> {
    /// New peekable input
    pub fn new(inner: &'a mut I) -> Self {
        Self { init: None, inner }
    }

    /// Peek the next byte
    pub fn peek(&mut self) -> Result<u8, crate::Error> {
        self.init
            .map_or_else(|| self.inner.read_byte().map(|b| *self.init.insert(b)), Ok)
    }
}

impl<I> Peekable<'_, I> {
    pub fn assert_tag_consumed(&self) {
        assert!(self.init.is_none());
    }
}

impl<I> Drop for Peekable<'_, I> {
    fn drop(&mut self) {
        self.assert_tag_consumed();
    }
}

impl<I: Input> Input for Peekable<'_, I> {
    fn remaining_len(&mut self) -> Result<Option<usize>, crate::Error> {
        self.inner.remaining_len().map(|x| x.map(|l| l + self.init.iter().len()))
    }

    fn read(&mut self, into: &mut [u8]) -> Result<(), crate::Error> {
        match self.init.take() {
            None => self.inner.read(into),
            Some(b) => {
                if let Some((first, rest)) = into.split_first_mut() {
                    *first = b;
                    self.inner.read(rest)?;
                }
                Ok(())
            }
        }
    }

    fn read_byte(&mut self) -> Result<u8, crate::Error> {
        self.init.take().map_or_else(|| self.inner.read_byte(), Ok)
    }

    fn ascend_ref(&mut self) {
        self.inner.ascend_ref()
    }

    fn descend_ref(&mut self) -> Result<(), crate::Error> {
        self.inner.descend_ref()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn peek_twice(encoded_orig in prop::collection::vec(any::<u8>(), 5..100)) {
            let mut encoded = &encoded_orig[..];
            let orig_len = encoded.len();

            let mut input = Peekable::new(&mut encoded);
            assert_eq!(input.remaining_len(), Ok(Some(orig_len)));
            assert_eq!(input.inner.len(), orig_len);

            let byte = input.peek().expect("first empty");
            assert_eq!(input.remaining_len(), Ok(Some(orig_len)));
            assert_eq!(input.inner.len(), orig_len - 1);
            assert_eq!(byte, encoded_orig[0]);

            let byte = input.peek().expect("second empty");
            assert_eq!(input.remaining_len(), Ok(Some(orig_len)));
            assert_eq!(input.inner.len(), orig_len - 1);
            assert_eq!(byte, encoded_orig[0]);

            let byte = input.read_byte().expect("third empty");
            assert_eq!(input.remaining_len(), Ok(Some(orig_len - 1)));
            assert_eq!(input.inner.len(), orig_len - 1);
            assert_eq!(byte, encoded_orig[0]);

            let byte = input.read_byte().expect("fourth empty");
            assert_eq!(input.remaining_len(), Ok(Some(orig_len - 2)));
            assert_eq!(input.inner.len(), orig_len - 2);
            assert_eq!(byte, encoded_orig[1]);

            let byte = input.peek().expect("fourth empty");
            assert_eq!(input.remaining_len(), Ok(Some(orig_len - 2)));
            assert_eq!(input.inner.len(), orig_len - 3);
            assert_eq!(byte, encoded_orig[2]);

            let byte = input.read_byte().expect("fourth empty");
            assert_eq!(input.remaining_len(), Ok(Some(orig_len - 3)));
            assert_eq!(input.inner.len(), orig_len - 3);
            assert_eq!(byte, encoded_orig[2]);
        }

        #[test]
        fn copy(data: Vec<u8>) {
            let mut source = &data[..];
            let mut input = Peekable::new(&mut source);
            let mut target = vec![0; input.remaining_len().unwrap().unwrap_or_default()];
            input.read(&mut target).expect("bad size");
            assert_eq!(data, target);
        }
    }
}
