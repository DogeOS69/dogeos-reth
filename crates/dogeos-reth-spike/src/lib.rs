//! Dependency-resolution spike for the standalone DogeOS Reth migration.
//!
//! This crate deliberately contains no copied upstream Reth code and does not
//! claim to be a runnable node.  It is the first gate: Cargo must resolve one
//! Reth 2 / REVM 36 / Alloy 1.8 family together with the DogeOS-owned EVM
//! dependency before component integration starts.

/// The immutable Reth source revision selected for the Phase 0 spike.
pub const RETH_V2_REVISION: &str = "83fde18d01ed0ef6b7bf501280116b4babc69bef";

/// The immutable DogeOS REVM revision evaluated by the Phase 0 spike.
pub const DOGEOS_REVM_REVISION: &str = "1b87ecf17af029ac2f39e8ad362f3503ff2f4583";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_are_immutable_git_object_ids() {
        assert_eq!(RETH_V2_REVISION.len(), 40);
        assert_eq!(DOGEOS_REVM_REVISION.len(), 40);
        assert!(
            RETH_V2_REVISION
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(
            DOGEOS_REVM_REVISION
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }
}
