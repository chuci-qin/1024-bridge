# Bridge1024 API Reference

## EVM Contract API

### Write Functions

#### `stake(uint256 amount, string receiverAddress) → uint64`
Deposit USDC to bridge. Returns nonce.
- `amount`: USDC amount in smallest unit (6 decimals)
- `receiverAddress`: Target chain receiver address

#### `submitSignature(StakeEventData eventData, bytes signature)`
Submit relayer signature for cross-chain unlock. Relayer-only.

#### `addRelayer(address relayerAddress)` / `removeRelayer(address)` / `rotateRelayer(address old, address new)`
Manage relayer whitelist. Admin-only.

#### `configureUsdc(address)` / `configurePeer(bytes32, uint64, uint64)` / `configureDecimalRatio(uint64)`
Configuration functions. Admin-only.

#### `configureRateLimits(uint256 maxPerWindow, uint256 windowDuration, uint256 maxSingle, uint256 minReserve)`
Set rate limiting parameters. Admin-only.

#### `proposeAdmin(address)` / `acceptAdmin()`
Two-step admin transfer.

#### `pause()` / `unpause()`
Emergency circuit breaker. Admin-only.

#### `emergencyWithdraw(address token, uint256 amount, address to)`
Recover stuck funds. Admin-only.

### Read Functions

- `senderState()` — returns sender configuration
- `receiverState()` — returns receiver configuration  
- `getRelayers()` — returns relayer list
- `isRelayer(address)` — check if address is relayer
- `processedNonces(uint64)` — check if nonce was processed

## SVM Program API

### Instructions

- `initialize` — Deploy bridge, set admin
- `configure_usdc(usdc_mint)` — Set USDC mint
- `configure_peer(peer_contract, source_chain_id, target_chain_id)` — Set peer
- `configure_fee(fee)` — Set bridge fee (1024chain only)
- `configure_rate_limits(max_unlock_per_window, window_duration, max_single_unlock, min_reserve)`
- `stake(amount, receiver_address)` — Deposit USDC
- `submit_signature(nonce, event_data, signature)` — Submit Ed25519 signature
- `add_relayer(relayer)` / `remove_relayer(relayer)` / `rotate_relayer(old, new)`
- `propose_admin(new_admin)` / `accept_admin()`
- `pause()` / `unpause()`
- `close_request(nonce)` — Reclaim rent after unlock
- `add_liquidity(amount)` / `withdraw_liquidity(amount)`

## Relayer API

### Health Endpoint
`GET /health` — Returns 200 if healthy

### Environment Variables
- `BRIDGE_ID` — Bridge pair identifier (e.g., "arbsep-1024test-usdc")
- `CONFIG_PATH` — Path to bridges.json
- `QUEUE_DIR` — Event queue directory
- `RELAYER_PRIVATE_KEY` — Relayer signing key
- `ROLE` — "listener", "submitter", or "both"
- `HEALTH_PORT` — Health check port (default: 8080/8081)
