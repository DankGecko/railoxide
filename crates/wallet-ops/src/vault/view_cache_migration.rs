use std::collections::BTreeMap;

use alloy::primitives::FixedBytes;
use local_db::{
    OpaqueWalletPrivateRow, OutputPoiRecoveryRecord, PendingOutputPoiContextRecord,
    WalletPrivateCanonicalizationBatch, WalletPrivateCanonicalizationKindBatch,
    WalletPrivateNamespaceId, WalletPrivateRecordKind, WalletPrivateV1Row,
};

use super::{DesktopEncryptedWalletCacheStore, KEY_LEN, VaultError};

const WALLET_PRIVATE_CANONICALIZATION_VERSION: u32 = 1;
const LEGACY_V1_PRIORITY: u8 = 0;
const LEGACY_V2_PRIORITY: u8 = 1;
const CANONICAL_V1_PRIORITY: u8 = 2;
const CANONICAL_V2_PRIORITY: u8 = 3;

struct Candidate<T> {
    record: T,
    canonical_row: Option<OpaqueWalletPrivateRow>,
    priority: u8,
}

trait CanonicalRecord {
    const KIND: &'static str;

    fn chain_id(&self) -> u64;
    fn wallet_id(&self) -> &str;
    fn wallet_id_mut(&mut self) -> &mut String;
    fn output_commitment(&self) -> FixedBytes<KEY_LEN>;
    fn immutable_matches(&self, other: &Self) -> bool;
}

impl CanonicalRecord for PendingOutputPoiContextRecord {
    const KIND: &'static str = "pending output POI context";

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn wallet_id(&self) -> &str {
        &self.wallet_id
    }

    fn wallet_id_mut(&mut self) -> &mut String {
        &mut self.wallet_id
    }

    fn output_commitment(&self) -> FixedBytes<KEY_LEN> {
        self.output_commitment
    }

    fn immutable_matches(&self, other: &Self) -> bool {
        self.output_commitment == other.output_commitment
            && self.txid_version == other.txid_version
            && self.output_npk == other.output_npk
            && self.utxo_tree_in == other.utxo_tree_in
            && self.railgun_txid == other.railgun_txid
            && self.output_role == other.output_role
            && self.source_operation_id == other.source_operation_id
    }
}

impl CanonicalRecord for OutputPoiRecoveryRecord {
    const KIND: &'static str = "output POI recovery";

    fn chain_id(&self) -> u64 {
        self.chain_id
    }

    fn wallet_id(&self) -> &str {
        &self.wallet_id
    }

    fn wallet_id_mut(&mut self) -> &mut String {
        &mut self.wallet_id
    }

    fn output_commitment(&self) -> FixedBytes<KEY_LEN> {
        self.output_commitment
    }

    fn immutable_matches(&self, other: &Self) -> bool {
        self.output_commitment == other.output_commitment
            && self.source_tx_hash == other.source_tx_hash
    }
}

impl DesktopEncryptedWalletCacheStore {
    pub(super) fn canonicalize_wallet_private_storage(
        &self,
        canonical: &WalletPrivateNamespaceId,
        legacy: Option<&WalletPrivateNamespaceId>,
    ) -> Result<(), VaultError> {
        if self.db.wallet_private_canonicalization_version(canonical)?
            >= WALLET_PRIVATE_CANONICALIZATION_VERSION
        {
            return Ok(());
        }

        let canonical_v1 = self.db.list_wallet_private_v1_rows(canonical)?;
        let canonical_pending_v2 = self.db.list_opaque_wallet_private_rows(
            canonical,
            WalletPrivateRecordKind::PendingOutputPoiContext,
        )?;
        let canonical_recovery_v2 = self.db.list_opaque_wallet_private_rows(
            canonical,
            WalletPrivateRecordKind::OutputPoiRecovery,
        )?;
        let legacy_v1 = legacy
            .map(|namespace| self.db.list_wallet_private_v1_rows(namespace))
            .transpose()?
            .unwrap_or_default();
        let legacy_pending_v2 = legacy
            .map(|namespace| {
                self.db.list_opaque_wallet_private_rows(
                    namespace,
                    WalletPrivateRecordKind::PendingOutputPoiContext,
                )
            })
            .transpose()?
            .unwrap_or_default();
        let legacy_recovery_v2 = legacy
            .map(|namespace| {
                self.db.list_opaque_wallet_private_rows(
                    namespace,
                    WalletPrivateRecordKind::OutputPoiRecovery,
                )
            })
            .transpose()?
            .unwrap_or_default();

        let pending_destinations = self.canonical_pending_destinations(
            canonical,
            legacy,
            &canonical_v1.pending_output_contexts,
            &legacy_v1.pending_output_contexts,
            &canonical_pending_v2,
            &legacy_pending_v2,
        )?;
        let recovery_destinations = self.canonical_recovery_destinations(
            canonical,
            legacy,
            &canonical_v1.output_poi_recoveries,
            &legacy_v1.output_poi_recoveries,
            &canonical_recovery_v2,
            &legacy_recovery_v2,
        )?;
        let report =
            self.db
                .canonicalize_wallet_private_rows(&WalletPrivateCanonicalizationBatch {
                    canonical_namespace: canonical,
                    legacy_namespace: legacy,
                    target_version: WALLET_PRIVATE_CANONICALIZATION_VERSION,
                    pending_output_contexts: WalletPrivateCanonicalizationKindBatch {
                        canonical_v1_sources: &canonical_v1.pending_output_contexts,
                        legacy_v1_sources: &legacy_v1.pending_output_contexts,
                        canonical_v2_sources: &canonical_pending_v2,
                        legacy_v2_sources: &legacy_pending_v2,
                        canonical_v2_destinations: &pending_destinations,
                    },
                    output_poi_recoveries: WalletPrivateCanonicalizationKindBatch {
                        canonical_v1_sources: &canonical_v1.output_poi_recoveries,
                        legacy_v1_sources: &legacy_v1.output_poi_recoveries,
                        canonical_v2_sources: &canonical_recovery_v2,
                        legacy_v2_sources: &legacy_recovery_v2,
                        canonical_v2_destinations: &recovery_destinations,
                    },
                })?;
        tracing::info!(
            chain_id = canonical.chain_id,
            pending_output_context_rows = report.pending_output_context_rows,
            output_poi_recovery_rows = report.output_poi_recovery_rows,
            plaintext_rows_removed = report.plaintext_rows_removed,
            "canonicalized wallet-private storage"
        );
        Ok(())
    }

    fn canonical_pending_destinations(
        &self,
        canonical: &WalletPrivateNamespaceId,
        legacy: Option<&WalletPrivateNamespaceId>,
        canonical_v1: &[WalletPrivateV1Row],
        legacy_v1: &[WalletPrivateV1Row],
        canonical_v2: &[OpaqueWalletPrivateRow],
        legacy_v2: &[OpaqueWalletPrivateRow],
    ) -> Result<Vec<OpaqueWalletPrivateRow>, VaultError> {
        let mut candidates = BTreeMap::new();
        if let Some(legacy) = legacy {
            for row in legacy_v1 {
                let record = rmp_serde::from_slice(&row.payload)?;
                select_candidate(
                    &mut candidates,
                    normalized_record(record, legacy, canonical)?,
                    None,
                    LEGACY_V1_PRIORITY,
                )?;
            }
            for row in legacy_v2 {
                let record = self.decode_pending_output_row(legacy, row)?;
                select_candidate(
                    &mut candidates,
                    normalized_record(record, legacy, canonical)?,
                    None,
                    LEGACY_V2_PRIORITY,
                )?;
            }
        }
        for row in canonical_v1 {
            let record = rmp_serde::from_slice(&row.payload)?;
            select_candidate(
                &mut candidates,
                normalized_record(record, canonical, canonical)?,
                None,
                CANONICAL_V1_PRIORITY,
            )?;
        }
        for row in canonical_v2 {
            let record = self.decode_pending_output_row(canonical, row)?;
            select_candidate(
                &mut candidates,
                normalized_record(record, canonical, canonical)?,
                Some(row.clone()),
                CANONICAL_V2_PRIORITY,
            )?;
        }
        candidates
            .into_values()
            .map(|candidate| {
                candidate
                    .canonical_row
                    .map_or_else(|| self.pending_output_row(canonical, &candidate.record), Ok)
            })
            .collect()
    }

    fn canonical_recovery_destinations(
        &self,
        canonical: &WalletPrivateNamespaceId,
        legacy: Option<&WalletPrivateNamespaceId>,
        canonical_v1: &[WalletPrivateV1Row],
        legacy_v1: &[WalletPrivateV1Row],
        canonical_v2: &[OpaqueWalletPrivateRow],
        legacy_v2: &[OpaqueWalletPrivateRow],
    ) -> Result<Vec<OpaqueWalletPrivateRow>, VaultError> {
        let mut candidates = BTreeMap::new();
        if let Some(legacy) = legacy {
            for row in legacy_v1 {
                let record = rmp_serde::from_slice(&row.payload)?;
                select_candidate(
                    &mut candidates,
                    normalized_record(record, legacy, canonical)?,
                    None,
                    LEGACY_V1_PRIORITY,
                )?;
            }
            for row in legacy_v2 {
                let record = self.decode_output_recovery_row(legacy, row)?;
                select_candidate(
                    &mut candidates,
                    normalized_record(record, legacy, canonical)?,
                    None,
                    LEGACY_V2_PRIORITY,
                )?;
            }
        }
        for row in canonical_v1 {
            let record = rmp_serde::from_slice(&row.payload)?;
            select_candidate(
                &mut candidates,
                normalized_record(record, canonical, canonical)?,
                None,
                CANONICAL_V1_PRIORITY,
            )?;
        }
        for row in canonical_v2 {
            let record = self.decode_output_recovery_row(canonical, row)?;
            select_candidate(
                &mut candidates,
                normalized_record(record, canonical, canonical)?,
                Some(row.clone()),
                CANONICAL_V2_PRIORITY,
            )?;
        }
        candidates
            .into_values()
            .map(|candidate| {
                candidate.canonical_row.map_or_else(
                    || self.output_recovery_row(canonical, &candidate.record),
                    Ok,
                )
            })
            .collect()
    }
}

fn normalized_record<T: CanonicalRecord>(
    mut record: T,
    source: &WalletPrivateNamespaceId,
    canonical: &WalletPrivateNamespaceId,
) -> Result<T, VaultError> {
    if record.chain_id() != source.chain_id || record.wallet_id() != source.wallet_id.as_str() {
        return Err(VaultError::WalletPrivateMigrationConflict {
            kind: T::KIND,
            reason: "source namespace mismatch",
        });
    }
    *record.wallet_id_mut() = canonical.wallet_id.to_string();
    Ok(record)
}

fn select_candidate<T: CanonicalRecord>(
    candidates: &mut BTreeMap<FixedBytes<KEY_LEN>, Candidate<T>>,
    record: T,
    canonical_row: Option<OpaqueWalletPrivateRow>,
    priority: u8,
) -> Result<(), VaultError> {
    let key = record.output_commitment();
    if let Some(current) = candidates.get(&key)
        && !current.record.immutable_matches(&record)
    {
        return Err(VaultError::WalletPrivateMigrationConflict {
            kind: T::KIND,
            reason: "immutable identity mismatch",
        });
    }
    if candidates
        .get(&key)
        .is_none_or(|current| priority > current.priority)
    {
        candidates.insert(
            key,
            Candidate {
                record,
                canonical_row,
                priority,
            },
        );
    }
    Ok(())
}
