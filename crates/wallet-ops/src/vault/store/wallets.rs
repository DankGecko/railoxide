use super::{
    DesktopVaultStore, DesktopViewSession, GeneratedSeedMaterial, SoftwareRailgunSpendSigner,
    SpendGrant, StoredWalletRecord, VaultError, VaultRecordEntries, ViewUnlock, WALLET_VIEW_PREFIX,
    WalletKeys, WalletMetadataBundle, WalletSpendBundle, WalletViewBundle, Zeroizing,
    bip39_entropy_from_mnemonic, initial_derived_public_account,
    public_account_metadata_record_entry, unlock_spend, unlock_view,
    wallet_chain_index_complete_record_entry, wallet_metadata_record_key, wallet_spend_record_key,
    wallet_view_record_key,
};

impl DesktopVaultStore {
    pub fn store_wallet_from_entropy(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: impl Into<String>,
        entropy: &[u8],
    ) -> Result<StoredWalletRecord, VaultError> {
        let (stored, records) = self.encrypted_wallet_records_from_entropy(
            password,
            wallet_id,
            derivation_index,
            bip39_language.into(),
            entropy,
            None,
        )?;
        self.db.put_desktop_wallet_vault_records(&records)?;
        Ok(stored)
    }

    pub fn store_wallet_from_entropy_with_metadata(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: impl Into<String>,
        entropy: &[u8],
        metadata: &WalletMetadataBundle,
    ) -> Result<StoredWalletRecord, VaultError> {
        let (stored, records) = self.encrypted_wallet_records_from_entropy(
            password,
            wallet_id,
            derivation_index,
            bip39_language.into(),
            entropy,
            Some(metadata),
        )?;
        self.db.put_desktop_wallet_vault_records(&records)?;
        Ok(stored)
    }

    pub fn store_generated_wallet(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: impl Into<String>,
        seed: &GeneratedSeedMaterial,
    ) -> Result<StoredWalletRecord, VaultError> {
        self.store_wallet_from_entropy(
            password,
            wallet_id,
            derivation_index,
            bip39_language,
            &seed.entropy,
        )
    }

    pub fn store_generated_wallet_with_metadata(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: impl Into<String>,
        seed: &GeneratedSeedMaterial,
        metadata: &WalletMetadataBundle,
    ) -> Result<StoredWalletRecord, VaultError> {
        self.store_wallet_from_entropy_with_metadata(
            password,
            wallet_id,
            derivation_index,
            bip39_language,
            &seed.entropy,
            metadata,
        )
    }

    pub fn import_wallet_mnemonic(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: impl Into<String>,
        mnemonic: &str,
    ) -> Result<StoredWalletRecord, VaultError> {
        let entropy = Zeroizing::new(bip39_entropy_from_mnemonic(mnemonic)?);
        self.store_wallet_from_entropy(
            password,
            wallet_id,
            derivation_index,
            bip39_language,
            &entropy,
        )
    }

    pub fn import_wallet_mnemonic_with_metadata(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: impl Into<String>,
        mnemonic: &str,
        metadata: &WalletMetadataBundle,
    ) -> Result<StoredWalletRecord, VaultError> {
        let entropy = Zeroizing::new(bip39_entropy_from_mnemonic(mnemonic)?);
        self.store_wallet_from_entropy_with_metadata(
            password,
            wallet_id,
            derivation_index,
            bip39_language,
            &entropy,
            metadata,
        )
    }

    pub fn load_view_bundle(
        &self,
        password: &str,
        wallet_id: &str,
    ) -> Result<WalletViewBundle, VaultError> {
        let view = self.unlock_view(password)?;
        self.ensure_password_view_allowed(&view, wallet_id)?;
        let record = self.encrypted_record(&wallet_view_record_key(wallet_id))?;
        view.decrypt_view_bundle(wallet_id, &record)
    }

    pub fn list_wallet_ids(&self) -> Result<Vec<String>, VaultError> {
        let records = self
            .db
            .list_desktop_wallet_vault_records(WALLET_VIEW_PREFIX)?;
        Ok(records
            .into_iter()
            .filter_map(|record| {
                record
                    .key
                    .strip_prefix(WALLET_VIEW_PREFIX)
                    .map(str::to_owned)
            })
            .collect())
    }

    pub fn load_view_session(
        &self,
        password: &str,
        wallet_id: &str,
    ) -> Result<DesktopViewSession, VaultError> {
        let view = self.unlock_view(password)?;
        self.ensure_password_view_allowed(&view, wallet_id)?;
        let record = self.encrypted_record(&wallet_view_record_key(wallet_id))?;
        let bundle = view.decrypt_view_bundle(wallet_id, &record)?;
        Ok(DesktopViewSession::from_bundle(
            wallet_id.to_owned(),
            &bundle,
            view,
        ))
    }

    pub fn load_view_session_with_view_session(
        &self,
        view_session: &DesktopViewSession,
        wallet_id: &str,
    ) -> Result<DesktopViewSession, VaultError> {
        self.ensure_password_view_allowed(&view_session.view, wallet_id)?;
        let record = self.encrypted_record(&wallet_view_record_key(wallet_id))?;
        let bundle = view_session.view.decrypt_view_bundle(wallet_id, &record)?;
        Ok(DesktopViewSession::from_bundle(
            wallet_id.to_owned(),
            &bundle,
            view_session.view.clone_unlock(),
        ))
    }

    pub fn load_view_session_with_view_unlock(
        &self,
        view: &ViewUnlock,
        wallet_id: &str,
    ) -> Result<DesktopViewSession, VaultError> {
        self.ensure_password_view_allowed(view, wallet_id)?;
        let record = self.encrypted_record(&wallet_view_record_key(wallet_id))?;
        let bundle = view.decrypt_view_bundle(wallet_id, &record)?;
        Ok(DesktopViewSession::from_bundle(
            wallet_id.to_owned(),
            &bundle,
            view.clone_unlock(),
        ))
    }

    pub fn unlock_first_view_session(
        &self,
        password: &str,
    ) -> Result<Option<DesktopViewSession>, VaultError> {
        let view = self.unlock_view(password)?;
        let wallet_ids = self.list_wallet_ids()?;
        if wallet_ids.is_empty() {
            return Ok(None);
        }
        for wallet_id in wallet_ids {
            match self.ensure_password_view_allowed(&view, &wallet_id) {
                Ok(()) => {
                    let record = self.encrypted_record(&wallet_view_record_key(&wallet_id))?;
                    let bundle = view.decrypt_view_bundle(&wallet_id, &record)?;
                    return Ok(Some(DesktopViewSession::from_bundle(
                        wallet_id, &bundle, view,
                    )));
                }
                Err(
                    VaultError::HardwareWalletViewRequiresDevice
                    | VaultError::UnsupportedHardwareCustodyBackend(_),
                ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    pub fn load_spend_bundle(
        &self,
        grant: &mut SpendGrant,
        wallet_id: &str,
    ) -> Result<WalletSpendBundle, VaultError> {
        let record = self.encrypted_record(&wallet_spend_record_key(wallet_id))?;
        grant
            .take_spend_unlock()?
            .decrypt_spend_bundle(wallet_id, &record)
    }

    pub fn railgun_spend_signer(
        &self,
        grant: &mut SpendGrant,
        wallet_id: &str,
    ) -> Result<SoftwareRailgunSpendSigner, VaultError> {
        let bundle = self.load_spend_bundle(grant, wallet_id)?;
        let wallet =
            WalletKeys::from_bip39_entropy(&bundle.bip39_entropy, bundle.derivation_index)?;
        Ok(SoftwareRailgunSpendSigner { wallet })
    }

    fn encrypted_wallet_records_from_entropy(
        &self,
        password: &str,
        wallet_id: &str,
        derivation_index: u32,
        bip39_language: String,
        entropy: &[u8],
        metadata: Option<&WalletMetadataBundle>,
    ) -> Result<(StoredWalletRecord, VaultRecordEntries), VaultError> {
        let vault_metadata = self.metadata()?;
        let view = unlock_view(&vault_metadata, password)?;
        let spend = unlock_spend(&vault_metadata, password)?;
        let wallet = WalletKeys::from_bip39_entropy(entropy, derivation_index)?;
        let view_bundle = WalletViewBundle::from_wallet_keys(derivation_index, &wallet);
        let spend_bundle = WalletSpendBundle {
            derivation_index,
            bip39_language,
            bip39_entropy: entropy.to_vec(),
        };
        let existing_public_accounts = if metadata.is_some() {
            self.list_public_account_metadata_with_view(&view)?
        } else {
            Vec::new()
        };

        let view_record = view.encrypt_view_bundle(wallet_id, &view_bundle)?;
        let spend_record = spend.encrypt_spend_bundle(wallet_id, &spend_bundle)?;
        let view_record_key = wallet_view_record_key(wallet_id);
        let spend_record_key = wallet_spend_record_key(wallet_id);
        let mut records = Vec::with_capacity(2 + usize::from(metadata.is_some()) * 3);
        records.push(view_record.to_record_entry(view_record_key.clone())?);
        records.push(spend_record.to_record_entry(spend_record_key.clone())?);

        if let Some(metadata) = metadata {
            let record = view.encrypt_wallet_metadata(&metadata.wallet_uuid, metadata)?;
            records
                .push(record.to_record_entry(wallet_metadata_record_key(&metadata.wallet_uuid))?);
            records.push(wallet_chain_index_complete_record_entry(
                &metadata.wallet_uuid,
            )?);

            let public_account =
                initial_derived_public_account(wallet_id, entropy, &existing_public_accounts)?;
            records.push(public_account_metadata_record_entry(
                &view,
                &public_account,
            )?);
        }

        Ok((
            StoredWalletRecord {
                wallet_id: wallet_id.to_string(),
                derivation_index,
                view_record_key,
                spend_record_key,
            },
            records,
        ))
    }
}
