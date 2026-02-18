use std::io::Read;

use incrementalmerkletree::frontier::CommitmentTree;
use orchard::tree::MerkleHashOrchard;

/// Compute the Orchard note commitment tree root from a lightwalletd frontier hex string.
///
/// The hex string is the `orchard_tree` field from a lightwalletd `TreeState` response.
/// It encodes a legacy `CommitmentTree` using the Zcash serialization format:
///   Optional<Node> left, Optional<Node> right, CompactSize + Vec<Optional<Node>> parents
///
/// Each node is 32 bytes (Pallas base field element, little-endian).
pub fn root_from_frontier_hex(hex_str: &str) -> anyhow::Result<[u8; 32]> {
    if hex_str.is_empty() {
        return Ok(CommitmentTree::<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>::empty()
            .root()
            .to_bytes());
    }

    let bytes = hex::decode(hex_str)?;
    let mut cursor = &bytes[..];

    let left = read_optional_hash(&mut cursor)?;
    let right = read_optional_hash(&mut cursor)?;
    let count = read_compact_size(&mut cursor)?;
    let parents: Vec<Option<MerkleHashOrchard>> = (0..count)
        .map(|_| read_optional_hash(&mut cursor))
        .collect::<Result<_, _>>()?;

    let tree = CommitmentTree::<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>::from_parts(
        left, right, parents,
    )
    .map_err(|_| anyhow::anyhow!("invalid commitment tree: parents exceed tree depth"))?;

    Ok(tree.root().to_bytes())
}

fn read_optional_hash(reader: &mut &[u8]) -> anyhow::Result<Option<MerkleHashOrchard>> {
    let mut flag = [0u8; 1];
    reader.read_exact(&mut flag)?;
    match flag[0] {
        0 => Ok(None),
        1 => {
            let mut buf = [0u8; 32];
            reader.read_exact(&mut buf)?;
            let hash = <Option<MerkleHashOrchard>>::from(MerkleHashOrchard::from_bytes(&buf))
                .ok_or_else(|| anyhow::anyhow!("non-canonical Pallas base field element in tree node"))?;
            Ok(Some(hash))
        }
        _ => Err(anyhow::anyhow!("invalid optional flag byte: {}", flag[0])),
    }
}

/// Read a Bitcoin-style CompactSize unsigned integer.
fn read_compact_size(reader: &mut &[u8]) -> anyhow::Result<usize> {
    let mut b = [0u8; 1];
    reader.read_exact(&mut b)?;
    match b[0] {
        n @ 0..=252 => Ok(n as usize),
        253 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            Ok(u16::from_le_bytes(buf) as usize)
        }
        254 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(u32::from_le_bytes(buf) as usize)
        }
        255 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            Ok(u64::from_le_bytes(buf) as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frontier_returns_empty_root() {
        let root = root_from_frontier_hex("").unwrap();
        let expected = CommitmentTree::<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>::empty()
            .root()
            .to_bytes();
        assert_eq!(root, expected);
    }

    #[test]
    fn invalid_hex_returns_error() {
        assert!(root_from_frontier_hex("zzzz").is_err());
    }

    #[test]
    fn truncated_data_returns_error() {
        assert!(root_from_frontier_hex("01").is_err());
    }
}
