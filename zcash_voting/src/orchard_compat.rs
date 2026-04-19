//! Byte-round-trip conversions between upstream orchard and valar-orchard types.
//!
//! valar-orchard is a pure superset of upstream orchard 0.11 — same wire
//! formats, additive governance-visibility methods only. Conversions here
//! serialize through the public byte APIs and cannot fail for well-formed
//! inputs. Delete this module (and the `orchard_upstream` dep) when the
//! governance-visibility changes land in zcash/orchard upstream.

pub fn fvk_upstream_to_valar(
    upstream: &orchard_upstream::keys::FullViewingKey,
) -> orchard::keys::FullViewingKey {
    let bytes = upstream.to_bytes();
    orchard::keys::FullViewingKey::from_bytes(&bytes)
        .expect("upstream fvk bytes are a valid valar-orchard fvk (identical wire format)")
}

pub fn sk_upstream_to_valar(
    upstream: &orchard_upstream::keys::SpendingKey,
) -> orchard::keys::SpendingKey {
    let bytes = *upstream.to_bytes();
    Option::from(orchard::keys::SpendingKey::from_bytes(bytes))
        .expect("upstream sk bytes are a valid valar-orchard sk (identical wire format)")
}
