//! The public half of Strata's update signing key. Every download is checked
//! against it before it's unpacked (see `sig`). The private half never enters
//! this repository: it lives with the maintainer (and as a CI secret); see
//! docs/UPDATES.md for how to make, store and rotate it.
//!
//! Base64: wcG5rtEUOUMByjNONjqFCGK1Sbrl18ZJP3MAAckN44I=

pub const PUBLIC_KEY: [u8; 32] = [
    0xc1, 0xc1, 0xb9, 0xae, 0xd1, 0x14, 0x39, 0x43, 0x01, 0xca, 0x33, 0x4e, 0x36, 0x3a, 0x85, 0x08, 0x62, 0xb5, 0x49, 0xba, 0xe5, 0xd7, 0xc6, 0x49, 0x3f, 0x73, 0x00, 0x01, 0xc9, 0x0d, 0xe3, 0x82,
];

#[cfg(test)]
mod tests {
    #[test]
    fn the_key_is_a_valid_ed25519_point_and_matches_its_comment() {
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&super::PUBLIC_KEY).is_ok());
        assert_eq!(super::super::sig::b64_encode(&super::PUBLIC_KEY), "wcG5rtEUOUMByjNONjqFCGK1Sbrl18ZJP3MAAckN44I=");
    }
}
