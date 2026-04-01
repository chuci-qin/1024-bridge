# Bridge1024 Test Plan

## Test Levels

### Unit Tests
- **EVM**: Foundry (`forge test`) — 37+ test cases covering all audit findings
- **SVM**: Anchor TS (`anchor test`) — 20+ test cases
- **Relayer**: Rust `cargo test` — 40+ unit tests across core, listener, submitter

### Integration Tests
- Relayer listener with mock RPC providers
- Relayer submitter with mock chain endpoints

### E2E Tests
- `evm-to-svm.ts` — Full EVM→SVM flow on testnet
- `svm-to-evm.ts` — Full SVM→EVM flow on testnet
- `sol-to-svm.ts` — Full Solana→1024chain flow
- `svm-to-sol.ts` — Full 1024chain→Solana flow

### Acceptance Tests (ATDD)
Gherkin feature files covering:
- Happy path scenarios for all 4 bridge directions
- Security scenarios from real-world exploit case studies
- Rate limiting, nonce bitmap, circuit breaker behavior

## Security Test Categories

| Category | What to test | Inspired by |
|----------|-------------|-------------|
| Key compromise | Threshold prevents unlock with < 2/3 keys | Ronin ($624M) |
| Sig bypass | Ed25519/ECDSA checks cannot be circumvented | Wormhole ($326M) |
| Zero values | Uninitialized state doesn't create attack surface | Nomad ($190M) |
| Origin validation | Events from wrong chain/contract rejected | CrossCurve ($3M) |
| Rate limits | Limits can't be gamed at window boundaries | Industry |
| Vault drain | Min reserve + rate limit prevent full drain | Industry |
| Admin recovery | Bridge recovers from compromised admin key | IoTeX ($4.4M) |

## Coverage Requirements
- EVM contract: > 90% line coverage
- SVM program: All instructions covered
- Relayer core: > 80% line coverage
- All CRITICAL and HIGH audit findings have dedicated test cases
