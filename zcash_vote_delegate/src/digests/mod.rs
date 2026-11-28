//! The five proxy delegation digests (contract §4).
//!
//! D1 is signed with RedPallas under each DC's `r_vpk`. The others are signed
//! with Ed25519 as the 32-byte message. Each is a
//! [`DigestPreimage`](crate::DigestPreimage) under its own domain. The domains
//! and the three existing `SVOTE_` sighash domains are prefix-free together, so
//! no preimage of one digest can be read as another's.

mod batch_sighash;
mod registry;

pub use batch_sighash::{
    proxy_batch_digest, proxy_batch_sighash, ProxyDelegationActionFields,
    ProxyDelegationBatchFields,
};
pub use registry::{
    attest_digest, ballot_digest, register_digest, retire_digest, AttestationWindow,
    DelegateBallotFields, DelegateRegisterFields, DelegateRetireFields, IdentityProvider,
};

/// Domain of D1, the 0x09 batch sighash (§4.1).
pub const PROXY_DELEGATION_BATCH_SIGHASH_DOMAIN: &str = "SVOTE_PROXY_DELEGATION_BATCH_SIGHASH_V1";
/// Domain of REGISTER, signed by the delegate key (§4.2).
pub const DELEGATE_REGISTER_DOMAIN: &str = "SVOTE_PROXY_DELEGATE_REGISTER_V1";
/// Domain of ATTEST, signed by a verifier's params key (§4.2).
pub const DELEGATE_ATTEST_DOMAIN: &str = "SVOTE_PROXY_DELEGATE_ATTEST_V1";
/// Domain of RETIRE, signed by the retiring entry's key (§4.2).
pub const DELEGATE_RETIRE_DOMAIN: &str = "SVOTE_PROXY_DELEGATE_RETIRE_V1";
/// Domain of BALLOT, signed by the delegate entry's key (§4.2).
pub const DELEGATE_BALLOT_DOMAIN: &str = "SVOTE_PROXY_DELEGATE_BALLOT_V1";

/// Every domain this crate hashes under, in contract order.
pub const PROXY_DELEGATION_DOMAINS: [&str; 5] = [
    PROXY_DELEGATION_BATCH_SIGHASH_DOMAIN,
    DELEGATE_REGISTER_DOMAIN,
    DELEGATE_ATTEST_DOMAIN,
    DELEGATE_RETIRE_DOMAIN,
    DELEGATE_BALLOT_DOMAIN,
];
