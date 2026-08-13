//! Serde helpers for fixed-size byte arrays larger than serde's built-in
//! 32-element array impls.

pub mod byte_array {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(
        value: &[u8; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[u8; N], D::Error> {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        let len = bytes.len();
        bytes
            .try_into()
            .map_err(|_| D::Error::invalid_length(len, &"fixed-size byte array"))
    }
}
