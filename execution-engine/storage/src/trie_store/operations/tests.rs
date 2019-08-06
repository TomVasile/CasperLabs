use failure;
use lmdb::DatabaseFlags;
use tempfile::{tempdir, TempDir};

use common::bytesrepr::{self, FromBytes, ToBytes};
use shared::newtypes::Blake2bHash;

use error;
use trie::{Pointer, Trie};
use trie_store::in_memory::{self, InMemoryEnvironment, InMemoryTrieStore};
use trie_store::lmdb::{LmdbEnvironment, LmdbTrieStore};
use trie_store::operations::{read, write, ReadResult, WriteResult};
use trie_store::{Readable, Transaction, TransactionSource, TrieStore};
use TEST_MAP_SIZE;

const TEST_KEY_LENGTH: usize = 7;

/// A short key type for tests.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct TestKey([u8; TEST_KEY_LENGTH]);

impl ToBytes for TestKey {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        Ok(self.0.to_vec())
    }
}

impl FromBytes for TestKey {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (key, rem) = bytes.split_at(TEST_KEY_LENGTH);
        let mut ret = [0u8; TEST_KEY_LENGTH];
        ret.copy_from_slice(key);
        Ok((TestKey(ret), rem))
    }
}

const TEST_VAL_LENGTH: usize = 6;

/// A short value type for tests.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct TestValue([u8; TEST_VAL_LENGTH]);

impl ToBytes for TestValue {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        Ok(self.0.to_vec())
    }
}

impl FromBytes for TestValue {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (key, rem) = bytes.split_at(TEST_VAL_LENGTH);
        let mut ret = [0u8; TEST_VAL_LENGTH];
        ret.copy_from_slice(key);
        Ok((TestValue(ret), rem))
    }
}

type TestTrie = Trie<TestKey, TestValue>;

/// A pairing of a trie element and its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HashedTestTrie {
    hash: Blake2bHash,
    trie: TestTrie,
}

impl HashedTestTrie {
    pub fn new(trie: TestTrie) -> Result<Self, bytesrepr::Error> {
        let trie_bytes = trie.to_bytes()?;
        let hash = Blake2bHash::new(&trie_bytes);
        Ok(HashedTestTrie { hash, trie })
    }
}

const TEST_LEAVES_LENGTH: usize = 6;

/// Keys have been chosen deliberately and the `create_` functions below depend
/// on these exact definitions.  Values are arbitrary.
const TEST_LEAVES: [TestTrie; TEST_LEAVES_LENGTH] = [
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"value0"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 1]),
        value: TestValue(*b"value1"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 2, 0, 0, 0]),
        value: TestValue(*b"value2"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 255, 0]),
        value: TestValue(*b"value3"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 1, 0, 0, 0, 0, 0]),
        value: TestValue(*b"value4"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 2, 0, 0, 0, 0]),
        value: TestValue(*b"value5"),
    },
];

const TEST_LEAVES_UPDATED: [TestTrie; TEST_LEAVES_LENGTH] = [
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueA"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 1]),
        value: TestValue(*b"valueB"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 2, 0, 0, 0]),
        value: TestValue(*b"valueC"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 255, 0]),
        value: TestValue(*b"valueD"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 1, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueE"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 2, 0, 0, 0, 0]),
        value: TestValue(*b"valueF"),
    },
];

const TEST_LEAVES_NON_COLLIDING: [TestTrie; TEST_LEAVES_LENGTH] = [
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueA"),
    },
    Trie::Leaf {
        key: TestKey([1u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueB"),
    },
    Trie::Leaf {
        key: TestKey([2u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueC"),
    },
    Trie::Leaf {
        key: TestKey([3u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueD"),
    },
    Trie::Leaf {
        key: TestKey([4u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueE"),
    },
    Trie::Leaf {
        key: TestKey([5u8, 0, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueF"),
    },
];

const TEST_LEAVES_ADJACENTS: [TestTrie; TEST_LEAVES_LENGTH] = [
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 2]),
        value: TestValue(*b"valueA"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 0, 3]),
        value: TestValue(*b"valueB"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 3, 0, 0, 0]),
        value: TestValue(*b"valueC"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 0, 0, 0, 1, 0]),
        value: TestValue(*b"valueD"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 2, 0, 0, 0, 0, 0]),
        value: TestValue(*b"valueE"),
    },
    Trie::Leaf {
        key: TestKey([0u8, 0, 3, 0, 0, 0, 0]),
        value: TestValue(*b"valueF"),
    },
];

type TestTrieGenerator = fn() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error>;

const TEST_TRIE_GENERATORS_LENGTH: usize = 7;

const TEST_TRIE_GENERATORS: [TestTrieGenerator; TEST_TRIE_GENERATORS_LENGTH] = [
    create_0_leaf_trie,
    create_1_leaf_trie,
    create_2_leaf_trie,
    create_3_leaf_trie,
    create_4_leaf_trie,
    create_5_leaf_trie,
    create_6_leaf_trie,
];

fn hash_test_tries(tries: &[TestTrie]) -> Result<Vec<HashedTestTrie>, bytesrepr::Error> {
    tries
        .iter()
        .map(|trie| HashedTestTrie::new(trie.to_owned()))
        .collect()
}

fn create_0_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let root = HashedTestTrie::new(Trie::node(&[]))?;

    let root_hash: Blake2bHash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn create_1_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let leaves = hash_test_tries(&TEST_LEAVES[..1])?;

    let root = HashedTestTrie::new(Trie::node(&[(0, Pointer::LeafPointer(leaves[0].hash))]))?;

    let root_hash: Blake2bHash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(leaves);
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn create_2_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let leaves = hash_test_tries(&TEST_LEAVES[..2])?;

    let node = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::LeafPointer(leaves[0].hash)),
        (1, Pointer::LeafPointer(leaves[1].hash)),
    ]))?;

    let ext = HashedTestTrie::new(Trie::extension(
        vec![0u8, 0, 0, 0, 0],
        Pointer::NodePointer(node.hash),
    ))?;

    let root = HashedTestTrie::new(Trie::node(&[(0, Pointer::NodePointer(ext.hash))]))?;

    let root_hash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root, ext, node];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(leaves);
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn create_3_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let leaves = hash_test_tries(&TEST_LEAVES[..3])?;

    let node_1 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::LeafPointer(leaves[0].hash)),
        (1, Pointer::LeafPointer(leaves[1].hash)),
    ]))?;

    let ext_1 = HashedTestTrie::new(Trie::extension(
        vec![0u8, 0],
        Pointer::NodePointer(node_1.hash),
    ))?;

    let node_2 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(ext_1.hash)),
        (2, Pointer::LeafPointer(leaves[2].hash)),
    ]))?;

    let ext_2 = HashedTestTrie::new(Trie::extension(
        vec![0u8, 0],
        Pointer::NodePointer(node_2.hash),
    ))?;

    let root = HashedTestTrie::new(Trie::node(&[(0, Pointer::NodePointer(ext_2.hash))]))?;

    let root_hash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root, ext_2, node_2, ext_1, node_1];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(leaves);
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn create_4_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let leaves = hash_test_tries(&TEST_LEAVES[..4])?;

    let node_1 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::LeafPointer(leaves[0].hash)),
        (1, Pointer::LeafPointer(leaves[1].hash)),
    ]))?;

    let node_2 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(node_1.hash)),
        (255, Pointer::LeafPointer(leaves[3].hash)),
    ]))?;

    let ext_1 = HashedTestTrie::new(Trie::extension(
        vec![0u8],
        Pointer::NodePointer(node_2.hash),
    ))?;

    let node_3 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(ext_1.hash)),
        (2, Pointer::LeafPointer(leaves[2].hash)),
    ]))?;

    let ext_2 = HashedTestTrie::new(Trie::extension(
        vec![0u8, 0],
        Pointer::NodePointer(node_3.hash),
    ))?;

    let root = HashedTestTrie::new(Trie::node(&[(0, Pointer::NodePointer(ext_2.hash))]))?;

    let root_hash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root, ext_2, node_3, ext_1, node_2, node_1];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(leaves);
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn create_5_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let leaves = hash_test_tries(&TEST_LEAVES[..5])?;

    let node_1 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::LeafPointer(leaves[0].hash)),
        (1, Pointer::LeafPointer(leaves[1].hash)),
    ]))?;

    let node_2 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(node_1.hash)),
        (255, Pointer::LeafPointer(leaves[3].hash)),
    ]))?;

    let ext_1 = HashedTestTrie::new(Trie::extension(
        vec![0u8],
        Pointer::NodePointer(node_2.hash),
    ))?;

    let node_3 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(ext_1.hash)),
        (2, Pointer::LeafPointer(leaves[2].hash)),
    ]))?;

    let ext_2 = HashedTestTrie::new(Trie::extension(
        vec![0u8],
        Pointer::NodePointer(node_3.hash),
    ))?;

    let node_4 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(ext_2.hash)),
        (1, Pointer::LeafPointer(leaves[4].hash)),
    ]))?;

    let root = HashedTestTrie::new(Trie::node(&[(0, Pointer::NodePointer(node_4.hash))]))?;

    let root_hash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root, node_4, ext_2, node_3, ext_1, node_2, node_1];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(leaves);
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn create_6_leaf_trie() -> Result<(Blake2bHash, Vec<HashedTestTrie>), bytesrepr::Error> {
    let leaves = hash_test_tries(&TEST_LEAVES)?;

    let node_1 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::LeafPointer(leaves[0].hash)),
        (1, Pointer::LeafPointer(leaves[1].hash)),
    ]))?;

    let node_2 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(node_1.hash)),
        (255, Pointer::LeafPointer(leaves[3].hash)),
    ]))?;

    let ext = HashedTestTrie::new(Trie::extension(
        vec![0u8],
        Pointer::NodePointer(node_2.hash),
    ))?;

    let node_3 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(ext.hash)),
        (2, Pointer::LeafPointer(leaves[2].hash)),
    ]))?;

    let node_4 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(node_3.hash)),
        (2, Pointer::LeafPointer(leaves[5].hash)),
    ]))?;

    let node_5 = HashedTestTrie::new(Trie::node(&[
        (0, Pointer::NodePointer(node_4.hash)),
        (1, Pointer::LeafPointer(leaves[4].hash)),
    ]))?;

    let root = HashedTestTrie::new(Trie::node(&[(0, Pointer::NodePointer(node_5.hash))]))?;

    let root_hash = root.hash;

    let parents: Vec<HashedTestTrie> = vec![root, node_5, node_4, node_3, ext, node_2, node_1];

    let tries: Vec<HashedTestTrie> = {
        let mut ret = Vec::new();
        ret.extend(leaves);
        ret.extend(parents);
        ret
    };

    Ok((root_hash, tries))
}

fn put_tries<'a, R, S, E>(environment: &'a R, store: &S, tries: &[HashedTestTrie]) -> Result<(), E>
where
    R: TransactionSource<'a, Handle = S::Handle>,
    S: TrieStore<TestKey, TestValue>,
    S::Error: From<R::Error>,
    E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
{
    if tries.is_empty() {
        return Ok(());
    }
    let mut txn = environment.create_read_write_txn()?;
    for HashedTestTrie { hash, trie } in tries.iter() {
        store.put(&mut txn, hash, trie)?;
    }
    txn.commit()?;
    Ok(())
}

// A context for holding lmdb-based test resources
struct LmdbTestContext {
    _temp_dir: TempDir,
    environment: LmdbEnvironment,
    store: LmdbTrieStore,
}

impl LmdbTestContext {
    fn new(tries: &[HashedTestTrie]) -> Result<Self, failure::Error> {
        let _temp_dir = tempdir()?;
        let environment = LmdbEnvironment::new(&_temp_dir.path().to_path_buf(), *TEST_MAP_SIZE)?;
        let store = LmdbTrieStore::new(&environment, None, DatabaseFlags::empty())?;
        put_tries::<_, _, error::Error>(&environment, &store, tries)?;
        Ok(LmdbTestContext {
            _temp_dir,
            environment,
            store,
        })
    }

    fn update(&self, tries: &[HashedTestTrie]) -> Result<(), failure::Error> {
        put_tries::<_, _, error::Error>(&self.environment, &self.store, tries)?;
        Ok(())
    }
}

// A context for holding in-memory test resources
struct InMemoryTestContext {
    environment: InMemoryEnvironment,
    store: InMemoryTrieStore,
}

impl InMemoryTestContext {
    fn new(tries: &[HashedTestTrie]) -> Result<Self, failure::Error> {
        let environment = InMemoryEnvironment::new();
        let store = InMemoryTrieStore::new(&environment);
        put_tries::<_, _, in_memory::Error>(&environment, &store, tries)?;
        Ok(InMemoryTestContext { environment, store })
    }

    fn update(&self, tries: &[HashedTestTrie]) -> Result<(), failure::Error> {
        put_tries::<_, _, in_memory::Error>(&self.environment, &self.store, tries)?;
        Ok(())
    }
}

fn check_leaves_exist<T, S, E>(
    txn: &T,
    store: &S,
    root: &Blake2bHash,
    leaves: &[TestTrie],
) -> Result<Vec<bool>, E>
where
    T: Readable<Handle = S::Handle>,
    S: TrieStore<TestKey, TestValue>,
    S::Error: From<T::Error>,
    E: From<S::Error> + From<common::bytesrepr::Error>,
{
    let mut ret = Vec::new();

    for leaf in leaves {
        if let Trie::Leaf { key, value } = leaf {
            let maybe_value: ReadResult<TestValue> = read::<_, _, _, _, E>(txn, store, root, key)?;
            ret.push(ReadResult::Found(*value) == maybe_value)
        } else {
            panic!("leaves should only contain leaves")
        }
    }
    Ok(ret)
}

fn check_leaves<'a, R, S, E>(
    environment: &'a R,
    store: &S,
    root: &Blake2bHash,
    present: &[TestTrie],
    absent: &[TestTrie],
) -> Result<(), E>
where
    R: TransactionSource<'a, Handle = S::Handle>,
    S: TrieStore<TestKey, TestValue>,
    S::Error: From<R::Error>,
    E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
{
    let txn: R::ReadTransaction = environment.create_read_txn()?;

    assert!(check_leaves_exist::<_, _, E>(&txn, store, root, present)?
        .iter()
        .all(|b| *b));
    assert!(check_leaves_exist::<_, _, E>(&txn, store, root, absent)?
        .iter()
        .all(|b| !*b));
    txn.commit()?;
    Ok(())
}

mod read {
    //! This module contains tests for [`StateReader::read`].
    //!
    //! Our primary goal here is to test this functionality in isolation.
    //! Therefore, we manually construct test tries from a well-known set of
    //! leaves called [`TEST_LEAVES`](super::TEST_LEAVES), each of which represents a value we are
    //! trying to store in the trie at a given key.
    //!
    //! We use two strategies for testing.  See the [`partial_tries`] and
    //! [`full_tries`] modules for more info.

    use super::*;
    use error;
    use trie_store::in_memory;

    mod partial_tries {
        //! Here we construct 6 separate "partial" tries, increasing in size
        //! from 0 to 5 leaves.  Each of these tries contains no past history,
        //! only a single a root to read from.  The tests check that we can read
        //! only the expected set of leaves from the trie from this single root.

        use super::*;

        #[test]
        fn lmdb_reads_from_n_leaf_partial_trie_had_expected_results() {
            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                let context = LmdbTestContext::new(&tries).unwrap();
                let test_leaves = TEST_LEAVES;
                let (used, unused) = test_leaves.split_at(num_leaves);

                check_leaves::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &root_hash,
                    used,
                    unused,
                )
                .unwrap();
            }
        }

        #[test]
        fn in_memory_reads_from_n_leaf_partial_trie_had_expected_results() {
            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                let context = InMemoryTestContext::new(&tries).unwrap();
                let test_leaves = TEST_LEAVES;
                let (used, unused) = test_leaves.split_at(num_leaves);

                check_leaves::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &root_hash,
                    used,
                    unused,
                )
                .unwrap();
            }
        }
    }

    mod full_tries {
        //! Here we construct a series of 6 "full" tries, increasing in size
        //! from 0 to 5 leaves.  Each trie contains the history from preceding
        //! tries in this series, and past history can be read from the roots of
        //! each preceding trie.  The tests check that we can read only the
        //! expected set of leaves from the trie at the current root and all past
        //! roots.

        use super::*;

        #[test]
        fn lmdb_reads_from_n_leaf_full_trie_had_expected_results() {
            let context = LmdbTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (state_index, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                for (num_leaves, state) in states[..state_index].iter().enumerate() {
                    let test_leaves = TEST_LEAVES;
                    let (used, unused) = test_leaves.split_at(num_leaves);
                    check_leaves::<_, _, error::Error>(
                        &context.environment,
                        &context.store,
                        state,
                        used,
                        unused,
                    )
                    .unwrap();
                }
            }
        }

        #[test]
        fn in_memory_reads_from_n_leaf_full_trie_had_expected_results() {
            let context = InMemoryTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (state_index, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                for (num_leaves, state) in states[..state_index].iter().enumerate() {
                    let test_leaves = TEST_LEAVES;
                    let (used, unused) = test_leaves.split_at(num_leaves);
                    check_leaves::<_, _, in_memory::Error>(
                        &context.environment,
                        &context.store,
                        state,
                        used,
                        unused,
                    )
                    .unwrap();
                }
            }
        }
    }
}

mod scan {
    use shared::newtypes::Blake2bHash;

    use super::*;
    use error;
    use trie_store::in_memory;
    use trie_store::operations::{scan, TrieScan};

    fn check_scan<'a, R, S, E>(
        environment: &'a R,
        store: &S,
        root_hash: &Blake2bHash,
        key: &[u8],
    ) -> Result<(), E>
    where
        R: TransactionSource<'a, Handle = S::Handle>,
        S: TrieStore<TestKey, TestValue>,
        S::Error: From<R::Error> + std::fmt::Debug,
        E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
    {
        let txn: R::ReadTransaction = environment.create_read_txn()?;
        let root = store
            .get(&txn, &root_hash)?
            .expect("check_scan received an invalid root hash");
        let TrieScan { mut tip, parents } =
            scan::<TestKey, TestValue, R::ReadTransaction, S, E>(&txn, store, key, &root)?;

        for (index, parent) in parents.into_iter().rev() {
            let expected_tip_hash = {
                let tip_bytes = tip.to_bytes().unwrap();
                Blake2bHash::new(&tip_bytes)
            };
            match parent {
                Trie::Leaf { .. } => panic!("parents should not contain any leaves"),
                Trie::Node { pointer_block } => {
                    let pointer_tip_hash = pointer_block[index.into()].map(|ptr| *ptr.hash());
                    assert_eq!(Some(expected_tip_hash), pointer_tip_hash);
                    tip = Trie::Node { pointer_block };
                }
                Trie::Extension { affix, pointer } => {
                    let pointer_tip_hash = pointer.hash().to_owned();
                    assert_eq!(expected_tip_hash, pointer_tip_hash);
                    tip = Trie::Extension { affix, pointer };
                }
            }
        }
        assert_eq!(root, tip);
        txn.commit()?;
        Ok(())
    }

    mod partial_tries {
        use super::*;

        #[test]
        fn lmdb_scans_from_n_leaf_partial_trie_had_expected_results() {
            for generator in &TEST_TRIE_GENERATORS {
                let (root_hash, tries) = generator().unwrap();
                let context = LmdbTestContext::new(&tries).unwrap();

                for leaf in TEST_LEAVES.iter() {
                    let leaf_bytes = leaf.to_bytes().unwrap();
                    check_scan::<_, _, error::Error>(
                        &context.environment,
                        &context.store,
                        &root_hash,
                        &leaf_bytes,
                    )
                    .unwrap()
                }
            }
        }

        #[test]
        fn in_memory_scans_from_n_leaf_partial_trie_had_expected_results() {
            for generator in &TEST_TRIE_GENERATORS {
                let (root_hash, tries) = generator().unwrap();
                let context = InMemoryTestContext::new(&tries).unwrap();

                for leaf in TEST_LEAVES.iter() {
                    let leaf_bytes = leaf.to_bytes().unwrap();
                    check_scan::<_, _, in_memory::Error>(
                        &context.environment,
                        &context.store,
                        &root_hash,
                        &leaf_bytes,
                    )
                    .unwrap()
                }
            }
        }
    }

    mod full_tries {
        use super::*;

        #[test]
        fn lmdb_scans_from_n_leaf_full_trie_had_expected_results() {
            let context = LmdbTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (state_index, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                for state in &states[..state_index] {
                    for leaf in TEST_LEAVES.iter() {
                        let leaf_bytes = leaf.to_bytes().unwrap();
                        check_scan::<_, _, error::Error>(
                            &context.environment,
                            &context.store,
                            state,
                            &leaf_bytes,
                        )
                        .unwrap()
                    }
                }
            }
        }

        #[test]
        fn in_memory_scans_from_n_leaf_full_trie_had_expected_results() {
            let context = InMemoryTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (state_index, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                for state in &states[..state_index] {
                    for leaf in TEST_LEAVES.iter() {
                        let leaf_bytes = leaf.to_bytes().unwrap();
                        check_scan::<_, _, in_memory::Error>(
                            &context.environment,
                            &context.store,
                            state,
                            &leaf_bytes,
                        )
                        .unwrap()
                    }
                }
            }
        }
    }
}

mod write {
    use super::*;

    fn write_leaves<'a, R, S, E>(
        environment: &'a R,
        store: &S,
        root_hash: &Blake2bHash,
        leaves: &[TestTrie],
    ) -> Result<Vec<WriteResult>, E>
    where
        R: TransactionSource<'a, Handle = S::Handle>,
        S: TrieStore<TestKey, TestValue>,
        S::Error: From<R::Error>,
        E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
    {
        let mut results = Vec::new();
        if leaves.is_empty() {
            return Ok(results);
        }
        let mut root_hash = root_hash.to_owned();
        let mut txn = environment.create_read_write_txn()?;

        for leaf in leaves.iter() {
            if let Trie::Leaf { key, value } = leaf {
                let write_result = write::<_, _, _, _, E>(&mut txn, store, &root_hash, key, value)?;
                match write_result {
                    WriteResult::Written(hash) => {
                        root_hash = hash;
                    }
                    WriteResult::AlreadyExists => (),
                    WriteResult::RootNotFound => panic!("write_leaves given an invalid root"),
                };
                results.push(write_result);
            } else {
                panic!("leaves should contain only leaves");
            }
        }
        txn.commit()?;
        Ok(results)
    }

    mod empty_tries {
        use super::*;
        use std::collections::HashMap;

        fn writes_to_n_leaf_empty_trie_had_expected_results<'a, R, S, E>(
            environment: &'a R,
            store: &S,
            states: &[Blake2bHash],
            test_leaves: &[TestTrie],
        ) -> Result<(), E>
        where
            R: TransactionSource<'a, Handle = S::Handle>,
            S: TrieStore<TestKey, TestValue>,
            S::Error: From<R::Error>,
            E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
        {
            let mut states = states.to_vec();

            // Write set of leaves to the trie
            let hashes =
                write_leaves::<_, _, E>(environment, store, states.last().unwrap(), &test_leaves)?
                    .iter()
                    .map(|result| match result {
                        WriteResult::Written(root_hash) => *root_hash,
                        _ => panic!("write_leaves resulted in non-write"),
                    })
                    .collect::<Vec<Blake2bHash>>();

            states.extend(hashes);

            // Check that the expected set of leaves is in the trie at every
            // state, and that the set of other leaves is not.
            for (num_leaves, state) in states.iter().enumerate() {
                let (used, unused) = test_leaves.split_at(num_leaves);
                check_leaves::<_, _, E>(environment, store, state, used, unused)?;
            }

            Ok(())
        }

        #[test]
        fn lmdb_non_colliding_writes_to_n_leaf_empty_trie_had_expected_results() {
            for num_leaves in 1..=TEST_LEAVES_LENGTH {
                let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
                let mut context = LmdbTestContext::new(&tries).unwrap();
                let initial_states = vec![root_hash];

                writes_to_n_leaf_empty_trie_had_expected_results::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &initial_states,
                    &TEST_LEAVES_NON_COLLIDING[..num_leaves],
                )
                .unwrap()
            }
        }

        #[test]
        fn in_memory_non_colliding_writes_to_n_leaf_empty_trie_had_expected_results() {
            for num_leaves in 1..=TEST_LEAVES_LENGTH {
                let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
                let mut context = InMemoryTestContext::new(&tries).unwrap();
                let initial_states = vec![root_hash];

                writes_to_n_leaf_empty_trie_had_expected_results::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &initial_states,
                    &TEST_LEAVES_NON_COLLIDING[..num_leaves],
                )
                .unwrap()
            }
        }

        #[test]
        fn lmdb_writes_to_n_leaf_empty_trie_had_expected_results() {
            for num_leaves in 1..=TEST_LEAVES_LENGTH {
                let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
                let context = LmdbTestContext::new(&tries).unwrap();
                let initial_states = vec![root_hash];

                writes_to_n_leaf_empty_trie_had_expected_results::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &initial_states,
                    &TEST_LEAVES[..num_leaves],
                )
                .unwrap()
            }
        }

        #[test]
        fn in_memory_writes_to_n_leaf_empty_trie_had_expected_results() {
            for num_leaves in 1..=TEST_LEAVES_LENGTH {
                let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
                let context = InMemoryTestContext::new(&tries).unwrap();
                let initial_states = vec![root_hash];

                writes_to_n_leaf_empty_trie_had_expected_results::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &initial_states,
                    &TEST_LEAVES[..num_leaves],
                )
                .unwrap()
            }
        }

        #[test]
        fn in_memory_writes_to_n_leaf_empty_trie_had_expected_store_contents() {
            let expected_contents: HashMap<Blake2bHash, TestTrie> = {
                let mut ret = HashMap::new();
                for generator in &TEST_TRIE_GENERATORS {
                    let (_, tries) = generator().unwrap();
                    for HashedTestTrie { hash, trie } in tries {
                        ret.insert(hash, trie);
                    }
                }
                ret
            };

            let actual_contents: HashMap<Blake2bHash, TestTrie> = {
                let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
                let context = InMemoryTestContext::new(&tries).unwrap();

                write_leaves::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &root_hash,
                    &TEST_LEAVES,
                )
                .unwrap();

                context.environment.dump().unwrap()
            };

            assert_eq!(expected_contents, actual_contents)
        }
    }

    mod partial_tries {
        use super::*;

        fn noop_writes_to_n_leaf_partial_trie_had_expected_results<'a, R, S, E>(
            environment: &'a R,
            store: &S,
            states: &[Blake2bHash],
            num_leaves: usize,
        ) -> Result<(), E>
        where
            R: TransactionSource<'a, Handle = S::Handle>,
            S: TrieStore<TestKey, TestValue>,
            S::Error: From<R::Error>,
            E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
        {
            // Check that the expected set of leaves is in the trie
            check_leaves::<_, _, E>(
                environment,
                store,
                &states[0],
                &TEST_LEAVES[..num_leaves],
                &[],
            )?;

            // Rewrite that set of leaves
            let write_results = write_leaves::<_, _, E>(
                environment,
                store,
                &states[0],
                &TEST_LEAVES[..num_leaves],
            )?;

            assert!(write_results
                .iter()
                .all(|result| *result == WriteResult::AlreadyExists));

            // Check that the expected set of leaves is in the trie
            check_leaves::<_, _, E>(
                environment,
                store,
                &states[0],
                &TEST_LEAVES[..num_leaves],
                &[],
            )
        }

        #[test]
        fn lmdb_noop_writes_to_n_leaf_partial_trie_had_expected_results() {
            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (mut root_hash, tries) = generator().unwrap();
                let mut context = LmdbTestContext::new(&tries).unwrap();
                let states = vec![root_hash];

                noop_writes_to_n_leaf_partial_trie_had_expected_results::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    num_leaves,
                )
                .unwrap()
            }
        }

        #[test]
        fn in_memory_noop_writes_to_n_leaf_partial_trie_had_expected_results() {
            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (mut root_hash, tries) = generator().unwrap();
                let mut context = InMemoryTestContext::new(&tries).unwrap();
                let states = vec![root_hash];

                noop_writes_to_n_leaf_partial_trie_had_expected_results::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    num_leaves,
                )
                .unwrap();
            }
        }

        fn update_writes_to_n_leaf_partial_trie_had_expected_results<'a, R, S, E>(
            environment: &'a R,
            store: &S,
            states: &[Blake2bHash],
            num_leaves: usize,
        ) -> Result<(), E>
        where
            R: TransactionSource<'a, Handle = S::Handle>,
            S: TrieStore<TestKey, TestValue>,
            S::Error: From<R::Error>,
            E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
        {
            let mut states = states.to_owned();

            // Check that the expected set of leaves is in the trie
            check_leaves::<_, _, E>(
                environment,
                store,
                &states[0],
                &TEST_LEAVES[..num_leaves],
                &[],
            )?;

            // Update and check leaves
            for (n, leaf) in TEST_LEAVES_UPDATED[..num_leaves].iter().enumerate() {
                let expected_leaves: Vec<TestTrie> = {
                    let n = n + 1;
                    TEST_LEAVES_UPDATED[..n]
                        .iter()
                        .chain(&TEST_LEAVES[n..num_leaves])
                        .map(ToOwned::to_owned)
                        .collect()
                };

                let root_hash = {
                    let current_root = states.last().unwrap();
                    let results = write_leaves::<_, _, E>(
                        environment,
                        store,
                        &current_root,
                        &[leaf.to_owned()],
                    )?;
                    assert_eq!(1, results.len());
                    match results[0] {
                        WriteResult::Written(root_hash) => root_hash,
                        _ => panic!("value not written"),
                    }
                };

                states.push(root_hash);

                // Check that the expected set of leaves is in the trie
                check_leaves::<_, _, E>(
                    environment,
                    store,
                    states.last().unwrap(),
                    &expected_leaves,
                    &[],
                )?;
            }

            Ok(())
        }

        #[test]
        fn lmdb_update_writes_to_n_leaf_partial_trie_had_expected_results() {
            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                let mut context = LmdbTestContext::new(&tries).unwrap();
                let initial_states = vec![root_hash];

                update_writes_to_n_leaf_partial_trie_had_expected_results::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &initial_states,
                    num_leaves,
                )
                .unwrap()
            }
        }

        #[test]
        fn in_memory_update_writes_to_n_leaf_partial_trie_had_expected_results() {
            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                let mut context = InMemoryTestContext::new(&tries).unwrap();
                let states = vec![root_hash];

                update_writes_to_n_leaf_partial_trie_had_expected_results::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    num_leaves,
                )
                .unwrap()
            }
        }
    }

    mod full_tries {
        use super::*;

        fn noop_writes_to_n_leaf_full_trie_had_expected_results<'a, R, S, E>(
            environment: &'a R,
            store: &S,
            states: &[Blake2bHash],
            index: usize,
        ) -> Result<(), E>
        where
            R: TransactionSource<'a, Handle = S::Handle>,
            S: TrieStore<TestKey, TestValue>,
            S::Error: From<R::Error>,
            E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
        {
            // Check that the expected set of leaves is in the trie at every state reference
            for (num_leaves, state) in states[..index].iter().enumerate() {
                check_leaves::<_, _, E>(
                    environment,
                    store,
                    state,
                    &TEST_LEAVES[..num_leaves],
                    &[],
                )?;
            }

            // Rewrite that set of leaves
            let write_results = write_leaves::<_, _, E>(
                environment,
                store,
                states.last().unwrap(),
                &TEST_LEAVES[..index],
            )?;

            assert!(write_results
                .iter()
                .all(|result| *result == WriteResult::AlreadyExists));

            // Check that the expected set of leaves is in the trie at every state reference
            for (num_leaves, state) in states[..index].iter().enumerate() {
                check_leaves::<_, _, E>(environment, store, state, &TEST_LEAVES[..num_leaves], &[])?
            }

            Ok(())
        }

        #[test]
        fn lmdb_noop_writes_to_n_leaf_full_trie_had_expected_results() {
            let context = LmdbTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (index, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                noop_writes_to_n_leaf_full_trie_had_expected_results::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    index,
                )
                .unwrap();
            }
        }

        #[test]
        fn in_memory_noop_writes_to_n_leaf_full_trie_had_expected_results() {
            let context = InMemoryTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (index, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                noop_writes_to_n_leaf_full_trie_had_expected_results::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    index,
                )
                .unwrap();
            }
        }

        fn update_writes_to_n_leaf_full_trie_had_expected_results<'a, R, S, E>(
            environment: &'a R,
            store: &S,
            states: &[Blake2bHash],
            num_leaves: usize,
        ) -> Result<(), E>
        where
            R: TransactionSource<'a, Handle = S::Handle>,
            S: TrieStore<TestKey, TestValue>,
            S::Error: From<R::Error>,
            E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
        {
            let mut states = states.to_vec();

            // Check that the expected set of leaves is in the trie at every state reference
            for (state_index, state) in states.iter().enumerate() {
                check_leaves::<_, _, E>(
                    environment,
                    store,
                    state,
                    &TEST_LEAVES[..state_index],
                    &[],
                )?;
            }

            // Write set of leaves to the trie
            let hashes = write_leaves::<_, _, E>(
                environment,
                store,
                states.last().unwrap(),
                &TEST_LEAVES_UPDATED[..num_leaves],
            )?
            .iter()
            .map(|result| match result {
                WriteResult::Written(root_hash) => *root_hash,
                _ => panic!("write_leaves resulted in non-write"),
            })
            .collect::<Vec<Blake2bHash>>();

            states.extend(hashes);

            let expected: Vec<Vec<TestTrie>> = {
                let mut ret = vec![vec![]];
                if num_leaves > 0 {
                    for i in 1..=num_leaves {
                        ret.push(TEST_LEAVES[..i].to_vec())
                    }
                    for i in 1..=num_leaves {
                        ret.push(
                            TEST_LEAVES[i..num_leaves]
                                .iter()
                                .chain(&TEST_LEAVES_UPDATED[..i])
                                .map(ToOwned::to_owned)
                                .collect::<Vec<TestTrie>>(),
                        )
                    }
                }
                ret
            };

            assert_eq!(states.len(), expected.len());

            // Check that the expected set of leaves is in the trie at every state reference
            for (state_index, state) in states.iter().enumerate() {
                check_leaves::<_, _, E>(environment, store, state, &expected[state_index], &[])?;
            }

            Ok(())
        }

        #[test]
        fn lmdb_update_writes_to_n_leaf_full_trie_had_expected_results() {
            let context = LmdbTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                update_writes_to_n_leaf_full_trie_had_expected_results::<_, _, error::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    num_leaves,
                )
                .unwrap()
            }
        }

        #[test]
        fn in_memory_update_writes_to_n_leaf_full_trie_had_expected_results() {
            let context = InMemoryTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for (num_leaves, generator) in TEST_TRIE_GENERATORS.iter().enumerate() {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);

                update_writes_to_n_leaf_full_trie_had_expected_results::<_, _, in_memory::Error>(
                    &context.environment,
                    &context.store,
                    &states,
                    num_leaves,
                )
                .unwrap()
            }
        }

        fn node_writes_to_5_leaf_full_trie_had_expected_results<'a, R, S, E>(
            environment: &'a R,
            store: &S,
            states: &[Blake2bHash],
        ) -> Result<(), E>
        where
            R: TransactionSource<'a, Handle = S::Handle>,
            S: TrieStore<TestKey, TestValue>,
            S::Error: From<R::Error>,
            E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
        {
            let mut states = states.to_vec();
            let num_leaves = TEST_LEAVES_LENGTH;

            // Check that the expected set of leaves is in the trie at every state reference
            for (state_index, state) in states.iter().enumerate() {
                check_leaves::<_, _, E>(
                    environment,
                    store,
                    state,
                    &TEST_LEAVES[..state_index],
                    &[],
                )?;
            }

            // Write set of leaves to the trie
            let hashes = write_leaves::<_, _, E>(
                environment,
                store,
                states.last().unwrap(),
                &TEST_LEAVES_ADJACENTS,
            )?
            .iter()
            .map(|result| match result {
                WriteResult::Written(root_hash) => *root_hash,
                _ => panic!("write_leaves resulted in non-write"),
            })
            .collect::<Vec<Blake2bHash>>();

            states.extend(hashes);

            let expected: Vec<Vec<TestTrie>> = {
                let mut ret = vec![vec![]];
                if num_leaves > 0 {
                    for i in 1..=num_leaves {
                        ret.push(TEST_LEAVES[..i].to_vec())
                    }
                    for i in 1..=num_leaves {
                        ret.push(
                            TEST_LEAVES
                                .iter()
                                .chain(&TEST_LEAVES_ADJACENTS[..i])
                                .map(ToOwned::to_owned)
                                .collect::<Vec<TestTrie>>(),
                        )
                    }
                }
                ret
            };

            assert_eq!(states.len(), expected.len());

            // Check that the expected set of leaves is in the trie at every state reference
            for (state_index, state) in states.iter().enumerate() {
                check_leaves::<_, _, E>(environment, store, state, &expected[state_index], &[])?;
            }
            Ok(())
        }

        #[test]
        fn lmdb_node_writes_to_5_leaf_full_trie_had_expected_results() {
            let context = LmdbTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for generator in &TEST_TRIE_GENERATORS {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);
            }

            node_writes_to_5_leaf_full_trie_had_expected_results::<_, _, error::Error>(
                &context.environment,
                &context.store,
                &states,
            )
            .unwrap()
        }

        #[test]
        fn in_memory_node_writes_to_5_leaf_full_trie_had_expected_results() {
            let context = InMemoryTestContext::new(&[]).unwrap();
            let mut states: Vec<Blake2bHash> = Vec::new();

            for generator in &TEST_TRIE_GENERATORS {
                let (root_hash, tries) = generator().unwrap();
                context.update(&tries).unwrap();
                states.push(root_hash);
            }

            node_writes_to_5_leaf_full_trie_had_expected_results::<_, _, in_memory::Error>(
                &context.environment,
                &context.store,
                &states,
            )
            .unwrap()
        }
    }
}
mod proptests {
    use std::ops::RangeInclusive;

    use proptest::array;
    use proptest::collection::vec;
    use proptest::prelude::{any, proptest, Strategy};

    use super::*;

    const DEFAULT_MIN_LENGTH: usize = 0;

    const DEFAULT_MAX_LENGTH: usize = 100;

    fn get_range() -> RangeInclusive<usize> {
        let start = option_env!("CL_TRIE_TEST_VECTOR_MIN_LENGTH")
            .and_then(|s| str::parse::<usize>(s).ok())
            .unwrap_or(DEFAULT_MIN_LENGTH);
        let end = option_env!("CL_TRIE_TEST_VECTOR_MAX_LENGTH")
            .and_then(|s| str::parse::<usize>(s).ok())
            .unwrap_or(DEFAULT_MAX_LENGTH);
        RangeInclusive::new(start, end)
    }

    fn write_pairs<'a, R, S, E>(
        environment: &'a R,
        store: &S,
        root_hash: &Blake2bHash,
        pairs: &[(TestKey, TestValue)],
    ) -> Result<Vec<Blake2bHash>, E>
    where
        R: TransactionSource<'a, Handle = S::Handle>,
        S: TrieStore<TestKey, TestValue>,
        S::Error: From<R::Error>,
        E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
    {
        let mut results = Vec::new();
        if pairs.is_empty() {
            return Ok(results);
        }
        let mut root_hash = root_hash.to_owned();
        let mut txn = environment.create_read_write_txn()?;

        for (key, value) in pairs.iter() {
            match write::<_, _, _, _, E>(&mut txn, store, &root_hash, key, value)? {
                WriteResult::Written(hash) => {
                    root_hash = hash;
                }
                WriteResult::AlreadyExists => (),
                WriteResult::RootNotFound => panic!("write_leaves given an invalid root"),
            };
            results.push(root_hash);
        }
        txn.commit()?;
        Ok(results)
    }

    fn check_pairs<'a, R, S, E>(
        environment: &'a R,
        store: &S,
        root_hashes: &[Blake2bHash],
        pairs: &[(TestKey, TestValue)],
    ) -> Result<bool, E>
    where
        R: TransactionSource<'a, Handle = S::Handle>,
        S: TrieStore<TestKey, TestValue>,
        S::Error: From<R::Error>,
        E: From<R::Error> + From<S::Error> + From<common::bytesrepr::Error>,
    {
        let txn = environment.create_read_txn()?;
        for (index, root_hash) in root_hashes.iter().enumerate() {
            for (key, value) in &pairs[..=index] {
                let result = read::<_, _, _, _, E>(&txn, store, root_hash, key)?;
                if ReadResult::Found(*value) != result {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn lmdb_roundtrip_succeeds(pairs: &[(TestKey, TestValue)]) -> bool {
        let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
        let context = LmdbTestContext::new(&tries).unwrap();
        let mut states_to_check = vec![];

        let root_hashes = write_pairs::<_, _, error::Error>(
            &context.environment,
            &context.store,
            &root_hash,
            pairs,
        )
        .unwrap();

        states_to_check.extend(root_hashes);

        check_pairs::<_, _, error::Error>(
            &context.environment,
            &context.store,
            &states_to_check,
            &pairs,
        )
        .unwrap()
    }

    fn in_memory_roundtrip_succeeds(pairs: &[(TestKey, TestValue)]) -> bool {
        let (root_hash, tries) = TEST_TRIE_GENERATORS[0]().unwrap();
        let context = InMemoryTestContext::new(&tries).unwrap();
        let mut states_to_check = vec![];

        let root_hashes = write_pairs::<_, _, in_memory::Error>(
            &context.environment,
            &context.store,
            &root_hash,
            pairs,
        )
        .unwrap();

        states_to_check.extend(root_hashes);

        check_pairs::<_, _, in_memory::Error>(
            &context.environment,
            &context.store,
            &states_to_check,
            &pairs,
        )
        .unwrap()
    }

    fn test_key_arb() -> impl Strategy<Value = TestKey> {
        array::uniform7(any::<u8>()).prop_map(TestKey)
    }

    fn test_value_arb() -> impl Strategy<Value = TestValue> {
        array::uniform6(any::<u8>()).prop_map(TestValue)
    }

    proptest! {
        #[test]
        fn prop_in_memory_roundtrip_succeeds(inputs in vec((test_key_arb(), test_value_arb()), get_range())) {
            assert!(in_memory_roundtrip_succeeds(&inputs));
        }

        #[test]
        fn prop_lmdb_roundtrip_succeeds(inputs in vec((test_key_arb(), test_value_arb()), get_range())) {
            assert!(lmdb_roundtrip_succeeds(&inputs));
        }
    }
}
