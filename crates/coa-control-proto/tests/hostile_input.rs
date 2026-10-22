//! Adversarial, property and hostile-input fuzz tests for Phase 15.
//! Verifies that malformed or malicious inputs never panic, integer operations
//! do not wrap, allocations remain bounded, and invalid states fail closed.

use coa_control_proto::app::{
    decode_request, decode_response, encode_request, Request, MAX_TRANSFER_REQUEST_BYTES,
};
use coa_control_proto::coord::{
    parse_text, split_binary, Frame, ProbePayload, CONN_PREFIX, MAX_FRAME_BYTES,
};
use coa_control_proto::noise::{Initiator, Responder, CHUNK};
use coa_control_proto::relay::{
    rewrite_realm_list_address, sign_relay_challenge, verify_relay_challenge, TunnelMsg,
};
use coa_registry_proto::sign::RealmId;
use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use uuid::Uuid;

#[test]
fn test_cmd_realm_list_rewriter_adversarial_and_bounds() {
    let dummy_target = "127.0.0.1:8085";

    // 1. Truncated inputs
    assert!(rewrite_realm_list_address(&[], dummy_target)
        .unwrap()
        .is_none());
    assert!(rewrite_realm_list_address(&[0x10], dummy_target).is_err());
    assert!(rewrite_realm_list_address(&[0x10, 0x05, 0x00], dummy_target).is_err());
    assert!(
        rewrite_realm_list_address(&[0x10, 0x10, 0x00, 0, 0, 0, 0, 1, 0], dummy_target).is_err()
    );

    // 2. Non-0x10 opcodes ignored safely
    assert!(rewrite_realm_list_address(&[0x00, 0x01], dummy_target)
        .unwrap()
        .is_none());
    assert!(rewrite_realm_list_address(&[0xFF; 20], dummy_target)
        .unwrap()
        .is_none());

    // 3. Zero realms handled safely
    let zero_realms = [
        0x10, // Opcode
        0x06, 0x00, // Length: 6 bytes follow
        0x00, 0x00, 0x00, 0x00, // Unused
        0x00, 0x00, // Realm count: 0
    ];
    let res = rewrite_realm_list_address(&zero_realms, dummy_target).unwrap();
    assert_eq!(res.unwrap(), zero_realms);

    // 4. Missing null terminators in realm string
    let unterminated_name = [
        0x10, 0x20, 0x00, 0, 0, 0, 0, 1, 0, // Header (count = 1)
        1, 0, 0, // type, lock, flag
        b'R', b'e', b'a', b'l', b'm', // no null byte!
    ];
    assert!(rewrite_realm_list_address(&unterminated_name, dummy_target).is_err());

    // 5. Excessive new address (> 255 bytes) rejected
    let giant_addr = "a".repeat(256);
    let valid_packet = make_valid_realm_list_pkt("1.2.3.4:1234");
    assert!(rewrite_realm_list_address(&valid_packet, &giant_addr).is_err());

    // 6. Valid rewrite preserves packet structure
    let rewritten = rewrite_realm_list_address(&valid_packet, "99.88.77.66:30000")
        .unwrap()
        .unwrap();
    assert_eq!(rewritten[0], 0x10);
    let s = String::from_utf8_lossy(&rewritten);
    assert!(s.contains("99.88.77.66:30000"));
}

#[test]
fn test_app_request_hostile_limits_and_validation() {
    // 1. Oversized control request (> 4096 bytes)
    let big_client = "A".repeat(5000);
    let big_hello = format!(r#"{{"op":"hello","protocol":1,"client":"{big_client}"}}"#);
    assert!(decode_request(big_hello.as_bytes()).is_err());

    // 2. Unknown fields rejected
    let extra_field = r#"{"op":"claim","token":1,"unknown_admin_flag":true}"#;
    assert!(decode_request(extra_field.as_bytes()).is_err());

    // 3. Giant collection count in TransferOffer rejected
    let mut collections = BTreeMap::new();
    for i in 0..1500 {
        collections.insert(format!("cat_{i}"), vec![1, 2, 3]);
    }
    let offer = Request::TransferOffer {
        transfer_id: Uuid::now_v7(),
        character_id: Uuid::now_v7(),
        canonical_revision: 1,
        content_hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        total_size: 1024,
        collections,
    };
    let offer_bytes = encode_request(&offer);
    assert!(
        decode_request(&offer_bytes).is_err(),
        "offers with > 1000 categories must be rejected"
    );

    // 4. Invalid content hash (not 64 hex chars)
    let bad_hash_offer = Request::TransferOffer {
        transfer_id: Uuid::now_v7(),
        character_id: Uuid::now_v7(),
        canonical_revision: 1,
        content_hash: "not_a_valid_hex_hash".into(),
        total_size: 1024,
        collections: BTreeMap::new(),
    };
    assert!(decode_request(&encode_request(&bad_hash_offer)).is_err());

    // 5. TransferChunk offset out of bounds
    let bad_offset_chunk = Request::TransferChunk {
        transfer_id: Uuid::now_v7(),
        offset: MAX_TRANSFER_REQUEST_BYTES + 1,
        data: "abc".into(),
    };
    assert!(decode_request(&encode_request(&bad_offset_chunk)).is_err());
}

#[test]
fn test_coordinator_text_and_binary_adversarial() {
    // 1. Oversized text frame (> 2048 bytes)
    let giant_text = format!(
        r#"{{"t":"close","conn":1,"reason":"{}"}}"#,
        "x".repeat(3000)
    );
    assert!(parse_text(&giant_text).is_err());

    // 2. Binary frame bounds
    assert!(split_binary(&[]).is_err());
    assert!(split_binary(&[0, 1, 2]).is_err()); // less than 4 bytes
    let max_allowed = vec![0u8; CONN_PREFIX + MAX_FRAME_BYTES];
    assert!(split_binary(&max_allowed).is_ok());
    let too_large = vec![0u8; CONN_PREFIX + MAX_FRAME_BYTES + 1];
    assert!(split_binary(&too_large).is_err());

    // 3. ProbePayload unknown fields rejected
    let bad_probe = r#"{"ports":[8085],"admin":true}"#;
    assert!(serde_json::from_str::<ProbePayload>(bad_probe).is_err());
}

#[test]
fn test_noise_frame_reassembly_adversarial() {
    let (init, msg1) = Initiator::start().unwrap();
    let (resp, msg2) = Responder::start(&msg1).unwrap();
    let (msg3, mut client_chan) = init.finish(&msg2).unwrap();
    let mut server_chan = resp.finish(&msg3).unwrap();

    // 1. Normal message seals and opens
    let frames = client_chan.seal(b"hello world").unwrap();
    assert_eq!(frames.len(), 1);
    let opened = server_chan.open_frame(&frames[0]).unwrap().unwrap();
    assert_eq!(opened, b"hello world");

    // 2. Corrupted ciphertext ends/fails without panicking
    let mut corrupted = frames[0].clone();
    corrupted[10] ^= 0xFF;
    assert!(server_chan.open_frame(&corrupted).is_err());

    // 3. Truncated frame (< 17 bytes)
    assert!(server_chan.open_frame(&[0u8; 16]).is_err());

    // 4. Oversized frame (> CHUNK + 17)
    let giant_frame = vec![0u8; CHUNK + 18];
    assert!(server_chan.open_frame(&giant_frame).is_err());
}

#[test]
fn test_relay_protocol_adversarial() {
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let realm_id = RealmId::new();
    let nonce = "challenge_nonce_12345";

    let sig = sign_relay_challenge(&key, &realm_id, nonce);
    assert!(verify_relay_challenge(
        &key.verifying_key(),
        &realm_id,
        nonce,
        &sig
    ));

    // Tampered nonce
    assert!(!verify_relay_challenge(
        &key.verifying_key(),
        &realm_id,
        "different_nonce",
        &sig
    ));
    // Tampered realm_id
    assert!(!verify_relay_challenge(
        &key.verifying_key(),
        &RealmId::new(),
        nonce,
        &sig
    ));
    // Corrupted signature
    assert!(!verify_relay_challenge(
        &key.verifying_key(),
        &realm_id,
        nonce,
        "not_valid_hex"
    ));

    // TunnelMsg unknown fields rejected
    let bad_tunnel = r#"{"cmd":"close","stream_id":1,"extra":true}"#;
    assert!(serde_json::from_str::<TunnelMsg>(bad_tunnel).is_err());
}

#[test]
fn test_fuzz_smoke_never_panics_on_arbitrary_bytes() {
    // Deterministic pseudo-random bytes
    let mut seed: u64 = 0xDEADBEEFCAFEBABE;
    let mut next_bytes = |len: usize| -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        for _ in 0..len {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            v.push((seed >> 33) as u8);
        }
        v
    };

    for len in [0, 1, 2, 3, 4, 8, 16, 32, 64, 128, 256, 1024, 4096] {
        for _ in 0..50 {
            let sample = next_bytes(len);
            // Must not panic on any random input:
            let _ = decode_request(&sample);
            let _ = decode_response(&sample);
            let _ = split_binary(&sample);
            let _ = rewrite_realm_list_address(&sample, "127.0.0.1:8085");
            if let Ok(s) = std::str::from_utf8(&sample) {
                let _ = parse_text(s);
                let _ = serde_json::from_str::<TunnelMsg>(s);
                let _ = serde_json::from_str::<Frame>(s);
            }
        }
    }
}

fn make_valid_realm_list_pkt(addr: &str) -> Vec<u8> {
    let name = b"CoA Realm\0";
    let addr_bytes = format!("{addr}\0").into_bytes();
    let body_len = 4 + 2 + 3 + name.len() + addr_bytes.len() + 4 + 1 + 1 + 1 + 2;

    let mut pkt = Vec::new();
    pkt.push(0x10); // Opcode
    pkt.extend_from_slice(&(body_len as u16).to_le_bytes()); // Length
    pkt.extend_from_slice(&0u32.to_le_bytes()); // Unused
    pkt.extend_from_slice(&1u16.to_le_bytes()); // Realm count = 1

    // Realm info:
    pkt.extend_from_slice(&[1, 0, 0]); // type, lock, flag
    pkt.extend_from_slice(name);
    pkt.extend_from_slice(&addr_bytes);
    pkt.extend_from_slice(&[0, 0, 0, 0]); // pop
    pkt.extend_from_slice(&[0]); // chars
    pkt.extend_from_slice(&[1]); // timezone
    pkt.extend_from_slice(&[1]); // realm id

    // Footer:
    pkt.extend_from_slice(&[0x10, 0x00]);
    pkt
}
