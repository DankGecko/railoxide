use alloy::primitives::{I256, Signature, U256};
use protobuf::MessageField;
use trezor_client::protos::MessageType;

use super::super::{
    HardwareAppVersion, HardwareDerivationError, HardwareDeviceKind,
    HardwarePublicAccountDescriptor, HardwareTypedDataSigningMode,
};
use super::client::{TrezorDeviceInfo, TrezorHardwareDerivationClient};
use super::transaction::trezor_signature_to_alloy;
use super::transport::{trezor_call_raw_with_interactions, trezor_decode_message};
use crate::hardware_typed_data::{
    HardwareEip712FieldDefinition, HardwareEip712Model, HardwareEip712PrimitiveType,
    HardwareEip712StructValue, HardwareEip712Type, HardwareEip712Value,
};

const TREZOR_CLEAR_TYPED_DATA_MIN_VERSION: HardwareAppVersion = HardwareAppVersion::new(2, 4, 3);
const TREZOR_HASH_TYPED_DATA_MIN_VERSION: HardwareAppVersion = HardwareAppVersion::new(1, 11, 2);

pub(super) type TrezorEthereumDataType =
    trezor_client::protos::ethereum_typed_data_struct_ack::EthereumDataType;
type TrezorEthereumFieldType =
    trezor_client::protos::ethereum_typed_data_struct_ack::EthereumFieldType;
type TrezorEthereumStructMember =
    trezor_client::protos::ethereum_typed_data_struct_ack::EthereumStructMember;

impl TrezorHardwareDerivationClient {
    #[allow(dead_code)]
    pub(crate) fn sign_typed_data_clear(
        &mut self,
        descriptor: &HardwarePublicAccountDescriptor,
        typed_data: &HardwareEip712Model,
    ) -> Result<Signature, HardwareDerivationError> {
        descriptor.validate()?;
        if descriptor.device_kind != HardwareDeviceKind::Trezor {
            return Err(HardwareDerivationError::InvalidDescriptor(
                "Trezor typed-data signing requires a Trezor descriptor",
            ));
        }
        let result = self.sign_typed_data_clear_inner(descriptor, typed_data);
        self.passphrase.clear_app_passphrase();
        result
    }

    fn sign_typed_data_clear_inner(
        &mut self,
        descriptor: &HardwarePublicAccountDescriptor,
        typed_data: &HardwareEip712Model,
    ) -> Result<Signature, HardwareDerivationError> {
        let mut request = trezor_client::protos::EthereumSignTypedData::new();
        request.address_n.clone_from(&descriptor.path);
        request.set_primary_type(typed_data.primary_type().to_owned());
        request.set_metamask_v4_compat(true);

        let Self {
            client,
            passphrase,
            pin_matrix_provider,
        } = self;
        let mut response = trezor_call_raw_with_interactions(
            client,
            request,
            passphrase,
            pin_matrix_provider.as_ref(),
        )?;
        loop {
            match response.message_type() {
                MessageType::MessageType_EthereumTypedDataStructRequest => {
                    let request: trezor_client::protos::EthereumTypedDataStructRequest =
                        trezor_decode_message(response)?;
                    let ack = trezor_typed_data_struct_ack(typed_data, request.name())?;
                    response = trezor_call_raw_with_interactions(
                        client,
                        ack,
                        passphrase,
                        pin_matrix_provider.as_ref(),
                    )?;
                }
                MessageType::MessageType_EthereumTypedDataValueRequest => {
                    let request: trezor_client::protos::EthereumTypedDataValueRequest =
                        trezor_decode_message(response)?;
                    let ack = trezor_typed_data_value_ack(typed_data, &request.member_path)?;
                    response = trezor_call_raw_with_interactions(
                        client,
                        ack,
                        passphrase,
                        pin_matrix_provider.as_ref(),
                    )?;
                }
                MessageType::MessageType_EthereumTypedDataSignature => {
                    let signature: trezor_client::protos::EthereumTypedDataSignature =
                        trezor_decode_message(response)?;
                    return trezor_typed_data_signature_to_alloy(&signature);
                }
                message_type => {
                    return Err(trezor_client::Error::UnexpectedMessageType(message_type).into());
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn sign_typed_data_hash(
        &mut self,
        descriptor: &HardwarePublicAccountDescriptor,
        typed_data: &HardwareEip712Model,
    ) -> Result<Signature, HardwareDerivationError> {
        descriptor.validate()?;
        if descriptor.device_kind != HardwareDeviceKind::Trezor {
            return Err(HardwareDerivationError::InvalidDescriptor(
                "Trezor typed-data signing requires a Trezor descriptor",
            ));
        }
        let result = self.sign_typed_data_hash_inner(descriptor, typed_data);
        self.passphrase.clear_app_passphrase();
        result
    }

    fn sign_typed_data_hash_inner(
        &mut self,
        descriptor: &HardwarePublicAccountDescriptor,
        typed_data: &HardwareEip712Model,
    ) -> Result<Signature, HardwareDerivationError> {
        let mut request = trezor_typed_data_hash_request(typed_data);
        request.address_n.clone_from(&descriptor.path);
        let Self {
            client,
            passphrase,
            pin_matrix_provider,
        } = self;
        let response = trezor_call_raw_with_interactions(
            client,
            request,
            passphrase,
            pin_matrix_provider.as_ref(),
        )?;
        if response.message_type() != MessageType::MessageType_EthereumTypedDataSignature {
            return Err(
                trezor_client::Error::UnexpectedMessageType(response.message_type()).into(),
            );
        }
        let signature: trezor_client::protos::EthereumTypedDataSignature =
            trezor_decode_message(response)?;
        trezor_typed_data_signature_to_alloy(&signature)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrezorModelKind {
    ModelOne,
    CoreFirmware,
    Unknown,
}

fn trezor_model_kind(model: &str) -> TrezorModelKind {
    let normalized = model
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized == "1" || normalized.contains("modelone") || normalized.starts_with("t1") {
        TrezorModelKind::ModelOne
    } else if normalized == "t"
        || normalized.contains("modelt")
        || normalized.contains("safe")
        || normalized.starts_with("t2")
        || normalized.starts_with("t3")
    {
        TrezorModelKind::CoreFirmware
    } else {
        TrezorModelKind::Unknown
    }
}

#[must_use]
pub fn classify_trezor_typed_data_signing_mode(
    info: &TrezorDeviceInfo,
) -> HardwareTypedDataSigningMode {
    if !info.initialized || info.bootloader_mode {
        return HardwareTypedDataSigningMode::Unsupported;
    }
    match trezor_model_kind(&info.model) {
        TrezorModelKind::CoreFirmware if info.version >= TREZOR_CLEAR_TYPED_DATA_MIN_VERSION => {
            HardwareTypedDataSigningMode::ClearSign
        }
        TrezorModelKind::ModelOne if info.version >= TREZOR_HASH_TYPED_DATA_MIN_VERSION => {
            HardwareTypedDataSigningMode::Eip712HashFallback
        }
        TrezorModelKind::ModelOne | TrezorModelKind::CoreFirmware | TrezorModelKind::Unknown => {
            HardwareTypedDataSigningMode::Unsupported
        }
    }
}

pub(super) fn trezor_typed_data_struct_ack(
    typed_data: &HardwareEip712Model,
    type_name: &str,
) -> Result<trezor_client::protos::EthereumTypedDataStructAck, HardwareDerivationError> {
    let fields = typed_data.type_definitions().get(type_name).ok_or(
        HardwareDerivationError::UnexpectedHardwareResponse(
            "Trezor requested an unknown EIP-712 struct definition",
        ),
    )?;
    let mut ack = trezor_client::protos::EthereumTypedDataStructAck::new();
    ack.members = fields
        .iter()
        .map(|field| trezor_typed_data_struct_member(typed_data, field))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ack)
}

fn trezor_typed_data_struct_member(
    typed_data: &HardwareEip712Model,
    field: &HardwareEip712FieldDefinition,
) -> Result<TrezorEthereumStructMember, HardwareDerivationError> {
    let mut member = TrezorEthereumStructMember::new();
    member.type_ = MessageField::some(trezor_typed_data_field_type(typed_data, &field.value_type)?);
    member.set_name(field.name.clone());
    Ok(member)
}

fn trezor_typed_data_field_type(
    typed_data: &HardwareEip712Model,
    value_type: &HardwareEip712Type,
) -> Result<TrezorEthereumFieldType, HardwareDerivationError> {
    let mut field = TrezorEthereumFieldType::new();
    match value_type {
        HardwareEip712Type::Primitive(primitive) => {
            trezor_set_primitive_field_type(&mut field, primitive)?;
        }
        HardwareEip712Type::Struct(name) => {
            let fields = typed_data.type_definitions().get(name).ok_or(
                HardwareDerivationError::UnexpectedHardwareResponse(
                    "Trezor EIP-712 struct type is missing a definition",
                ),
            )?;
            field.set_data_type(TrezorEthereumDataType::STRUCT);
            field.set_size(u32::try_from(fields.len()).map_err(|_| {
                HardwareDerivationError::InvalidDescriptor(
                    "Trezor EIP-712 struct field count is too large",
                )
            })?);
            field.set_struct_name(name.clone());
        }
        HardwareEip712Type::DynamicArray(element) => {
            field.set_data_type(TrezorEthereumDataType::ARRAY);
            field.entry_type =
                MessageField::some(trezor_typed_data_field_type(typed_data, element)?);
        }
        HardwareEip712Type::FixedArray { element, len } => {
            field.set_data_type(TrezorEthereumDataType::ARRAY);
            field.set_size(u32::try_from(*len).map_err(|_| {
                HardwareDerivationError::InvalidDescriptor(
                    "Trezor EIP-712 array length is too large",
                )
            })?);
            field.entry_type =
                MessageField::some(trezor_typed_data_field_type(typed_data, element)?);
        }
    }
    Ok(field)
}

fn trezor_set_primitive_field_type(
    field: &mut TrezorEthereumFieldType,
    primitive: &HardwareEip712PrimitiveType,
) -> Result<(), HardwareDerivationError> {
    match primitive {
        HardwareEip712PrimitiveType::Bool => field.set_data_type(TrezorEthereumDataType::BOOL),
        HardwareEip712PrimitiveType::Int(bits) => {
            field.set_data_type(TrezorEthereumDataType::INT);
            field.set_size(trezor_integer_byte_size(*bits)?);
        }
        HardwareEip712PrimitiveType::Uint(bits) => {
            field.set_data_type(TrezorEthereumDataType::UINT);
            field.set_size(trezor_integer_byte_size(*bits)?);
        }
        HardwareEip712PrimitiveType::Address => {
            field.set_data_type(TrezorEthereumDataType::ADDRESS);
        }
        HardwareEip712PrimitiveType::FixedBytes(size) => {
            field.set_data_type(TrezorEthereumDataType::BYTES);
            field.set_size(u32::try_from(*size).map_err(|_| {
                HardwareDerivationError::InvalidDescriptor(
                    "Trezor EIP-712 fixed bytes size is too large",
                )
            })?);
        }
        HardwareEip712PrimitiveType::Bytes => {
            field.set_data_type(TrezorEthereumDataType::BYTES);
        }
        HardwareEip712PrimitiveType::String => {
            field.set_data_type(TrezorEthereumDataType::STRING);
        }
    }
    Ok(())
}

fn trezor_typed_data_value_ack(
    typed_data: &HardwareEip712Model,
    member_path: &[u32],
) -> Result<trezor_client::protos::EthereumTypedDataValueAck, HardwareDerivationError> {
    let mut ack = trezor_client::protos::EthereumTypedDataValueAck::new();
    ack.set_value(trezor_typed_data_value(typed_data, member_path)?);
    Ok(ack)
}

enum TrezorTypedDataValueRef<'a> {
    Struct(&'a HardwareEip712StructValue),
    Value(&'a HardwareEip712Value),
}

pub(super) fn trezor_typed_data_value(
    typed_data: &HardwareEip712Model,
    member_path: &[u32],
) -> Result<Vec<u8>, HardwareDerivationError> {
    let Some((&root, path)) = member_path.split_first() else {
        return Err(HardwareDerivationError::UnexpectedHardwareResponse(
            "Trezor requested an empty EIP-712 member path",
        ));
    };
    let mut value = match root {
        0 => TrezorTypedDataValueRef::Struct(typed_data.domain()),
        1 => TrezorTypedDataValueRef::Struct(typed_data.message().ok_or(
            HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested EIP-712 message data for a domain-only payload",
            ),
        )?),
        _ => {
            return Err(HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested an invalid EIP-712 root member path",
            ));
        }
    };
    for index in path {
        value = trezor_typed_data_descend(
            &value,
            usize::try_from(*index).map_err(|_| {
                HardwareDerivationError::InvalidDescriptor(
                    "Trezor EIP-712 member path is too large",
                )
            })?,
        )?;
    }
    trezor_typed_data_encode_value(&value)
}

fn trezor_typed_data_descend<'a>(
    value: &TrezorTypedDataValueRef<'a>,
    index: usize,
) -> Result<TrezorTypedDataValueRef<'a>, HardwareDerivationError> {
    match value {
        TrezorTypedDataValueRef::Struct(value) => value
            .fields
            .get(index)
            .map(|field| TrezorTypedDataValueRef::Value(&field.value))
            .ok_or(HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested an out-of-range EIP-712 struct field",
            )),
        TrezorTypedDataValueRef::Value(HardwareEip712Value::Struct(value)) => value
            .fields
            .get(index)
            .map(|field| TrezorTypedDataValueRef::Value(&field.value))
            .ok_or(HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested an out-of-range EIP-712 nested struct field",
            )),
        TrezorTypedDataValueRef::Value(
            HardwareEip712Value::DynamicArray(values) | HardwareEip712Value::FixedArray(values),
        ) => values
            .get(index)
            .map(|value| TrezorTypedDataValueRef::Value(&value.value))
            .ok_or(HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested an out-of-range EIP-712 array element",
            )),
        TrezorTypedDataValueRef::Value(_) => {
            Err(HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested a nested EIP-712 value from a primitive field",
            ))
        }
    }
}

fn trezor_typed_data_encode_value(
    value: &TrezorTypedDataValueRef<'_>,
) -> Result<Vec<u8>, HardwareDerivationError> {
    match value {
        TrezorTypedDataValueRef::Value(HardwareEip712Value::Bool(value)) => {
            Ok(vec![u8::from(*value)])
        }
        TrezorTypedDataValueRef::Value(HardwareEip712Value::Int { value, bits }) => {
            Ok(i256_to_trezor_fixed(value, *bits)?)
        }
        TrezorTypedDataValueRef::Value(HardwareEip712Value::Uint { value, bits }) => {
            Ok(u256_to_trezor_fixed(*value, *bits)?)
        }
        TrezorTypedDataValueRef::Value(HardwareEip712Value::Address(value)) => {
            Ok(value.as_slice().to_vec())
        }
        TrezorTypedDataValueRef::Value(
            HardwareEip712Value::FixedBytes { bytes, .. } | HardwareEip712Value::Bytes(bytes),
        ) => Ok(bytes.clone()),
        TrezorTypedDataValueRef::Value(HardwareEip712Value::String(value)) => {
            Ok(value.as_bytes().to_vec())
        }
        TrezorTypedDataValueRef::Value(
            HardwareEip712Value::DynamicArray(values) | HardwareEip712Value::FixedArray(values),
        ) => Ok(u16::try_from(values.len())
            .map_err(|_| {
                HardwareDerivationError::InvalidDescriptor("Trezor EIP-712 array is too large")
            })?
            .to_be_bytes()
            .to_vec()),
        TrezorTypedDataValueRef::Struct(_)
        | TrezorTypedDataValueRef::Value(HardwareEip712Value::Struct(_)) => {
            Err(HardwareDerivationError::UnexpectedHardwareResponse(
                "Trezor requested an EIP-712 struct as a raw value",
            ))
        }
    }
}

fn trezor_typed_data_hash_request(
    typed_data: &HardwareEip712Model,
) -> trezor_client::protos::EthereumSignTypedHash {
    let mut request = trezor_client::protos::EthereumSignTypedHash::new();
    request.set_domain_separator_hash(typed_data.domain_separator_hash().as_slice().to_vec());
    if let Some(message_hash) = typed_data.message_hash() {
        request.set_message_hash(message_hash.as_slice().to_vec());
    }
    request
}

fn trezor_typed_data_signature_to_alloy(
    signature: &trezor_client::protos::EthereumTypedDataSignature,
) -> Result<Signature, HardwareDerivationError> {
    let signature = signature.signature();
    if signature.len() != 65 {
        return Err(HardwareDerivationError::UnexpectedResponseLength {
            got: signature.len(),
            expected: 65,
        });
    }
    let r = signature
        .get(0..32)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(HardwareDerivationError::UnexpectedResponseLength {
            got: signature.len(),
            expected: 65,
        })?;
    let s = signature
        .get(32..64)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(HardwareDerivationError::UnexpectedResponseLength {
            got: signature.len(),
            expected: 65,
        })?;
    trezor_signature_to_alloy(trezor_client::client::Signature {
        r,
        s,
        v: u64::from(signature[64]),
    })
}

fn trezor_integer_byte_size(bits: usize) -> Result<u32, HardwareDerivationError> {
    if bits == 0 || bits > 256 || !bits.is_multiple_of(8) {
        return Err(HardwareDerivationError::InvalidDescriptor(
            "Trezor EIP-712 integer size is invalid",
        ));
    }
    Ok(u32::try_from(bits / 8).expect("EIP-712 integer byte size fits in u32"))
}

fn u256_to_trezor_fixed(value: U256, bits: usize) -> Result<Vec<u8>, HardwareDerivationError> {
    let byte_len = usize::try_from(trezor_integer_byte_size(bits)?)
        .expect("EIP-712 integer byte size fits in usize");
    let bytes = value.to_be_bytes::<32>();
    Ok(bytes[bytes.len() - byte_len..].to_vec())
}

fn i256_to_trezor_fixed(value: &I256, bits: usize) -> Result<Vec<u8>, HardwareDerivationError> {
    let byte_len = usize::try_from(trezor_integer_byte_size(bits)?)
        .expect("EIP-712 integer byte size fits in usize");
    let bytes = value.to_be_bytes::<32>();
    Ok(bytes[bytes.len() - byte_len..].to_vec())
}
