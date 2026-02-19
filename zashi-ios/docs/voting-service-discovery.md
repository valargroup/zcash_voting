# Voting Service Discovery

How Zashi discovers vote servers and nullifier providers at runtime.

## Resolution order

1. **Local override** — `voting-config-local.json` bundled in the app
2. **CDN** — `https://zally-phi.vercel.app/voting-config.json`
3. **Hardcoded fallback** — deployed dev server (`46.101.255.48`)

The first source that succeeds wins. This means a TestFlight build works out of the box (CDN or fallback), while a developer can drop a local file into the bundle to point at localhost.

## Config format

```json
{
  "version": 1,
  "vote_servers": [
    { "url": "https://46-101-255-48.sslip.io", "label": "Primary" }
  ],
  "nullifier_providers": [
    { "url": "https://46-101-255-48.sslip.io/nullifier", "label": "Primary" }
  ]
}
```

`vote_servers` entries each serve the full set of endpoints — both chain API (`/zally/v1/*`) and helper API (`/api/v1/shares`). This is because the SDK and helper server are a single merged binary.

`nullifier_providers` serve `/nullifier/proof/{nullifier_hex}` for IMT non-membership proofs.

## Local testing

Create `voting-config-local.json` in the Xcode project (add to the app target so it's copied into the bundle):

```json
{
  "version": 1,
  "vote_servers": [
    { "url": "http://localhost:1318", "label": "Localhost" }
  ],
  "nullifier_providers": [
    { "url": "http://localhost:3000", "label": "Localhost" }
  ]
}
```

This file is checked before the CDN fetch, so it always takes priority. Don't commit it — it's for local dev only.

## Where the URLs flow

```
VotingStore.initialize
  → votingAPI.fetchServiceConfig()        // resolves config per order above
  → votingAPI.configureURLs(config)       // sets ZallyAPIConfigStore actor
  → all subsequent API calls use resolved URLs
```

The resolved config is also stored in `VotingStore.State.serviceConfig` so the store can read URLs for the IMT server and chain node directly (used by `votingCrypto.syncVoteTree` and delegation proof).

## Share distribution

When multiple vote servers are configured, encrypted shares are distributed across them instead of all going to one server. With N servers and 4 shares:

- N >= 4: one share per server (shuffled)
- 1 < N < 4: round-robin
- N == 1: all shares to that server

## CDN config deployment

The production config lives at `shielded_vote_generator_ui/public/voting-config.json`. Merging to main auto-deploys it to Vercel at `https://zally-phi.vercel.app/voting-config.json`.
