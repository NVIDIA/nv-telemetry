// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Shared machinery for the model's sorted, duplicate-free collections.

use std::cmp::Ordering;

/// Sorts `items` and returns the first of an adjacent equal pair.
///
/// Detection uses the same comparator as the sort, so a collection cannot
/// order itself one way and check itself another.
pub(super) fn sort_and_find_duplicate<T>(
    items: &mut [T],
    compare: impl Fn(&T, &T) -> Ordering,
) -> Option<&T> {
    items.sort_unstable_by(&compare);
    items
        .windows(2)
        .find(|pair| compare(&pair[0], &pair[1]) == Ordering::Equal)
        .map(|pair| &pair[0])
}

/// Implements the read surface of a key-sorted, duplicate-free collection.
///
/// The wrapped slice is sorted by `$key`, so lookup is a binary search. Each
/// collection keeps its own `new`, since what makes an entry invalid beyond a
/// repeated key differs; everything below is identical once that holds.
///
/// The entries live in an [`Arc`](std::sync::Arc) because these are attached
/// to rows in bulk: one projection reattaches the same labels to every sample
/// it emits, so cloning has to be a refcount bump rather than a copy. An
/// empty collection is shared from a single allocation for the same reason,
/// since every row constructor starts from one.
///
/// `$entries` names the private type holding those entries, which exists so
/// the cached fingerprint sits inside the shared allocation rather than beside
/// it. A copy per clone would be recomputed by every row.
///
/// `$key` names the string-backed key field of `$item`, whose other field is
/// a `value` of type `$value`.
///
/// `$error` is what `new` rejects with, which also makes the collection a
/// `TryFrom<Vec<_>>`. Decoding is declared in terms of that conversion, so a
/// decoded collection is admitted by the constructor a caller uses rather
/// than by a second copy of the rule.
macro_rules! sorted_collection {
    (
        $collection:ident,
        $entries:ident,
        $item:ty,
        $key:ident,
        $value:ty,
        $error:ty
    ) => {
        #[derive(Debug, Default)]
        struct $entries {
            items: Box<[$item]>,
            fingerprint: std::sync::OnceLock<u64>,
        }

        impl $collection {
            /// Orders one entry against a bare key.
            ///
            /// Every ordering decision this collection makes goes through
            /// here: the sort and duplicate check in
            /// [`sorted_unique`](Self::sorted_unique) and the binary search in
            /// [`get`](Self::get). A key type that grew its own `Ord` would
            /// otherwise leave the search looking for entries where the sort
            /// did not put them.
            fn compare_key(entry: &$item, $key: &str) -> std::cmp::Ordering {
                entry.$key.as_str().cmp($key)
            }

            fn compare_entries(left: &$item, right: &$item) -> std::cmp::Ordering {
                Self::compare_key(left, right.$key.as_str())
            }

            /// Puts items in key order, rejecting a repeated key through
            /// `duplicate`.
            ///
            /// Each collection names its own error for that, which is the
            /// only part of the check that differs.
            fn sorted_unique(
                mut items: Vec<$item>,
                duplicate: impl FnOnce(&$item) -> $error,
            ) -> Result<Vec<$item>, $error> {
                match crate::model::collection::sort_and_find_duplicate(
                    &mut items,
                    Self::compare_entries,
                ) {
                    Some(repeated) => Err(duplicate(repeated)),
                    None => Ok(items),
                }
            }

            /// Takes ownership of items already sorted and validated by
            /// [`new`](Self::new).
            fn from_sorted(items: Vec<$item>) -> Self {
                Self(std::sync::Arc::new($entries {
                    items: items.into_boxed_slice(),
                    fingerprint: std::sync::OnceLock::new(),
                }))
            }

            pub fn empty() -> Self {
                static EMPTY: std::sync::OnceLock<std::sync::Arc<$entries>> =
                    std::sync::OnceLock::new();
                Self(std::sync::Arc::clone(EMPTY.get_or_init(Default::default)))
            }

            pub fn as_slice(&self) -> &[$item] {
                &self.0.items
            }

            pub fn iter(&self) -> std::slice::Iter<'_, $item> {
                self.as_slice().iter()
            }

            pub fn get(&self, $key: &str) -> Option<&$value> {
                let items = self.as_slice();
                items
                    .binary_search_by(|entry| Self::compare_key(entry, $key))
                    .ok()
                    .map(|index| &items[index].value)
            }

            pub fn len(&self) -> usize {
                self.as_slice().len()
            }

            pub fn is_empty(&self) -> bool {
                self.as_slice().is_empty()
            }

            /// Returns a digest of the entries, computing it once per
            /// allocation.
            ///
            /// Hashing a batch would otherwise walk every entry of every row,
            /// which is the bulk of the work in content-addressing a
            /// snapshot. The digest is derived from the entries alone, so
            /// equal collections produce equal digests and hashing stays
            /// consistent with equality.
            ///
            /// It is seeded per process rather than fixed. A caller's hasher
            /// only sees the digest, so a fixed seed would let an endpoint
            /// that chooses its own keys precompute colliding entries offline
            /// and undo the caller's seed. The digest is never serialized and
            /// only has to hold within one process.
            fn fingerprint(&self) -> u64 {
                *self.0.fingerprint.get_or_init(|| {
                    use std::hash::BuildHasher as _;

                    static SEED: std::sync::OnceLock<std::collections::hash_map::RandomState> =
                        std::sync::OnceLock::new();
                    SEED.get_or_init(std::collections::hash_map::RandomState::new)
                        .hash_one(self.as_slice())
                })
            }
        }

        impl std::hash::Hash for $collection {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                state.write_u64(self.fingerprint());
            }
        }

        impl PartialEq for $collection {
            fn eq(&self, other: &Self) -> bool {
                self.as_slice() == other.as_slice()
            }
        }

        impl Eq for $collection {}

        impl Default for $collection {
            fn default() -> Self {
                Self::empty()
            }
        }

        impl std::fmt::Debug for $collection {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .debug_tuple(stringify!($collection))
                    .field(&self.as_slice())
                    .finish()
            }
        }

        impl TryFrom<Vec<$item>> for $collection {
            type Error = $error;

            fn try_from(items: Vec<$item>) -> Result<Self, Self::Error> {
                Self::new(items)
            }
        }

        impl<'a> IntoIterator for &'a $collection {
            type Item = &'a $item;
            type IntoIter = std::slice::Iter<'a, $item>;

            fn into_iter(self) -> Self::IntoIter {
                self.iter()
            }
        }

        #[cfg(feature = "serde")]
        impl serde::Serialize for $collection {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                self.as_slice().serialize(serializer)
            }
        }
    };
}

pub(super) use sorted_collection;
