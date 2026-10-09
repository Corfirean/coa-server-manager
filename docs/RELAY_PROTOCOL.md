# Game Relay Protocol (Phase 13)

The Game Relay (`services/coa-relay`) provides TCP connectivity for WoW 3.3.5 clients when the realm's Host is behind NAT or CGNAT without port forwarding.

The Relay is logically and operationally separate from the Registry and Coordinator:
- **Registry**: Public directory of realms (HTTPS).
- **Coordinator**: Control plane signaling and routing for Noise channels (WSS).
- **Game Relay**: Multiplexed byte tunnel for WoW client game traffic (`authserver` on TCP 3724 and `worldserver` on TCP 8085).

---

## 1. Security Architecture and Invariants

```text
WoW Client (3.3.5)
   │
   ├── TCP :40000 (Auth)  ──► [coa-relay] ◄── Outbound WSS ── [Host Manager] ──► 127.0.0.1:3724 (local authserver)
   │                           (:8082 internal)
   └── TCP :40001 (World) ──► [coa-relay] ◄────────────────── [Host Manager] ──► 127.0.0.1:8085 (local worldserver)
```

1. **Host Outbound Only**: The Host initiates an outbound WebSocket connection to `wss://coa-manager.duckdns.org/relay/v1/host`. The Host requires zero inbound firewall openings or port forwarding.
2. **Strict Host Local Targets**: The Host's `RelayLink` worker strictly permits only two local targets:
   - `RelayTarget::Auth` -> `127.0.0.1:3724`
   - `RelayTarget::World` -> `127.0.0.1:8085`
   Any attempt to request another destination (e.g., MySQL 3306, Remote Access console, or arbitrary intranet hosts) is rejected by the Host.
3. **No General Bearer Tokens**: Relay allocations are requested by the Host on behalf of a specific authenticated PlayerIdentity/session via the control plane. Allocations are bound to `RealmId`, `PlayerId`, have short lifetimes (default 120s before connection), and cannot be used for administrative or control plane operations.
4. **Dumb Byte Tunnel**: The Relay never terminates WoW encryption or parses character data. The only packet inspection performed is rewriting the `address` field in the plaintext `CMD_REALM_LIST` response (opcode `0x10`) so the client seamlessly connects to the allocated WORLD port without patching AzerothCore or mutating MySQL `realmlist`.
5. **Zero Plaintext Logging**: Relay logs contain only connection identifiers, timestamps, stream IDs, and byte counts. Zero credentials, account names, session keys, or packet contents are logged.
6. **Resource Limits & Backpressure**:
   - Connection limits: max 16 concurrent hosts, max 256 allocations total, max 2 active allocations per player.
   - Timeouts: 10s challenge timeout, 60s idle timeout, 30s ping/pong keepalive.
   - Bounded streaming chunks: 16 KiB chunks with backpressure.
   - Dynamic port pool: 40000–40050/tcp. Unused or closed ports are immediately reclaimed.

---

## 2. Host Tunnel Handshake

When a Host Manager publishes a realm, it establishes an outbound connection to `/relay/v1/host`.

1. **Relay Challenge**: The Relay sends a challenge frame containing a random nonce:
   ```json
   { "nonce": "<base64url_32_bytes>" }
   ```
2. **Host Hello**: The Host signs the challenge with the realm's Ed25519 private key:
   ```
   message = "coa-relay-host-v1\n" || protocol_version || "\n" || realm_id || "\n" || nonce
   ```
   The Host replies:
   ```json
   { "realm_id": "<RealmId>", "signature": "<base64_signature>" }
   ```
3. **Relay Verification**: The Relay verifies the signature against the realm's registered public key in the database (`coa_registry.realms`).
4. **Relay Welcome**: On success:
   ```json
   { "ok": true, "message": "welcome" }
   ```

---

## 3. Tunnel Messaging (`TunnelMsg`)

Multiplexed JSON messages over the WebSocket tunnel:

| Message | Fields | Purpose |
|---|---|---|
| `ping` | — | Keepalive probe |
| `pong` | — | Keepalive response |
| `allocate` | `request_id`, `player_id` | Host requests game ports for a player |
| `allocate_ok` | `request_id`, `token`, `relay_host`, `auth_port`, `world_port`, `expires_at` | Relay provides allocated ports |
| `allocate_err` | `request_id`, `error` | Allocation failed (e.g. pool exhausted) |
| `connect` | `stream_id`, `token`, `target` (`auth` \| `world`) | Client connected to relay port; requests Host to connect local target |
| `connect_ok` | `stream_id` | Host connected local target successfully |
| `connect_err` | `stream_id`, `error` | Host failed to connect local target |
| `data` | `stream_id`, `chunk` (base64) | Bidirectional raw stream bytes |
| `close` | `stream_id` | Half-close / end of stream |
| `reset` | `stream_id` | Immediate abnormal stream termination |

---

## 4. Client Flow & REALM_LIST Address Rewriting

```text
1. Player clicks "Join Realm" in Manager
2. Player Manager requests AllocateRelay over Noise control channel
3. Host Manager allocates (auth_port, world_port) via Relay tunnel
4. Host returns RelayAllocated to Player Manager
5. Player Manager writes realmlist.wtf: "set realmlist relay_host:auth_port" and starts WoW client
6. WoW client connects to relay_host:auth_port (authenticates with SRP6 against local authserver)
7. Authserver sends CMD_REALM_LIST (0x10) with local address (e.g. "127.0.0.1:8085")
8. Relay rewrites address string to "relay_host:world_port" and updates packet body length
9. WoW client receives rewritten realmlist and connects directly to relay_host:world_port
10. World session begins; bytes tunneled end-to-end through Host tunnel
```

### Packet Layout (`0x10 CMD_REALM_LIST`)
- `[0]`: Opcode (`0x10`)
- `[1..3]`: Packet Body Length (`uint16_le`)
- `[3..7]`: Unused (`uint32`)
- `[7..9]`: Realm Count (`uint16_le`)
- For each realm:
  - `[0]`: Type (`uint8`)
  - `[1]`: Lock (`uint8`)
  - `[2]`: Flags (`uint8`)
  - `[...]`: Name (null-terminated string)
  - `[...]`: Address (null-terminated string) -> **Rewritten to `<relay_host>:<world_port>\0`**
  - `[...]`: Population (`float32`)
  - `[...]`: Characters (`uint8`)
  - `[...]`: Timezone (`uint8`)
  - `[...]`: Realm ID (`uint8`)
- Footer: `0x10 0x00`
