// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::{
    authority_store::{InternalSequenceNumber, ObjectKey},
    *,
};
use narwhal_executor::ExecutionIndices;
use rocksdb::Options;
use serde::{Deserialize, Serialize};
use std::path::Path;
use sui_storage::default_db_options;
use sui_types::base_types::{ExecutionDigests, SequenceNumber};
use sui_types::batch::{SignedBatch, TxSequenceNumber};
use typed_store::rocks::DBMap;
use typed_store::traits::TypedStoreDebug;

use typed_store_derive::DBMapUtils;

/// AuthorityEpochTables contains tables that contain data that is only valid within an epoch.
#[derive(DBMapUtils)]
pub struct AuthorityEpochTables<S> {
    /// This is map between the transaction digest and transactions found in the `transaction_lock`.
    #[default_options_override_fn = "transactions_table_default_config"]
    pub(crate) transactions: DBMap<TransactionDigest, TransactionEnvelope<S>>,

    /// The pending execution table holds a sequence of transactions that are present
    /// in the certificates table, but may not have yet been executed, and should be executed.
    /// The source of these certificates might be (1) the checkpoint proposal process (2) the
    /// gossip processes (3) the shared object post-consensus task. An active authority process
    /// reads this table and executes the certificates. The order is a hint as to their
    /// causal dependencies. Note that there is no guarantee digests are unique. Once executed, and
    /// effects are written the entry should be deleted.
    pub(crate) pending_execution: DBMap<InternalSequenceNumber, TransactionDigest>,

    /// Hold the lock for shared objects. These locks are written by a single task: upon receiving a valid
    /// certified transaction from consensus, the authority assigns a lock to each shared objects of the
    /// transaction. Note that all authorities are guaranteed to assign the same lock to these objects.
    /// TODO: These two maps should be merged into a single one (no reason to have two).
    pub(crate) assigned_object_versions: DBMap<(TransactionDigest, ObjectID), SequenceNumber>,
    pub(crate) next_object_versions: DBMap<ObjectID, SequenceNumber>,

    /// Track which transactions have been processed in handle_consensus_transaction. We must be
    /// sure to advance next_object_versions exactly once for each transaction we receive from
    /// consensus. But, we may also be processing transactions from checkpoints, so we need to
    /// track this state separately.
    ///
    /// Entries in this table can be garbage collected whenever we can prove that we won't receive
    /// another handle_consensus_transaction call for the given digest. This probably means at
    /// epoch change.
    pub(crate) consensus_message_processed: DBMap<TransactionDigest, bool>,

    /// The following table is used to store a single value (the corresponding key is a constant). The value
    /// represents the index of the latest consensus message this authority processed. This field is written
    /// by a single process acting as consensus (light) client. It is used to ensure the authority processes
    /// every message output by consensus (and in the right order).
    pub(crate) last_consensus_index: DBMap<u64, ExecutionIndicesWithHash>,
}

impl<S> AuthorityEpochTables<S>
where
    S: std::fmt::Debug + Serialize + for<'de> Deserialize<'de>,
{
    pub fn path(parent_path: &Path) -> PathBuf {
        parent_path.join("epoch")
    }

    pub fn open(parent_path: &Path, db_options: Option<Options>) -> Self {
        Self::open_tables_read_write(Self::path(parent_path), db_options, None)
    }

    pub fn open_readonly(parent_path: &Path) -> AuthorityEpochTablesReadOnly<S> {
        Self::get_read_only_handle(Self::path(parent_path), None, None)
    }
}

/// AuthorityPerpetualTables contains data that must be preserved from one epoch to the next.
#[derive(DBMapUtils)]
pub struct AuthorityPerpetualTables<S> {
    /// This is a map between the object (ID, version) and the latest state of the object, namely the
    /// state that is needed to process new transactions.
    ///
    /// Note that while this map can store all versions of an object, we will eventually
    /// prune old object versions from the db.
    ///
    /// IMPORTANT: object versions must *only* be pruned if they appear as inputs in some
    /// TransactionEffects. Simply pruning all objects but the most recent is an error!
    /// This is because there can be partially executed transactions whose effects have not yet
    /// been written out, and which must be retried. But, they cannot be retried unless their input
    /// objects are still accessible!
    #[default_options_override_fn = "objects_table_default_config"]
    pub(crate) objects: DBMap<ObjectKey, Object>,

    /// This is a an index of object references to currently existing objects, indexed by the
    /// composite key of the SuiAddress of their owner and the object ID of the object.
    /// This composite index allows an efficient iterator to list all objected currently owned
    /// by a specific user, and their object reference.
    pub(crate) owner_index: DBMap<(Owner, ObjectID), ObjectInfo>,

    /// This is a map between the transaction digest and the corresponding certificate for all
    /// certificates that have been successfully processed by this authority. This set of certificates
    /// along with the genesis allows the reconstruction of all other state, and a full sync to this
    /// authority.
    #[default_options_override_fn = "certificates_table_default_config"]
    pub(crate) certificates: DBMap<TransactionDigest, CertifiedTransaction>,

    /// The map between the object ref of objects processed at all versions and the transaction
    /// digest of the certificate that lead to the creation of this version of the object.
    ///
    /// When an object is deleted we include an entry into this table for its next version and
    /// a digest of ObjectDigest::deleted(), along with a link to the transaction that deleted it.
    pub(crate) parent_sync: DBMap<ObjectRef, TransactionDigest>,

    /// A map between the transaction digest of a certificate that was successfully processed
    /// (ie in `certificates`) and the effects its execution has on the authority state. This
    /// structure is used to ensure we do not double process a certificate, and that we can return
    /// the same response for any call after the first (ie. make certificate processing idempotent).
    #[default_options_override_fn = "effects_table_default_config"]
    pub(crate) effects: DBMap<TransactionDigest, TransactionEffectsEnvelope<S>>,

    // Tables used for authority batch structure
    // TODO: executed_sequence and batches both conceptually belong in AuthorityEpochTables,
    // but we currently require that effects and executed_sequence are written atomically.
    // See https://github.com/MystenLabs/sui/pull/4395 for the reason why.
    //
    // This can be addressed when we do the WAL rework. Something similar to the following flow
    // would be required:
    // 1. First execute the tx and store the outputs in an intermediate location.
    // 2. Note that execution has finished (e.g. in the WAL.)
    // 3. Write intermediate outputs to their permanent locations.
    // 4. Mark the tx as finished in the WAL.
    // 5. Crucially: If step 3 is interrupted, we must restart at step 3 based solely on the fact
    //    that the WAL indicates the tx is not written yet. This fixes the root cause of the issue,
    //    which is that we currently exit early if effects have been written.
    /// A sequence on all executed certificates and effects.
    pub executed_sequence: DBMap<TxSequenceNumber, ExecutionDigests>,

    /// A sequence of batches indexing into the sequence of executed transactions.
    pub batches: DBMap<TxSequenceNumber, SignedBatch>,
}

impl<S> AuthorityPerpetualTables<S>
where
    S: std::fmt::Debug + Serialize + for<'de> Deserialize<'de>,
{
    pub fn path(parent_path: &Path) -> PathBuf {
        parent_path.join("perpetual")
    }

    pub fn open(parent_path: &Path, db_options: Option<Options>) -> Self {
        Self::open_tables_read_write(Self::path(parent_path), db_options, None)
    }

    pub fn open_readonly(parent_path: &Path) -> AuthorityPerpetualTablesReadOnly<S> {
        Self::get_read_only_handle(Self::path(parent_path), None, None)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutionIndicesWithHash {
    pub index: ExecutionIndices,
    pub hash: u64,
}

// These functions are used to initialize the DB tables
fn objects_table_default_config() -> Options {
    default_db_options(None, None).1
}
fn transactions_table_default_config() -> Options {
    default_db_options(None, None).1
}
fn certificates_table_default_config() -> Options {
    default_db_options(None, None).1
}
fn effects_table_default_config() -> Options {
    default_db_options(None, None).1
}
