// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use anyhow::anyhow;
use clap::Parser;
use eyre::eyre;
use rocksdb::MultiThreaded;
use std::collections::BTreeMap;
use std::path::PathBuf;
use strum_macros::EnumString;
use sui_core::authority::authority_store_tables::{AuthorityEpochTables, AuthorityPerpetualTables};
use sui_core::checkpoints::CheckpointStoreTables;
use sui_core::epoch::committee_store::CommitteeStore;
use sui_storage::default_db_options;
use sui_storage::{lock_service::LockServiceImpl, node_sync_store::NodeSyncStore, IndexStore};
use sui_types::crypto::{AuthoritySignInfo, EmptySignInfo};

#[derive(EnumString, Parser, Debug)]
pub enum StoreName {
    Validator,
    Gateway,
    Index,
    LocksService,
    NodeSync,
    Checkpoints,
    Wal,
    Epoch,
}
impl std::fmt::Display for StoreName {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn list_tables(path: PathBuf) -> anyhow::Result<Vec<String>> {
    rocksdb::DBWithThreadMode::<MultiThreaded>::list_cf(&default_db_options(None, None).0, &path)
        .map_err(|e| e.into())
        .map(|q| {
            q.iter()
                .filter_map(|s| {
                    // The `default` table is not used
                    if s != "default" {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .collect()
        })
}

// TODO: condense this using macro or trait dyn skills
pub fn dump_table(
    store_name: StoreName,
    db_path: PathBuf,
    table_name: &str,
    page_size: u16,
    page_number: usize,
) -> anyhow::Result<BTreeMap<String, String>> {
    match store_name {
        StoreName::Validator => {
            let epoch_tables = AuthorityEpochTables::<AuthoritySignInfo>::describe_tables();
            if epoch_tables.contains_key(table_name) {
                AuthorityEpochTables::<AuthoritySignInfo>::open_readonly(&db_path).dump(
                    table_name,
                    page_size,
                    page_number,
                )
            } else {
                AuthorityPerpetualTables::<AuthoritySignInfo>::open_readonly(&db_path).dump(
                    table_name,
                    page_size,
                    page_number,
                )
            }
        }
        StoreName::Gateway => {
            AuthorityEpochTables::<EmptySignInfo>::open_readonly(&db_path)
                .dump(table_name, page_size, page_number)
                .map_err(|e| anyhow!(e))?;
            AuthorityPerpetualTables::<EmptySignInfo>::open_readonly(&db_path).dump(
                table_name,
                page_size,
                page_number,
            )
        }
        StoreName::Index => IndexStore::get_read_only_handle(db_path, None, None).dump(
            table_name,
            page_size,
            page_number,
        ),
        StoreName::LocksService => LockServiceImpl::get_read_only_handle(db_path, None, None).dump(
            table_name,
            page_size,
            page_number,
        ),
        StoreName::NodeSync => NodeSyncStore::get_read_only_handle(db_path, None, None).dump(
            table_name,
            page_size,
            page_number,
        ),
        StoreName::Checkpoints => CheckpointStoreTables::get_read_only_handle(db_path, None, None)
            .dump(table_name, page_size, page_number),
        StoreName::Wal => Err(eyre!(
            "Dumping WAL not yet supported. It requires kmowing the value type"
        )),
        StoreName::Epoch => CommitteeStore::get_read_only_handle(db_path, None, None).dump(
            table_name,
            page_size,
            page_number,
        ),
    }
    .map_err(|err| anyhow!(err.to_string()))
}

#[cfg(test)]
mod test {
    use sui_core::authority::authority_store_tables::AuthorityEpochTables;
    use sui_core::authority::authority_store_tables::AuthorityPerpetualTables;
    use sui_types::crypto::AuthoritySignInfo;

    use crate::db_tool::db_dump::{dump_table, list_tables, StoreName};

    #[test]
    fn db_dump_population() -> Result<(), anyhow::Error> {
        let primary_path = tempfile::tempdir()?.into_path();

        // Open the DB for writing
        let _: AuthorityEpochTables<AuthoritySignInfo> =
            AuthorityEpochTables::open(&primary_path, None);
        let _: AuthorityPerpetualTables<AuthoritySignInfo> =
            AuthorityPerpetualTables::open(&primary_path, None);

        // Get all the tables for AuthorityEpochTables
        let tables = {
            let mut epoch_tables = list_tables(AuthorityEpochTables::<AuthoritySignInfo>::path(
                &primary_path,
            ))
            .unwrap();
            let mut perpetual_tables = list_tables(
                AuthorityPerpetualTables::<AuthoritySignInfo>::path(&primary_path),
            )
            .unwrap();
            epoch_tables.append(&mut perpetual_tables);
            epoch_tables
        };

        let mut missing_tables = vec![];
        for t in tables {
            println!("{}", t);
            if dump_table(StoreName::Validator, primary_path.clone(), &t, 0, 0).is_err() {
                missing_tables.push(t);
            }
        }
        if missing_tables.is_empty() {
            return Ok(());
        }
        panic!(
            "{}",
            format!(
                "Missing {} table(s) from DB dump registration function: {:?} \n Update the dump function.",
                missing_tables.len(),
                missing_tables
            )
        );
    }
}
