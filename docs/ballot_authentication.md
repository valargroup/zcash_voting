# Ballot attestation

A round entry in the dynamic voting config may carry a `ballot_attestation`
that binds the round's displayed ballot content to the trusted config keys.
Wallets recompute the digest of the ballot a vote server serves and compare it
with the attested digest before displaying it.

This document specifies the digest and the attestation so that signing tools
and wallets in any language produce identical bytes. Test vectors are in
[`zcash_voting/test-vectors/ballot_digest_v1.json`](../zcash_voting/test-vectors/ballot_digest_v1.json).

## Ballot content

The ballot is the round content a wallet displays before the voter chooses,
taken from the chain's `VoteRound` and `Proposal` messages:

| Level | Fields, in encoding order |
| --- | --- |
| Round | `title`, `description`, `discussion_url` |
| Proposal | `id`, `title`, `description`, `zip_number`, `forum_url`, `options` |
| Option | `index`, `label`, `description` |

Proposals and options are encoded in the order the round lists them, which is
the order wallets display them. An absent string field is the empty string, as
in proto3. Strings are encoded as their exact UTF-8 bytes, with no trimming,
case folding, or Unicode normalization.

## Digest (`zcash-shielded-vote:ballot:v1`)

```text
ballot_digest = SHA-256(
    "zcash-shielded-vote:ballot:v1"
    || str(round.title) || str(round.description) || str(round.discussion_url)
    || u64(len(proposals))
    || for each proposal:
         u32(id) || str(title) || str(description) || str(zip_number)
         || str(forum_url) || u64(len(options))
         || for each option: u32(index) || str(label) || str(description)
)

str(s) = u64(byte length of s) || UTF-8 bytes of s
```

All integers are little-endian. The domain tag is the 29 ASCII bytes shown,
with no length prefix or terminator.

## Attestation payload (`zcash-shielded-vote:round-auth:v3`)

```text
"zcash-shielded-vote:round-auth:v3"   33 bytes
|| round_id                           32 bytes
|| ea_pk                              32 bytes
|| u32(pir_depth) || u32(tier0_layers) || u32(tier1_layers) || u32(poly_len)
|| ballot_digest                      32 bytes
```

This is the v2 payload with a new domain tag and the ballot digest appended.
The PIR layout is the dynamic config's top-level `pir_layout`.

## Dynamic config entry

```json
"rounds": {
  "<round_id hex>": {
    "auth_version": 2,
    "ea_pk": "<base64>",
    "signatures": [{ "key_id": "...", "alg": "ed25519", "sig": "<base64 over v2 payload>" }],
    "ballot_attestation": {
      "ballot_digest": "<64 lowercase hex>",
      "signatures": [{ "key_id": "...", "alg": "ed25519", "sig": "<base64 over v3 payload>" }]
    }
  }
}
```

`ballot_attestation` is optional and additive. Wallets built before it ignore
the field and verify `signatures` as before, so signers can publish it without
a coordinated wallet release.

A wallet that understands the field:

1. verifies the v2 `signatures` as before;
2. if `ballot_attestation` is present, requires it to decode, its
   `ballot_digest` to be 64 lowercase hex characters, and at least one
   signature over the v3 payload to verify under a trusted key, and skips the
   round otherwise. A malformed attestation skips only its own round;
3. records the attested digest on the authenticated round
   (`AuthenticatedRound::ballot_digest`).

## Requiring attestations

The dynamic config is served by a host that can edit it, so it could remove an
entry's `ballot_attestation` while leaving the v2 signatures intact. Making
attestations mandatory therefore has to be anchored in the hash-pinned static
config:

```json
{
  "static_config_version": 2,
  "dynamic_config_urls": ["..."],
  "trusted_keys": [ ... ],
  "require_ballot_attestation": true
}
```

When it is `true`, a wallet skips every round without a valid ballot
attestation, so `BallotNotAttested` cannot occur. It defaults to `false`, and
wallets that predate the field ignore it. Set it once signers publish an
attestation for every round the wallet should show.

## Wallet check

Before displaying a round's content, deserialize the `round` object of the vote
server's round response into a `Ballot` (absent fields default to proto3 zero
values) and call `ResolvedVotingConfig::authenticate_ballot`. Display only
the returned `AuthenticatedBallot`. Its errors are distinct:

| Error | Meaning |
| --- | --- |
| `RoundNotAuthenticated` | No authenticated config entry for the round. |
| `BallotNotAttested` | The entry carries no ballot attestation. |
| `DigestMismatch` | The served content differs from what was attested. Discard this server's response and try another. |

How a wallet treats `BallotNotAttested` while signers roll out attestations is
a wallet policy decision. Once every published round carries an attestation,
set `require_ballot_attestation` in the static config, as described above.

## Signer procedure

1. Read the round from chain state, not from a vote server.
2. Compute `ballot_digest` with the encoding above and show the signer the
   ballot content being attested.
3. Sign the v3 payload and add the `ballot_attestation` alongside the existing
   v2 signatures.
