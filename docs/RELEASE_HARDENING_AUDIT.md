# Release & Security Hardening Audit (Phase 15)

**Version:** 0.7.0-rc1  
**Status:** Pre-Release Freeze & Hardening Audit  
**Target Branch:** `feat/portable-characters`  
**Date:** October 2026  

---

## 1. Executive Summary & Architecture Freeze

Phase 15 represents the final engineering stabilization and security hardening phase prior to the public release of **CoA Server Manager**.

The functional architecture is strictly **frozen**:
- Realm Registry v2 (discovery, signed adverts, capabilities)
- Coordinator & Noise XX authenticated control channel
- Automatic account provisioning & credential linking
- Portable Characters & Remote Portable Transfer
- Direct Ingress (Host proxy, UPnP IGD, NAT-PMP, external reachability verification)
- Game Relay fallback (`coa-relay` multiplexed TCP tunnel)
- Real Ascension client launch & realmlist redirection
- Integrated Server Browser & "Play with Friends" tabbed UX

The objective is to verify and prove that the system is:
$$\text{Correct} + \text{Secure} + \text{Recoverable} + \text{Non-Destructive} + \text{Clean-Installable} + \text{Diagnosable} = \mathbf{Release\ Candidate}$$

---

## 2. Comprehensive Subsystem Security & State Map

| Subsystem | Trust Boundary | Secrets Handled | Persistent State | Network Input | Size / Rate Limits | Failure Mode | Recovery Behavior | Risk Classification |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **1. Manager UI & Tauri Commands** | Local user desktop to Tauri backend IPC | None exposed to frontend; passwords handled only as ephemeral strings | Frontend settings, local UI preferences | IPC payloads via Tauri invoke | Strict input schemas; validated paths & credentials | Command returns structured UiError | Frontend displays actionable error modal/banner | **P1**: Ensure no secret leakage in IPC serialization or error logs |
| **2. Portable Owner Store** | Local player process to SQLite DB | None (game character snapshots only) | `portable.sqlite` (canonical character snapshots, revisions, history) | None (local SQLite) | SQLite transaction sizes bounded to character JSON (< 2 MiB) | Transaction rollback | WAL journal recovery on restart; corrupt DB fails closed with diagnostic error | **P1**: Prevent silent deletion of unmigrated/corrupt DB; atomic migrations |
| **3. Portable Host Store** | Host Manager process to SQLite DB | Host session state, player session tokens (ephemeral) | `portable_host.sqlite` (active player sessions, baseline snapshots) | Controlled by Noise session handler | Bounded by max concurrent sessions (default 64) | Session abort | Server restart cleans stale sessions; canonical state remains safe on Owner | **P1**: WorkerThreads dependency must fail closed with explicit preflight check |
| **4. SecretStore (Player & Credentials)** | Local OS user to persistent storage | Player Ed25519 private key, per-realm game account passwords | `.dpapi` files (Windows) or `0600` files (Unix) | None (local storage) | Key names <= 96 bytes, alphanumeric + `._-` | DPAPI decryption failure (e.g. machine change) | Actionable error; fails closed; never generates silent replacement identity | **P1**: Must be strictly verified and audited for zero memory/log exposure |
| **5. Realm KeyStore (Host Realm Identity)** | Local Host Manager to persistent storage | Realm Ed25519 signing private key | Formerly `{realm}.key` file; migrating to DPAPI | None (local storage) | 32-byte Ed25519 seed | Decryption/load failure | Fails closed; NEVER silently regenerates RealmId behind user's back | **P0/P1**: Move from plaintext ACL file to DPAPI SecretStore with migration |
| **6. Registry Client** | Host/Player Manager to Public Registry HTTP API | None (signs with Realm Ed25519; reads public listings) | Cached listings, publication registration record | HTTP responses from `https://coa-manager.duckdns.org` | Max listing response 4 MiB; 100 entries per page | Network timeout / Registry HTTP error | Exponential backoff retry; Host marks registration unconfirmed | **P1**: Strict response parser hardening against hostile JSON |
| **7. Coordinator Client & Host Hub** | Public Internet WebSocket to Coordinator service | Noise XX handshake (ephemeral Curve25519, static Ed25519) | Active control connections | WebSocket text / binary frames | Rate-limited hello (60/min); max frame 64 KiB | WebSocket disconnect | Automatic reconnect with exponential backoff; active sessions abort cleanly | **P1**: Hostile frame reassembly and parser hardening |
| **8. Noise XX Control Protocol** | Player Manager to Host Manager peer channel | Ephemeral keys, symmetric ChaCha20-Poly1305 keys | None (in-memory cipher state) | Encrypted transport frames | Strict 4 KiB limit on standard control messages | Decryption error / MAC failure | Immediate connection drop; zero state mutation | **P1**: Adversarial fuzz testing for truncated/corrupt/tampered frames |
| **9. Remote Transfer Flow** | Player Manager to Host Manager authenticated channel | Transfer tokens (ephemeral UUID), character payload | Transfer staging directory | `TransferOffer`, `TransferChunk`, `TransferStatus` | 4 KiB chunks, 4 MiB total payload limit; 30s timeout | Chunk checksum failure / timeout | Offer rejected or transaction aborted; rollback staging files | **P1**: Strict bounded chunk allocations; verify zero character corruption |
| **10. Game Relay Client & Ingress Tunnel** | Host Manager / Player Manager to Public Relay VPS | Allocation token, Host auth signature | Active TCP tunnels | Multiplexed `TunnelMsg` (Auth & World) | Bounded buffer queues (256 KiB); backpressure drop | Relay disconnect | Player client disconnects; new JOIN requests fall back or retry | **P1**: Prevent arbitrary localhost/LAN port forwarding; strictly 3724 & 8085 |
| **11. DirectIngress (Host Proxy)** | Internet player TCP to Host local Auth/World | None (game packet proxy) | Listening TCP ports | Raw TCP stream (CMD_REALM_LIST) | 8 KiB buffer bounds; 500ms read timeout loops | Ingress worker socket error | Client disconnects cleanly; next JOIN automatically uses Relay | **P1**: Ensure CMD_REALM_LIST rewrite strictly uses verified external port |
| **12. UPnP IGD Discovery & Mapping** | Host Manager to LAN Gateway (SSDP/SOAP) | None | Active port mapping leases | SSDP UDP responses, SOAP HTTP XML | Local LAN IPs only; strict XML tag extraction | Device timeout / mapping refused | Fall back to NAT-PMP or Relay; transactional cleanup on partial failure | **P1**: Transactional mapping (auth+world); lease renewal; clean removal |
| **13. NAT-PMP Discovery & Mapping** | Host Manager to Default IPv4 Gateway | None | Active port mapping leases | NAT-PMP UDP binary packets | Default OS gateway only; bounded port response | Gateway timeout / unsupported | Fall back to UPnP or Relay; periodic lease renewal (3600s) | **P1**: OS default gateway routing check; transactional cleanup |
| **14. Client realmlist.wtf Handling** | Player Manager to local game client filesystem | None | `realmlist.wtf` file state | Local disk file | Bounded file size (< 64 KiB) | Process crash during active game | Transactional backup file; automatic restoration on next Manager startup | **P1**: Prevent permanently clobbered realmlist.wtf after crash |
| **15. Server Installation & Update** | Host Manager to server directory & packages | None | Manifest files, backups, installation journals | Signed update packages | SHA-256 verification of manifests and packages | Update failure / verification error | Atomic rollback from journal and snapshot; original files preserved | **P1**: Absolute non-destructive guarantee over existing user servers |
| **16. MySQL Database Access** | Host Manager to local/container MySQL server | MySQL root/acore password | `acore_auth`, `acore_characters`, `acore_world` | TCP 3306/3307 | Parameterized queries only; bounded statement execution | DB unreachable / query error | Error logged; no unconfirmed state recorded | **P1**: Credentials never logged or dumped in diagnostics |
| **17. RA / World Console Client** | Host Manager to worldserver Remote Admin | RA username & password | None (console connection) | TCP 3443 text stream | Line-based ASCII protocol; bounded timeouts (5s) | Worldserver console down | Returns structured Error::Invalid; graceful retry | **P1**: Never send unvalidated user input to RA console; zero password log |
| **18. VPS PostgreSQL Database** | Registry & Relay to VPS PostgreSQL | Dedicated DB user passwords (`coa_registry`, `coa_relay`) | `realms`, `adverts`, `allocations` tables | Internal Docker network SQL | Parameterized queries; connection pool limits (20) | Database failover / error | 500 Internal Error to client; automatic reconnection | **P1**: Logical rotating backups; dedicated unprivileged roles |
| **19. VPS Caddy Reverse Proxy** | Public Internet (80/443) to VPS internal network | Let's Encrypt TLS private key | ACME certificate storage in persistent volume | Public HTTPS & WSS requests | Admin API disabled (`admin off`); strict route reverse proxy | Upstream unreachable | 502 Bad Gateway | **P1**: Ensure only 8080/8081/8082 routed; internal ports unpublished |
| **20. Registry Service** | Public Internet to VPS Registry API | None (verifies Ed25519 signatures of Host adverts) | PostgreSQL `realms`, `adverts` | HTTP JSON POST/GET requests | Rate limiter per IP (120/min); max advert size 64 KiB | DB error / invalid signature | HTTP 400/403/429/500 | **P1**: Parameterized queries; strict schema parsing; bounded allocations |
| **21. Coordinator Service & Probe** | Public Internet to VPS Coordinator WSS & HTTP probe | Ephemeral Curve25519 keys; verifies Ed25519 | Active peer connections in-memory | WebSocket frames; HTTP probe query/JSON | Max 4 ports; rate-limited; forbidden ports blocked | Connection drop / probe timeout | Clean session teardown; error JSON | **P0/P1**: Probe must only probe requester's source IP; rate limit; block LAN/internal IPs |
| **22. Relay Service** | Public Internet to VPS Relay TCP ports (40000-43999) | Host signature over allocation token | In-memory active session multiplexer | TCP streams from Host & Players | Max 2,000 allocations; 256 KiB backpressure buffer | Stream error / slow reader | Session closed; port returned to pool; no memory leak | **P1**: Non-root UID; cap_drop ALL; read-only rootfs; log rotation |

---

## 3. Finding Classification & Action Plan

### P0 / P1 Findings (Must Fix in Phase 15)

1. **[P0/P1-01] Windows Realm Signing Key Storage**:
   - *Problem*: Host realm Ed25519 signing key stored as plaintext base64 file with ACL (`icacls`).
   - *Requirement*: Move to protected Windows DPAPI via `SecretStore` with domain separation entropy, transactional migration, validation of public key / RealmId, atomic replacement, and fail-closed if corrupt.
   - *Status*: Implemented in Task 2.

2. **[P1-02] Coordinator Probe Abuse Hardening**:
   - *Problem*: `/coord/v1/probe` must be verified against generic port scanning: only caller's verified source IP tested, X-Forwarded-For spoofing prevented, probe requests rate-limited, private/loopback/multicast IPs rejected, concurrent probe limits enforced.
   - *Status*: Implemented in Task 5.

3. **[P1-03] Client `realmlist.wtf` Crash Recovery**:
   - *Problem*: If Manager crashes or power is lost while playing, `realmlist.wtf` might retain the temporary override.
   - *Requirement*: Create a crash-safe transaction marker file before modification; on Manager startup, check for uncommitted marker and restore original bytes immediately.
   - *Status*: Implemented in Task 14.

4. **[P1-04] CharacterDatabase WorkerThreads Safety Refusal**:
   - *Problem*: Runtime portable character session checkpoints require `CharacterDatabase.WorkerThreads = 1`.
   - *Requirement*: Preflight check must explicitly inspect `worldserver.conf` and refuse portable hosting with clear instructions if `WorkerThreads != 1`.
   - *Status*: Implemented in Task 10.

5. **[P1-05] Secret & Log Redaction Verification**:
   - *Problem*: Passwords, tokens, and private keys could accidentally appear in logs, panics, or diagnostics bundles.
   - *Requirement*: Structured redaction helpers, diagnostic bundle sanitizer, and automated integration test asserting zero leak of sentinel secrets.
   - *Status*: Implemented in Tasks 3 & 27.

6. **[P1-06] Adversarial Protocol Fuzz / Decoder Hardening**:
   - *Problem*: Malformed, oversized, or truncated network inputs must never panic or allocate unbounded memory.
   - *Requirement*: Comprehensive property and fuzz test suite covering Registry v2 JSON, Noise frames, TransferOffer/Chunk, TunnelMsg, REALM_LIST rewriter, and Coordinator frames.
   - *Status*: Implemented in Task 4.

7. **[P1-07] VPS Infrastructure Backup & Restore Automation**:
   - *Problem*: VPS PostgreSQL metadata needs automated rotating logical dumps (`pg_dump`) with retention and atomic completion markers, plus verified test restore.
   - *Status*: Implemented in Task 17.

### P2 Findings (Acceptable Follow-Up / Out of Scope for Phase 15)

- **[P2-01] Desktop Keyring on Linux/macOS**: Documented weaker `0600` file store behavior on non-Windows platforms. (Windows is the primary release target).
- **[P2-02] Live Mid-Session TCP Migration Direct $\to$ Relay**: When a direct connection dies mid-session, the game TCP stream disconnects normally; the user re-joins and the next session automatically falls back to Relay. Seamless live session migration is architecturally impossible over raw WoW TCP and is not promised.
- **[P2-03] Native Ascension Login Screen Prefill**: Left out of scope to avoid reverse-engineering `GlueConfig.json` or client memory.
