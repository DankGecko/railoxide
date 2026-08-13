use super::{
    Arc, HardwareDeviceKind, HardwareProfileMetadata, HardwareRailgunAccountMetadata,
    RememberedWalletKind, WalletMetadataBundle, WalletOption, WalletSelectItem,
    WalletSoftwareContextKind, WalletSource, sort_wallet_metadata,
};

pub(in crate::root) fn wallet_options_from_metadata(
    mut metadata: Vec<WalletMetadataBundle>,
) -> Vec<WalletOption> {
    metadata.retain(|metadata| metadata.status == wallet_ops::vault::WalletStatus::Active);
    sort_wallet_metadata(&mut metadata);
    metadata
        .into_iter()
        .map(|metadata| WalletOption {
            wallet_id: Arc::from(metadata.wallet_uuid),
            source: metadata.source,
        })
        .collect()
}

pub(in crate::root) fn visible_wallet_metadata(
    metadata: &[WalletMetadataBundle],
    revealed_wallet_id: Option<&str>,
) -> Vec<WalletMetadataBundle> {
    metadata
        .iter()
        .filter(|metadata| {
            let concealed = metadata
                .software_context
                .as_ref()
                .is_some_and(|context| context.kind == WalletSoftwareContextKind::Passphrase);
            !concealed || revealed_wallet_id == Some(metadata.wallet_uuid.as_str())
        })
        .cloned()
        .collect()
}

pub(in crate::root) fn passphrase_open_action_is_eligible(
    metadata: Option<&WalletMetadataBundle>,
) -> bool {
    let Some(metadata) = metadata else {
        return false;
    };
    let Some(context) = metadata.software_context.as_ref() else {
        return false;
    };
    metadata.status == wallet_ops::vault::WalletStatus::Active
        && !metadata.source.is_hardware_derived()
        && context.kind == WalletSoftwareContextKind::Standard
        && context.base_profile_uuid == metadata.wallet_uuid
}

pub(in crate::root) fn remembered_wallet_id_for_restore(
    metadata: &[WalletMetadataBundle],
    remembered_wallet_id: Option<&str>,
    remembered_wallet_kind: RememberedWalletKind,
) -> Option<Arc<str>> {
    let remembered_wallet_id = remembered_wallet_id?;
    let standard_context_for_base = |base_profile_uuid: &str| {
        metadata.iter().find_map(|metadata| {
            let context = metadata.software_context.as_ref()?;
            (context.base_profile_uuid == base_profile_uuid
                && context.kind == WalletSoftwareContextKind::Standard)
                .then(|| Arc::from(metadata.wallet_uuid.clone()))
        })
    };

    match remembered_wallet_kind {
        RememberedWalletKind::SoftwareProfile => {
            let base_profile_uuid = metadata
                .iter()
                .find(|metadata| metadata.wallet_uuid == remembered_wallet_id)
                .and_then(|metadata| metadata.software_context.as_ref())
                .filter(|context| context.kind == WalletSoftwareContextKind::Passphrase)
                .map_or(remembered_wallet_id, |context| {
                    context.base_profile_uuid.as_str()
                });
            standard_context_for_base(base_profile_uuid)
        }
        RememberedWalletKind::HardwareWallet => metadata
            .iter()
            .find(|metadata| {
                metadata.wallet_uuid == remembered_wallet_id
                    && metadata.source.is_hardware_derived()
            })
            .map(|metadata| Arc::from(metadata.wallet_uuid.clone())),
        RememberedWalletKind::Unknown => {
            let remembered = metadata
                .iter()
                .find(|metadata| metadata.wallet_uuid == remembered_wallet_id)?;
            if let Some(context) = remembered
                .software_context
                .as_ref()
                .filter(|context| context.kind == WalletSoftwareContextKind::Passphrase)
            {
                return standard_context_for_base(&context.base_profile_uuid);
            }
            Some(Arc::from(remembered.wallet_uuid.clone()))
        }
    }
}

pub(in crate::root) const fn hardware_device_wallet_select_value(
    device_kind: HardwareDeviceKind,
) -> &'static str {
    match device_kind {
        HardwareDeviceKind::Ledger => "hardware-device:ledger",
        HardwareDeviceKind::Trezor => "hardware-device:trezor",
    }
}

pub(in crate::root) const fn hardware_device_wallet_select_label(
    device_kind: HardwareDeviceKind,
) -> &'static str {
    match device_kind {
        HardwareDeviceKind::Ledger => "Ledger",
        HardwareDeviceKind::Trezor => "Trezor",
    }
}

pub(in crate::root) fn hardware_device_kind_from_wallet_select_value(
    value: &str,
) -> Option<HardwareDeviceKind> {
    match value {
        "hardware-device:ledger" => Some(HardwareDeviceKind::Ledger),
        "hardware-device:trezor" => Some(HardwareDeviceKind::Trezor),
        _ => None,
    }
}

pub(in crate::root) fn wallet_select_items_from_metadata(
    metadata: &[WalletMetadataBundle],
) -> Vec<WalletSelectItem> {
    let mut metadata = metadata.to_vec();
    metadata.retain(|metadata| metadata.status == wallet_ops::vault::WalletStatus::Active);
    sort_wallet_metadata(&mut metadata);

    let mut items = Vec::new();
    let mut ledger_added = false;
    let mut trezor_added = false;

    for metadata in metadata {
        let Some(device_kind) = hardware_device_kind_from_source(metadata.source) else {
            items.push(WalletSelectItem {
                wallet_id: Arc::from(metadata.wallet_uuid),
                label: Arc::from(metadata.label),
            });
            continue;
        };
        if metadata.hardware_account.is_none() {
            continue;
        }

        let added = match device_kind {
            HardwareDeviceKind::Ledger => &mut ledger_added,
            HardwareDeviceKind::Trezor => &mut trezor_added,
        };
        if *added {
            continue;
        }
        *added = true;
        items.push(WalletSelectItem {
            wallet_id: Arc::from(hardware_device_wallet_select_value(device_kind)),
            label: Arc::from(hardware_device_wallet_select_label(device_kind)),
        });
    }

    items
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::root) struct HardwareWalletDisplayInfo {
    pub(in crate::root) chip_label: String,
    pub(in crate::root) detail_label: String,
}

pub(in crate::root) fn hardware_wallet_display_info(
    wallet: &WalletMetadataBundle,
    active_profile: Option<&HardwareProfileMetadata>,
) -> Option<HardwareWalletDisplayInfo> {
    let account = wallet.hardware_account.as_ref()?;
    let device_kind = hardware_device_kind_from_source(wallet.source)?;
    let profile_label = hardware_wallet_profile_label(
        wallet,
        &account.profile_id,
        account.account_index,
        active_profile,
    );
    let compact_profile_label = compact_hardware_profile_label(&profile_label, device_kind);
    let renamed_account_label = renamed_hardware_account_label(wallet, account);
    let account_label = renamed_account_label
        .clone()
        .unwrap_or_else(|| hardware_account_index_label(account.account_index));
    let chip_label = format!("{compact_profile_label} / {account_label}");
    let detail_label = if let Some(renamed_account_label) = renamed_account_label {
        format!(
            "{}: {profile_label} / {renamed_account_label} account {}",
            hardware_device_wallet_select_label(device_kind),
            account.account_index
        )
    } else {
        format!(
            "{}: {profile_label} account {}",
            hardware_device_wallet_select_label(device_kind),
            account.account_index
        )
    };

    Some(HardwareWalletDisplayInfo {
        chip_label,
        detail_label,
    })
}

fn hardware_wallet_profile_label(
    wallet: &WalletMetadataBundle,
    profile_id: &str,
    account_index: u32,
    active_profile: Option<&HardwareProfileMetadata>,
) -> String {
    if let Some(profile) = active_profile.filter(|profile| profile.profile_id == profile_id) {
        return profile.label.clone();
    }

    let label = wallet
        .hardware_account
        .as_ref()
        .map_or(wallet.label.as_str(), |account| account.label.as_str());
    strip_hardware_account_suffix(label, account_index).to_owned()
}

fn renamed_hardware_account_label(
    wallet: &WalletMetadataBundle,
    account: &HardwareRailgunAccountMetadata,
) -> Option<String> {
    (wallet.label != account.label)
        .then(|| strip_hardware_account_suffix(&wallet.label, account.account_index).to_owned())
}

fn hardware_account_index_label(account_index: u32) -> String {
    format!("Account {account_index}")
}

fn strip_hardware_account_suffix(label: &str, account_index: u32) -> &str {
    let recovery_suffix = format!(" account {account_index} recovery");
    if let Some(profile_label) = label.strip_suffix(&recovery_suffix)
        && !profile_label.trim().is_empty()
    {
        return profile_label.trim();
    }

    let suffix = format!(" account {account_index}");
    if let Some(profile_label) = label.strip_suffix(&suffix)
        && !profile_label.trim().is_empty()
    {
        return profile_label.trim();
    }

    label
}

fn compact_hardware_profile_label(profile_label: &str, device_kind: HardwareDeviceKind) -> String {
    let default_profile_prefix = match device_kind {
        HardwareDeviceKind::Ledger => "Ledger hardware profile",
        HardwareDeviceKind::Trezor => "Trezor hardware profile",
    };

    if let Some(suffix) = profile_label.strip_prefix(default_profile_prefix) {
        let suffix = suffix.trim();
        return if suffix.is_empty() {
            "Profile".to_owned()
        } else {
            format!("Profile {suffix}")
        };
    }

    profile_label.to_owned()
}

pub(in crate::root) fn wallet_select_value_for_selected_wallet(
    wallet_id: &Arc<str>,
    metadata: &[WalletMetadataBundle],
) -> Arc<str> {
    metadata
        .iter()
        .find(|metadata| {
            metadata.status == wallet_ops::vault::WalletStatus::Active
                && metadata.wallet_uuid == wallet_id.as_ref()
        })
        .and_then(|metadata| {
            if metadata.hardware_account.is_some() {
                hardware_device_kind_from_source(metadata.source)
                    .map(hardware_device_wallet_select_value)
                    .map(Arc::from)
            } else {
                None
            }
        })
        .unwrap_or_else(|| Arc::clone(wallet_id))
}

pub(in crate::root::vault) const fn hardware_device_kind_from_source(
    source: WalletSource,
) -> Option<HardwareDeviceKind> {
    match source {
        WalletSource::LedgerDerived => Some(HardwareDeviceKind::Ledger),
        WalletSource::TrezorDerived => Some(HardwareDeviceKind::Trezor),
        WalletSource::Generated | WalletSource::Imported => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_ops::vault::WalletSoftwareContext;

    fn metadata(wallet_uuid: &str, context: Option<WalletSoftwareContext>) -> WalletMetadataBundle {
        WalletMetadataBundle {
            wallet_uuid: wallet_uuid.to_owned(),
            label: wallet_uuid.to_owned(),
            derivation_index: 0,
            source: WalletSource::Imported,
            status: wallet_ops::vault::WalletStatus::Active,
            display_order: 0,
            hardware_descriptor: None,
            hardware_account: None,
            pending_create_new_chain_ids: std::collections::BTreeSet::default(),
            software_context: context,
        }
    }

    #[test]
    fn concealed_passphrase_metadata_is_not_visible_without_an_open_context() {
        let standard = metadata("base", Some(WalletSoftwareContext::standard("base")));
        let child = metadata("child", Some(WalletSoftwareContext::passphrase("base")));
        let visible = visible_wallet_metadata(&[standard, child], None);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].wallet_uuid, "base");
    }

    #[test]
    fn only_the_selected_passphrase_context_is_revealed() {
        let standard = metadata("base", Some(WalletSoftwareContext::standard("base")));
        let first = metadata("first", Some(WalletSoftwareContext::passphrase("base")));
        let second = metadata("second", Some(WalletSoftwareContext::passphrase("base")));
        let visible = visible_wallet_metadata(&[standard, first, second], Some("second"));
        assert_eq!(
            visible
                .iter()
                .map(|metadata| metadata.wallet_uuid.as_str())
                .collect::<Vec<_>>(),
            vec!["base", "second"]
        );
    }

    #[test]
    fn passphrase_open_action_is_standard_only() {
        let standard = metadata("base", Some(WalletSoftwareContext::standard("base")));
        let child = metadata("child", Some(WalletSoftwareContext::passphrase("base")));

        assert!(passphrase_open_action_is_eligible(Some(&standard)));
        assert!(!passphrase_open_action_is_eligible(Some(&child)));
        assert!(!passphrase_open_action_is_eligible(None));
    }
}
