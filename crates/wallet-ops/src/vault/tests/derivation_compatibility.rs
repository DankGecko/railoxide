use super::super::*;

struct DerivationFixture {
    name: &'static str,
    mnemonic: &'static str,
    passphrase: &'static str,
    railgun_index: u32,
    evm_index: u32,
    seed: &'static str,
    spending_private_key: &'static str,
    spending_public_key: [&'static str; 2],
    viewing_private_key: &'static str,
    viewing_public_key: &'static str,
    railgun_address: &'static str,
    evm_private_key: &'static str,
    evm_address: &'static str,
}

// These vectors were independently calculated with the installed JavaScript
// RAILGUN engine.
const FIXTURES: &[DerivationFixture] = &[
    DerivationFixture {
        name: "empty-passphrase",
        mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        passphrase: "",
        railgun_index: 0,
        evm_index: 7,
        seed: "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4",
        spending_private_key: "08b2d974aa7fffd9d068b78c34434c534ddcd9343fcbf5aa12cf78e1a3c1ccb9",
        spending_public_key: [
            "3008064177791584c9378d04a8f382f43195f76d3fd6f758a50076dcd392ae4c",
            "2834610a1ec9e739a664edc0c8eb0839065e2debfbc592d5e75e3c978bcc29a0",
        ],
        viewing_private_key: "9a9e1ca3b9476dc8500b43f30f34104c92a3eedfd727757ffd0ad15da8e11572",
        viewing_public_key: "df2dfb942aa6fb8cf9fe60d7984cd10b20b59027e677ecb4960d764f7d42408a",
        railgun_address: "0zk1qy4v02p5zkq0zfpaxhz79j5tslrv8c44d80d8jr2fuecrtxlp8lemrv7j6fe3z53ll0jm7u592n0hr8elesd0xzv6y9jpdvsyln80m95jcxhvnmagfqg5p6e9mp",
        evm_private_key: "dfb0930bcb8f6ca83296c1870e941998c641d3d0d413013c890b8b255dd537b5",
        evm_address: "0x593814d3309e2dF31D112824F0bb5aa7Cb0D7d47",
    },
    DerivationFixture {
        name: "non-empty-passphrase",
        mnemonic: "legal winner thank year wave sausage worth useful legal winner thank yellow",
        passphrase: "TREZOR",
        railgun_index: 5,
        evm_index: 0,
        seed: "2e8905819b8723fe2c1d161860e5ee1830318dbf49a83bd451cfb8440c28bd6fa457fe1296106559a3c80937a1c1069be3a3a5bd381ee6260e8d9739fce1f607",
        spending_private_key: "a595eba9ad3372147eac1513457ca7b1be153504d9d892ef0655989cefb40c0b",
        spending_public_key: [
            "2091f0d0c31465bf8db83473c931cf3262b2326cc3308f4a9ecb1026b759d8a1",
            "27d2004232fa1ef788a33759e8e4eef55ee1aee58b80280f7b6a517d6676e0f8",
        ],
        viewing_private_key: "5ce5a4d9ba0c2679e35bbd6a810b64380551c58b074a684fb8d47ce9509425e2",
        viewing_public_key: "86e7ee34df11fdfb5695098f4eba80a7223c3236c19c465b0fa9d68aba9fd57e",
        railgun_address: "0zk1qyvyp4w852nzhxn4j0znfsuu69cwffadmnda0hpflh272vvjrdvadrv7j6fe3z53l7rw0m35muglm76kj5yc7n46sznjy0pjxmqec3jmp75adz46nl2hu6v0zlz",
        evm_private_key: "9f20bfeef91877e3c5f50fc0557a80d25f77a650c83d47601a8193bccb0e678a",
        evm_address: "0x6006ef1944FB519A746d00cDAf715Cbd27a5a008",
    },
    DerivationFixture {
        name: "unicode-nfkd-passphrase",
        mnemonic: "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        passphrase: "e\u{301}clair",
        railgun_index: 2,
        evm_index: 11,
        seed: "8549062ee2738b52903bc34caa647ed78861681fd53528ffbba1b38b61878d9a87ea32b5f568aaa58c788557077133755abd44d29adfd6617fee110c687e8be2",
        spending_private_key: "194fc1ef6ba0ff9e03e50d9ad0b19d13cf085ef0974f11fc208b62259b150afe",
        spending_public_key: [
            "17582b11f442445b9e70f08057cd6b7b1230c93980872289320a0aa4124414fa",
            "10b5bb82a64f538b560e23f3dfa35eacaa7eca30d129052f3f784638f82cefc9",
        ],
        viewing_private_key: "20512a0aff955399eff4cfcba6fb3ae3331af34e3f395efd40a94f78e6dd4bb9",
        viewing_public_key: "adcf8892a5bd85391265dbf49a2e61c0957308f67fcefce257f50362a777d895",
        railgun_address: "0zk1qy8e8g87dry5xg08qvevcux9wle7h9gt5mgt4wa6qdwjacch0j0xtrv7j6fe3z53l7kulzyj5k7c2wgjvhdlfx3wv8qf2ucg7elual8z2l6sxc48wlvf24vues4",
        evm_private_key: "86d6a8ecce79da442034aee7599a02271360bad9dd804015591ae49647b0f960",
        evm_address: "0x474e28a2F6A0de93BacCc53C8456E96ff6CA6692",
    },
];

fn assert_hex(label: &str, actual: &[u8], expected: &str) {
    let actual = alloy::hex::encode(actual);
    assert!(actual == expected, "{label} fixture mismatch");
}

#[test]
fn bip39_railgun_and_evm_vectors_match() {
    for fixture in FIXTURES {
        let seed = bip39_seed_from_mnemonic(fixture.mnemonic, fixture.passphrase)
            .expect("derive fixture seed");
        assert_hex(fixture.name, &*seed, fixture.seed);
        assert!(seed.len() == 64, "{} seed length mismatch", fixture.name);

        let wallet =
            wallet_keys_from_mnemonic(fixture.mnemonic, fixture.passphrase, fixture.railgun_index)
                .expect("derive fixture RAILGUN keys");
        assert_hex(
            "RAILGUN spending private key",
            &wallet.spending_private_key,
            fixture.spending_private_key,
        );
        for (actual, expected) in wallet
            .spending_public_key
            .iter()
            .zip(fixture.spending_public_key)
        {
            assert_hex(
                "RAILGUN spending public key",
                &actual.to_be_bytes::<KEY_LEN>(),
                expected,
            );
        }
        assert_hex(
            "RAILGUN viewing private key",
            &wallet.viewing.viewing_private_key,
            fixture.viewing_private_key,
        );
        assert_hex(
            "RAILGUN viewing public key",
            &wallet.viewing.viewing_public_key,
            fixture.viewing_public_key,
        );
        let railgun_address = wallet
            .viewing
            .derive_address(None)
            .expect("derive fixture RAILGUN address")
            .to_string();
        assert!(
            railgun_address == fixture.railgun_address,
            "{} RAILGUN address fixture mismatch",
            fixture.name
        );

        let evm_private_key = derive_public_evm_private_key_from_mnemonic_with_passphrase(
            fixture.mnemonic,
            fixture.passphrase,
            fixture.evm_index,
        )
        .expect("derive fixture EVM key");
        assert_hex(
            "EVM private key",
            &*evm_private_key,
            fixture.evm_private_key,
        );
        let evm_address = derive_public_evm_address_from_mnemonic_with_passphrase(
            fixture.mnemonic,
            fixture.passphrase,
            fixture.evm_index,
        )
        .expect("derive fixture EVM address")
        .to_string();
        assert!(
            evm_address == fixture.evm_address,
            "{} EVM address fixture mismatch",
            fixture.name
        );
    }
}

#[test]
fn passphrase_preserves_exact_text_and_nfkd_equivalence() {
    let mnemonic = FIXTURES[1].mnemonic;
    let exact = bip39_seed_from_mnemonic(mnemonic, "TREZOR").expect("derive exact seed");
    let case_changed = bip39_seed_from_mnemonic(mnemonic, "trezor").expect("derive case seed");
    let whitespace_changed =
        bip39_seed_from_mnemonic(mnemonic, " TREZOR").expect("derive whitespace seed");
    assert!(*exact != *case_changed, "passphrase case was folded");
    assert!(
        *exact != *whitespace_changed,
        "passphrase whitespace was trimmed"
    );

    let composed = bip39_seed_from_mnemonic(FIXTURES[2].mnemonic, "\u{e9}clair")
        .expect("derive composed Unicode seed");
    let decomposed = bip39_seed_from_mnemonic(FIXTURES[2].mnemonic, FIXTURES[2].passphrase)
        .expect("derive decomposed Unicode seed");
    assert!(
        *composed == *decomposed,
        "passphrase NFKD normalization mismatch"
    );
}
