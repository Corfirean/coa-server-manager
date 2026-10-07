# Control plane (Phase 12)

How a player's Manager gets a game account on a realm, links one it already has, and turns a character that exists on that realm into a portable one, **without the central node ever
seeing a password, a character or a database credential**. This is the *control* plane: there is no game traffic here (that is the Relay, which does not exist yet), and the Manager does not
type into the game.

Code: `crates/coa-control-proto` (the protocol: pure, no I/O), `services/coa-coordinator` (the Coordinator), `crates/coa-core/src/control` (Host and Player sides),
`src-tauri/src/control_cmds.rs` and `src/screens/JoinPanel.tsx` (the interface). End-to-end tests: `services/coa-control-e2e`. Deployment: [REGISTRY_DEPLOYMENT.md](REGISTRY_DEPLOYMENT.md).
`CONTROL_PROTOCOL_VERSION = 1`; it is independent of the Registry protocol (2) and of the portable formats.

## 1. Shape

```text
Player Manager --- WebSocket --->  Coordinator  <--- WebSocket (opened by the Host) ---  Host Manager
      \_____________ Noise_XX end-to-end channel, authenticated by Ed25519 signatures over its handshake hash ____________/
```

* The **Host Manager** (the one that runs the realm) keeps one *outbound* connection per published realm to `/coord/v1/host`. It needs no open port.
* A **Player Manager** connects to `/coord/v1/player?realm=<RealmId>`; the Coordinator routes it to that realm's Host by a small connection number.
* Both sides then run a **Noise** channel through the Coordinator. The Coordinator forwards opaque frames; it can neither read nor change them, nor pretend to be either side.
* The realm key (Ed25519, registered with the Registry in Phase 10) is what proves "this Host is the realm in the list". The player key (Ed25519, created on the player's machine) proves "this is that player".

What each party learns:

| | learns | does **not** learn |
|---|---|---|
| **Coordinator** | which realm id, which player id and public key connected, source address, when, for how long, how many bytes | any account name, password, character, database or console credential, any request or answer (all inside the channel) |
| **Registry** | unchanged by Phase 12 (the Coordinator reads one realm's public key from its database through a four-column, read-only role) | anything about players |
| **Host Manager** | the player's id and public key, the account it created or linked for that player on its own realm, the password *for that realm* (it sets it: unavoidable, and unique per realm), the characters the player claims there | the player's other realms' credentials, the player's private key |
| **Player Manager** | its own credentials per realm, characters it claims | the Host's database or console credentials, other players' data |

## 2. Identities

* **RealmId + realm key**: Phase 10 (`docs/REGISTRY_PROTOCOL.md` section 2). The Registry publishes the key with the realm (`GET /registry/v2/realms/{id}` → `public_key`). The Coordinator checks a Host's hello against the
  key it reads from the Registry's database; a player checks the Host's proof against the key in the record **and pins it at the first visit**: a different key for the same realm id later is refused before anything
  is sent (a realm never changes its key; a new key is a new realm id). So a Registry that is compromised *after* a player first joined a realm cannot redirect that player.
* **PlayerIdentity**: a random `PlayerId` (UUIDv7) and an Ed25519 key, created on first use and kept in the `SecretStore` (section 8). The id is public (a Host records it); the key never leaves the machine.
  A key that cannot be read is an error; it is never silently replaced by a new identity (a new identity would own none of the player's accounts and characters).
* **Host-side binding**: the first time a PlayerId gets an account on a realm, the Host records the public key the player presented. From then on that PlayerId is only accepted with that key
  (`Invalid`: "This player id belongs to another key on this realm", and the channel is closed before any request). A PlayerId is a pseudonym, not a secret; a stolen id is useless without the key.

## 3. The Coordinator protocol

`GET /coord/v1/health` → `{"status":"ok","protocol_version":1,"hosts":N}`. `GET /coord/v1/host` and `GET /coord/v1/player?realm=<RealmId>` upgrade to WebSocket. Text frames are JSON control frames (strict:
unknown fields are refused, ≤ 2048 bytes); binary frames are `[connection number: u32 big-endian][one end-to-end frame, ≤ 20 KiB]`.

Both endpoints start the same way: the Coordinator sends `{"t":"challenge","protocol":1,"nonce":"<base64url of 32 random bytes>"}` and the peer must answer within 10 s.

Host hello: `{"t":"host_hello","realm_id":…,"ts":<unix s>,"sig":…}`, the signature (Ed25519 of the realm key, strict) over the UTF-8 bytes

```
coa-coord-host-v1\n<protocol>\n<realm id>\n<nonce>\n<ts>
```

Player hello: `{"t":"player_hello","player_id":…,"public_key":…,"realm_id":…,"ts":…,"sig":…}`, signature (proof of possession of the key in the frame) over

```
coa-coord-player-v1\n<protocol>\n<player id>\n<public key base64url>\n<realm id>\n<nonce>\n<ts>
```

The nonce is the Coordinator's own, fresh per connection, so a hello cannot be replayed; the timestamp must be within 120 s of the Coordinator's clock. The Coordinator accepts a Host only for a realm that is
published (protocol 2) and whose Registry key verifies the hello; a Player's hello only proves the player holds *a* key: whether it is the key of that PlayerId is the Host's decision (section 2).

After a good hello: the Host gets `host_ready`; a Player gets `player_ready{conn}` (the Host got `open{conn}` first). Either side ends a channel with `close{conn,reason}`; a Host that reconnects replaces its old
connection (its channels are closed with `host_offline`). Errors: `bad_hello`, `unknown_realm`, `host_offline`, `host_busy`, `rate_limited`, `too_large`, `timeout`, `internal`.

Limits (enforced by the service, unit- and integration-tested): 16 simultaneous channels per Host, 8 connections per address, 60 new channels per minute per realm and 30 per address, 16 MiB through one channel, a channel
lives at most 600 s and at most 60 s silent (the Host has 10 s to answer a new one), frames ≤ 20 KiB (a larger one cuts the channel), an unexpected text frame on an established channel ends it. The Coordinator keeps no state on disk.

## 4. The end-to-end channel (the exact handshake)

`Noise_XX_25519_ChaChaPoly_BLAKE2s` (the Noise Protocol Framework, implemented by the `snow` crate; nothing is invented). Both sides use **fresh random X25519 static keys for every channel**, so the Noise layer gives
confidentiality, integrity and forward secrecy but no identity. Identity is added by **channel binding**: after the handshake each side signs the handshake hash `h` (unique to this channel and equal on both ends) with its long-lived
Ed25519 key.

```text
Player -> Host    msg1   (e)                           (binary frame, ≤ 1 KiB)
Host   -> Player  msg2   (e, ee, s, es)
Player -> Host    msg3   (s, se)                       the channel is open; h = the handshake hash
Host   -> Player  HostProof   = {realm_id, sig = Ed25519_realm_key("coa-ctl-host-v1\0" ‖ h ‖ realm_id)}        sent encrypted
                  the Player verifies it against the realm key from the Registry (pinned) -- if it fails nothing is sent
Player -> Host    PlayerProof = {player_id, public_key, sig = Ed25519_player_key("coa-ctl-player-v1\0" ‖ h ‖ realm_id ‖ player_id)}
                  the Host verifies it, and that the key is the one it knows for this player id
Player <-> Host   requests and responses (JSON), one at a time, in order
```

A Coordinator that tries to sit in the middle would have to run two handshakes, each with its own `h`; the Host's signature covers only the Host's `h`, so the Player rejects it. The proofs of one channel are useless on another.
Messages are split into frames of ≤ 16 KiB of plaintext (+ a flag byte + the 16-byte tag, so ≤ 20 KiB with the connection number), reassembled up to 8 MiB; a frame that does not authenticate ends the channel.
Application messages: requests ≤ 4096 bytes, strict JSON with unknown fields refused, at most 60 requests per channel, 300 s per channel at the Host (idle 45 s).

## 5. Application messages

| Request | Answer | |
|---|---|---|
| `hello{protocol, client}` | `welcome{realm_id, automatic, existing_only, route}` | version check and what the realm offers |
| `provision{desired, password, have_credentials}` | `provisioned{username, created, reset}` | get or reuse the player's account |
| `link{login, password}` | `linked{username}` | one-time link of an account that already exists |
| `list_characters` | `characters{characters}` | the characters of the player's account (opaque per-connection handles; the realm's own numbers never leave the Host) |
| `claim{token}` | `claimed{character_id, sha256, payload, collections}` | export one character (section 7) |
| `claim_ack{character_id}` | `done` | the player stored it; the claim is final |
| `route` | `route_info{address}` | the address the game client can use right now, if the owner stated one |

Errors (`error{code,message}`): `unsupported_version`, `invalid`, `provisioning_off`, `no_account`, `not_at_character_select`, `not_yours`, `already_claimed`, `not_eligible`, `wrong_credentials`, `account_taken`, `rate_limited`, `unavailable`.
Backend failures are logged on the Host and answered as `unavailable` without internals.

## 6. Accounts

**Automatic provisioning is the default** (a realm's owner can switch it off: *Settings → Public listing → Do not create accounts for new players*, advertised as `existing_only`).

* The player's Manager makes a **unique random password per realm**: 16 characters of `A–Z0–9` from the operating system's randomness with rejection sampling (about 82 bits). The game compares passwords case-insensitively (SRP6 uppercases them),
  so letters carry no extra strength. It is saved in the `SecretStore` *before* it is sent, so a crash between the Host creating the account and the player learning of it leaves a password the next attempt sets again.
* The player's **preferred username** (a setting; default `PLAYER`) is made fit for the game: letters and digits, upper case, ≤ 16 characters. If the name is taken, the Host adds a stable suffix derived from the PlayerId:
  `DMITRY` → `DMITRY_7K4M` (four characters from a 32-letter alphabet without look-alikes; the same player gets the same alternative; attempt n gives another). Up to 10 alternatives are tried.
* The account is created through the realm's **supported path**: the world console's `account create <name> <password>` over RA (and `account set password` for a reset). **No verifier, salt or other field of the login
  database is written by the Manager.** Account names may contain `_` inside (`DMITRY_7K4M`); the Manager's own validation accepts it.
* The Host records `(realm, PlayerId) → (account id, name, kind, public key)` in `control.sqlite`. It never stores the password. The Player records the name in its control database and the
  password in the `SecretStore` (`realm-<id>`).
* Returning: the Host finds the mapping and answers `created:false`. A player who lost the saved password (reinstalled Manager) gets `reset:true` and a new password for the *same* account. An account an administrator deleted is created again.
* **Existing accounts** (`existing_only`, or an account from before Phase 12): the player enters name and password once; they go to the Host inside the channel and are checked against the realm's own stored SRP6 verifier
  (the Host computes the verifier from the name, the password and the stored salt and compares in constant time; internal and companion accounts are refused). On success the account is bound to that PlayerId; the plaintext is not stored on the
  Host, and failed attempts are rate-limited (5 per 10 minutes per player, 30 per realm). An account belongs to one PlayerId; a PlayerId has one account per realm.

## 7. Claiming a character that already exists on a realm

The player logs in to the realm with the game, stays at the **character selection screen**, switches to the Manager (Alt+Tab), opens the server in *Servers*, *Characters on this server*, and presses **Make portable** next to the character.
There is no GUID, UUID, claim code, RA or database credential anywhere in this flow.

The Host (`HostService`) checks, in this order, before it reads anything:

1. the PlayerId has an account on this realm (else `no_account`) and the character handle came from this channel's own listing;
2. **the account is online in the world session and has no character in the world**: `account.online` is set by the worldserver when a session authenticates, and no `characters.online` row exists for the account → "at the character-select screen";
   anything else is `not_at_character_select`;
3. **the character belongs to that account** (`not_yours`) and is **offline and eligible** (`not_eligible`: online, deleted, bot account, challenge or game-mode states, unknown tables, …);
4. **no other PlayerIdentity has claimed it** (`already_claimed`), and it is not already portable on this Host by other means.

Then the Host exports the character with the same code as *Make portable* (one consistent snapshot), registers it with its **host** store, and sends `claimed`: the encoded `PortableCharacter` (zstd JSON, base64), its content hash, and the account's
appearance and vanity collections, all inside the channel. The Player's Manager decodes it **with verification** (hash, format, that the character id is the one claimed), stores it as **canonical revision 1** with the collections merged into
its profile, and only then sends `claim_ack`; the Host marks the binding `PortableCharacterId ↔ PlayerIdentity` as acknowledged. An unacknowledged claim can be repeated by the same player and returns the same character; nobody else can take it.

## 8. Secrets on the player's machine

`SecretStore` (`control/secrets.rs`) keeps the identity key and the realm passwords; nothing secret is in JSON, SQLite, a log or a diagnostic (`control.sqlite` holds names only, checked by the gate).

* **Windows**: one file per secret holding a **DPAPI** blob (`CryptProtectData`, current-user scope, application entropy `coa-server-manager/secret-store/v1`, written with the file's permissions restricted first). Only the same
  Windows user on the same machine can open it; a copy elsewhere is useless; a tampered blob is an error.
* **Other systems**: a file with mode 0600. **This is not encryption** (anybody who can read the user's files can read it); the interface says so (*Secrets are kept in a protected file, not encrypted by the system*). A desktop
  keyring (Secret Service / Keychain) is the right store there and is not implemented.
* **Limitation that remains**: the realm's own signing key (`registry/keys`, Phase 10) is still a restricted file, not DPAPI. It belongs to the Host role and moving it into the `SecretStore` is a small follow-up (it is behind its own `KeyStore` trait).

## 9. JOIN

*Join this server* (`join_realm`): connect and verify the Host → `welcome` → (existing-only realm and no credentials: ask for the one-time link) → get or reuse the account → if a character was chosen: the existing Phase 7/8
compatibility check against the capabilities in the Registry record (nothing is written when it is incompatible) → the realm's route → client check → write the realmlist (the existing Phase 9 machinery with backups) and start the game.
Outcomes that are **not failures but honest states**: `needs_link`, `needs_client` (no game client set up), `incompatible` (the verdict and its notes), `needs_transfer` (a character that did not come from this realm cannot be put on it
yet), and **`needs_relay`: "This server requires Relay support, which is not available yet."** when the owner has not stated an address the game client can reach (*Settings → Public listing → Address players connect to*, a host and optional port; it is data the
owner supplies, not discovery, NAT traversal or a relay). Connectivity is never faked.

The player is shown the account name and can **copy the name and the password** (the password is read from the protected store only when asked, shown only on request, never logged).

## 10. Native login prefill: audit and blocker

The goal was to start the Ascension client with the name and password already in its login window, *if* the client's "Remember" persistence can be reproduced safely (no process injection, no `WriteProcessMemory`, no input injection,
backup and recovery of every client file touched, passwords never logged). **It cannot, with what is known, and the Manager does not attempt it.**

What was found: the client has native login-window storage (Lua `GetLastAccount`/`SaveAccount`-style calls in its glue code); the remembered login is not in `WTF/Config.wtf` or `realmlist.wtf` but, as far as can be told,
in `WTF/Custom/GlueConfig.json`, a 180-byte file of printable characters with 5.75 bits/byte of entropy that is not base64, hex or any text scheme tried, contains none of the known plaintext of a login (account name,
password, realmlist address), and is **rewritten by the client every time it starts, before any login** (observed during the gate: starting the game from the Manager changed it without changing its size). So the file holds more than the login, in an unknown
obfuscated format. Writing it blindly could corrupt the client's glue configuration or lock the account out of "Remember"; guessing the scheme from outside the client is reverse engineering that the control plane does not need.
The controlled before/after diff that would settle it needs a human to log in with "Remember" ticked (driving the game's window from outside is exactly the input injection that is out of scope); `tools/audit-client-credentials.ps1` is the procedure and
tool for that: it snapshots the client's `WTF`/`Data` files, and after a disposable login reports which files changed and, as booleans only, whether the test name or password appears in them as text, UTF-16, hex or base64. **Never print the contents.**

What is delivered instead: the account and its password in the protected store, *Copy name* / *Copy password* / *Show password* in the join panel (the game's own login window is pasted into), and the realmlist written and the game started.
**Blocker before this can change:** the owner's run of the audit with a disposable account, and then a decision whether the format is understood well enough to write safely.

## 11. The Phase-12 gate

Run on 2026-10-07 with the final code: a **Host Manager** on the real smoke realm *PT Guest* (a real worldserver with its own MySQL and RA, realm id `01a116d9…`, published to the Registry), **three Player Managers**
(separate processes, separate data folders, their own DPAPI stores, fresh identities) and the **Coordinator on the VPS**; every request went Manager → Internet → Caddy → Coordinator → Manager. The scripts are driven through the real Managers'
own commands (`gate12.mjs`, `gate12b.mjs`: the Tauri commands behind the Join panel). Passwords were never printed: they were checked against the realm's stored SRP6 verifier or compared by hash.

| | Scenario | Result |
|---|---|---|
| **A** | New player: automatic account, credentials saved, game started | **pass, except native prefill** (section 10). `join` with the preferred name `Dmitry` created `DMITRY` on the real realm (the row is in `acore_auth.account`), the 16-character password is saved in DPAPI and its SRP6 verifier equals the realm's, `launch` wrote the realmlist `127.0.0.1:15724` and started `Ascension.exe` (stopped again; the realmlist was restored). The login window is **not** prefilled: name and password are copied/shown in the Manager |
| **B** | The same player again | pass: `created:false, reset:false`, same name, same password, still one account |
| **C** | Occupied name | pass: a second player asking for `Dmitry` got `DMITRY_7FYD` (stable suffix, valid on the realm); after losing the credentials the same name came back with a new password (`reset:true`), the old one stopped working, no second account |
| **D** | Legacy character claimed | pass: a character on a legacy account was refused while the account was not at character select, refused while the character was in the world, then claimed and stored as **canonical revision 1** in the player's Manager; the Host recorded the `PortableCharacterId ↔ PlayerIdentity` binding as acknowledged; the listing then marks it portable |
| **E** | Another PlayerIdentity is refused | pass: another identity cannot link the bound account even with its name and password (`account_taken`), cannot claim the character (`not_yours`) and does not see it in its listing |
| **F** | Unmapped legacy account: one-time link | pass: a wrong password is refused (`wrong_credentials`), the right one links once, the next joins reuse the mapping and create no account; the Host holds the mapping, not the password |
| **G** | A Host on another PC behaves the same | **pass in effect, not on a second physical PC** (none was available): the Player Managers are separate processes with separate data folders that have no realm descriptor, and neither the realm's database password nor its console password nor a `db_password`/`ra_password` key appears in any file of any Player (53 files); no player's password appears in the Host's data (22 files); the whole run above crossed the Internet through the VPS. A Host elsewhere differs only in the machine |
| **H** | Manager restart keeps identity, mapping, protected credentials | pass: after stopping and starting both the Host and a Player Manager the PlayerId, the linked account (still *linked*), the password (same hash), the Host's mapping and claim and the player's character are intact; the Host re-linked to the Coordinator by itself; joining created nothing new. A Coordinator restart: the Host link came back in 5 s and joining worked at once; a Registry restart did not disturb joining |
| **I** | VPS inspection | pass: the Coordinator's logs, the Registry's logs, Caddy's logs, a full `pg_dump` of the Registry database, the Coordinator container's settings and every non-secret file under `/opt/coa` contain **none** of: the players' passwords, the legacy password, the realm's database password, the account names `DMITRY`/`LEGACYA`, the character's name, the word `payload`, the descriptor key `ra_password`; the Coordinator's role can read `realms.public_key` and cannot read `display_name`, insert or update; the Coordinator container has a read-only root; the node still listens on 22, 80 and 443 only. (The wire itself is covered by `a_new_player_gets_an_account_and_keeps_it` in `services/coa-control-e2e`: a recording tap on the WebSocket sees 500+ bytes of traffic and none of the password, the name, `Provision` or `password`; and by the character-claim test for the payload)|
| **J** | Phases 1–10 still pass | pass: `cargo test --workspace` 595 passed / 0 failed (48 ignored: they need live fixtures); Registry 14, Registry e2e 12, Coordinator 11, control e2e 10; the Phase-9 live suite against a real worldserver (`live_service`: *make portable on a running realm is armed without a restart*, *a restart with nothing to report makes no revision*, *one-click play of a level 80 on a cap 60 realm survives navigation, a restart and the logout*) 3/3; the Phase 10 live gates 1–12 and the restart gates 2/14 re-run against protocol 2. One pre-existing test (`ra::tests::creates_an_account_over_ra_…`, a loopback fake of the console) failed once in a full parallel run and passes alone and on re-run: the flake that was already known |
| K | The owner's choice shows in the public list | pass: *existing accounts only* appears as `{automatic:false, existing_only:true}` in the Registry's record within two heartbeats and back to automatic when switched off |

**What was simulated.** "At the character-select screen" is what the worldserver records as `account.online = 1` with no `characters.online` row for the account. The gate set `account.online` itself (and the character's `online`
for the in-world refusal) instead of logging in with the game: the Manager must not drive the game's window, and nothing here automates it. The Host's check reads exactly those two facts from the realm's database; a real session sets the first. The claim code path
(listing, ownership, eligibility, export, hash-verified adoption, acknowledgement) was run for real.

## 12. Threat notes

| Threat | Answer |
|---|---|
| A compromised or curious Coordinator | reads nothing (Noise AEAD), changes nothing (authenticated), cannot impersonate a Host (needs the realm key's signature over the channel's own hash) or a Player (needs the player key); it can refuse, delay, cut and count. It stores nothing |
| A man in the middle between Player and Host | two handshakes means two different `h`; the Host proof covers only the real one, so the Player refuses and sends nothing |
| A Registry that names a wrong key for a realm | refused after the first visit (pinned); before it, the Registry is the trust anchor of "this id has this key" (the same anchor as for publishing) |
| Replay of a hello | the Coordinator's per-connection nonce and a 120 s window; the Noise handshake is fresh per channel |
| A player using another player's id | the Host binds id → key at first use; the proof is a signature over the channel hash, so a recorded proof is useless |
| A player claiming a character that is not theirs, or one already claimed, or one that is in the world | refused (section 7), including when another PlayerIdentity knows the account's name and password (the account is bound) |
| Guessing an account's password through *link* | 5 failures per 10 minutes per player id, 30 per realm; the Coordinator limits channels per address and per realm |
| A hostile or broken Host | sees only what a Host must (section 1); a character payload is verified (hash, format, id) before it is stored; one bad Host cannot reach another realm's credentials (they are per realm) |
| A flood | per-address and per-realm channel limits, a byte budget, lifetimes, frame and request size limits, strict JSON; the Host handles requests one at a time and caps a channel at 60 requests |
| Local theft of the player's files | the identity key and passwords are DPAPI-protected on Windows; on other systems only file permissions (stated) |

## 13. What remains before Relay

1. **TLS** on the node (needs a domain; no custom PKI). Until then the WebSocket is plain, which is safe for what it carries (everything private is inside the Noise channel) but exposes metadata to an on-path observer.
2. **A game route**: Relay or a direct route. Today only an owner-stated address makes `Join` start the game.
3. **Moving a portable character onto a realm that is not its home** (import, update, session prepare over the channel): the Host side exists in the engine (Phases 7–9) but the messages to carry an offer and its answers between two Managers are not defined; `needs_transfer` says so.
4. **Native login prefill**: the owner's audit run (section 10).
5. The Host's realm key into the `SecretStore` (section 8), and a keyring store on Linux/macOS.
6. A real login at the character-select screen is not driven by the Manager (by design); the gate set `account.online` the way the worldserver does (see the results).
