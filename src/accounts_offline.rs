use md5::{Digest, Md5};

use crate::core::error::{Error, Result};

/// An offline player: a name the server maps to a stable UUID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfflineAccount {
    pub name: String,
    pub uuid: String,
}

impl OfflineAccount {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if !is_valid_name(&name) {
            return Err(Error::InvalidName(name));
        }
        let uuid = uuid_for(&name);
        Ok(Self { name, uuid })
    }
}

/// 3-16 ASCII letters, digits, or underscore.
pub fn is_valid_name(name: &str) -> bool {
    (3..=16).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// UUID v3 of `OfflinePlayer:{name}`, as Java's `UUID.nameUUIDFromBytes` builds it.
pub fn uuid_for(name: &str) -> String {
    let mut digest = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());

    // Version 3, IETF variant.
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;

    let hex = hex::encode(digest);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_matches_java_offline_players() {
        assert_eq!(
            uuid_for("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert_eq!(
            uuid_for("Alex"),
            "36532b5e-c442-3dbb-a24c-c7e55d0f979a"
        );
        assert_eq!(
            uuid_for("Steve"),
            "5627dd98-e6be-3c21-b8a8-e92344183641"
        );
    }

    #[test]
    fn uuid_is_stable_and_has_version_3_variant() {
        let first = uuid_for("Player_1");
        assert_eq!(first, uuid_for("Player_1"));
        assert_eq!(&first[14..15], "3");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn names_are_3_to_16_alphanumeric_or_underscore() {
        for valid in ["abc", "Alex_2", "A123456789012345"] {
            assert!(is_valid_name(valid), "{valid} was rejected");
        }
        for invalid in [
            "",
            "ab",
            "A1234567890123456",
            "has space",
            "hyphen-name",
            "acentüado",
            "with.dot",
        ] {
            assert!(!is_valid_name(invalid), "{invalid} was accepted");
        }
    }

    #[test]
    fn account_keeps_name_and_uuid() {
        let account = OfflineAccount::new("Alex").unwrap();
        assert_eq!(account.name, "Alex");
        assert_eq!(account.uuid, "36532b5e-c442-3dbb-a24c-c7e55d0f979a");
        assert!(OfflineAccount::new("ab").is_err());
    }
}
