//! Ascension client `GlueConfig.json` reader, writer and login prefill.
//!
//! Ascension stores saved accounts in `<client>/WTF/Custom/GlueConfig.json`.
//! The file format is:
//! 1. Data structure: array of accounts `[[username, password, autoLogin?], ...]`.
//! 2. Serialization: LibSerialize v1 binary format.
//! 3. Compression: raw DEFLATE (`wbits = -15`).
//! 4. Encoding: custom 6-bit Base64 character set (`a-zA-Z0-9()`).
//!
//! On client startup, `AccountLogin.lua` calls `GetLastAccount()` which reads the first
//! element of `GlueConfig.json` and automatically prefills `AccountLoginAccountEdit`
//! and `AccountLoginPasswordEdit`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::error::{Error, Result};
use crate::fsx;

pub const CUSTOM_ALPHABET: &[u8; 64] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789()";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAccount {
    pub username: String,
    pub password: String,
    pub auto_login: Option<bool>,
}

impl SavedAccount {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            auto_login: None,
        }
    }
}

pub fn encode_for_print(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    let n = data.len();
    while i + 2 < n {
        let x1 = data[i] as u32;
        let x2 = data[i + 1] as u32;
        let x3 = data[i + 2] as u32;
        let mut cache = x1 | (x2 << 8) | (x3 << 16);
        for _ in 0..4 {
            out.push(CUSTOM_ALPHABET[(cache % 64) as usize] as char);
            cache /= 64;
        }
        i += 3;
    }
    if i < n {
        let mut cache = 0u32;
        let mut bitlen: usize = 0;
        while i < n {
            cache |= (data[i] as u32) << bitlen;
            bitlen += 8;
            i += 1;
        }
        while bitlen > 0 {
            out.push(CUSTOM_ALPHABET[(cache % 64) as usize] as char);
            cache /= 64;
            bitlen = bitlen.saturating_sub(6);
        }
    }
    out
}

fn decode_char(b: u8) -> Option<u8> {
    match b {
        b'a'..=b'z' => Some(b - b'a'),
        b'A'..=b'Z' => Some(b - b'A' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'(' => Some(62),
        b')' => Some(63),
        _ => None,
    }
}

pub fn decode_for_print(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bit_buf = 0u64;
    let mut bit_cnt = 0;
    for b in s.bytes() {
        let Some(val) = decode_char(b) else { continue };
        bit_buf |= (val as u64) << bit_cnt;
        bit_cnt += 6;
        while bit_cnt >= 8 {
            out.push((bit_buf & 0xFF) as u8);
            bit_buf >>= 8;
            bit_cnt -= 8;
        }
    }
    out
}

pub fn compress_deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::best());
    enc.write_all(data)
        .map_err(|e| Error::Invalid(format!("deflate compress error: {e}")))?;
    enc.finish()
        .map_err(|e| Error::Invalid(format!("deflate finish error: {e}")))
}

pub fn decompress_deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut dec = DeflateDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| Error::Invalid(format!("deflate decompress error: {e}")))?;
    Ok(out)
}

struct LibSerializeReader<'a> {
    data: &'a [u8],
    pos: usize,
    string_refs: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum LibValue {
    Nil,
    Bool(bool),
    Num(i64),
    Str(String),
    Array(Vec<LibValue>),
}

impl<'a> LibSerializeReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            string_refs: Vec::new(),
        }
    }

    fn read_byte(&mut self) -> Result<u8> {
        if self.pos >= self.data.len() {
            return Err(Error::Invalid("LibSerialize unexpected EOF".into()));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_int(&mut self, n: usize) -> Result<u64> {
        let mut res = 0u64;
        for i in 0..n {
            let b = self.read_byte()?;
            res |= (b as u64) << (i * 8);
        }
        Ok(res)
    }

    fn read_string(&mut self, len: usize) -> Result<String> {
        if self.pos + len > self.data.len() {
            return Err(Error::Invalid(
                "LibSerialize string exceeds buffer length".into(),
            ));
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        let s = String::from_utf8_lossy(slice).into_owned();
        if len > 2 {
            self.string_refs.push(s.clone());
        }
        Ok(s)
    }

    fn read_value(&mut self) -> Result<LibValue> {
        let val = self.read_byte()?;
        if val % 2 == 1 {
            return Ok(LibValue::Num(((val - 1) / 2) as i64));
        }
        if val % 4 == 2 {
            let typ = (val - 2) / 4;
            let count = (typ / 4) as usize;
            let typ = typ % 4;
            return match typ {
                0 => Ok(LibValue::Str(self.read_string(count)?)),
                1 => {
                    // Map/table: skip key/value pairs
                    for _ in 0..count {
                        self.read_value()?;
                        self.read_value()?;
                    }
                    Ok(LibValue::Nil)
                }
                2 => {
                    let mut arr = Vec::with_capacity(count);
                    for _ in 0..count {
                        arr.push(self.read_value()?);
                    }
                    Ok(LibValue::Array(arr))
                }
                _ => Ok(LibValue::Nil),
            };
        }
        if val % 8 == 4 {
            let b2 = self.read_byte()? as u16;
            let packed = (b2 * 256) + (val as u16);
            let num = if val % 16 == 12 {
                -(((packed - 12) / 16) as i64)
            } else {
                ((packed - 4) / 16) as i64
            };
            return Ok(LibValue::Num(num));
        }
        let typ = val / 8;
        match typ {
            0 => Ok(LibValue::Nil),
            1 => Ok(LibValue::Num(self.read_int(2)? as i64)),
            2 => Ok(LibValue::Num(-(self.read_int(2)? as i64))),
            12 => Ok(LibValue::Bool(true)),
            13 => Ok(LibValue::Bool(false)),
            14 => {
                let len = self.read_byte()? as usize;
                Ok(LibValue::Str(self.read_string(len)?))
            }
            15 => {
                let len = self.read_int(2)? as usize;
                Ok(LibValue::Str(self.read_string(len)?))
            }
            17 => {
                let count = self.read_byte()? as usize;
                for _ in 0..count {
                    self.read_value()?;
                    self.read_value()?;
                }
                Ok(LibValue::Nil)
            }
            20 => {
                let count = self.read_byte()? as usize;
                let mut arr = Vec::with_capacity(count);
                for _ in 0..count {
                    arr.push(self.read_value()?);
                }
                Ok(LibValue::Array(arr))
            }
            26 => {
                let ref_idx = self.read_byte()? as usize;
                if ref_idx == 0 || ref_idx > self.string_refs.len() {
                    return Err(Error::Invalid("invalid string ref index".into()));
                }
                Ok(LibValue::Str(self.string_refs[ref_idx - 1].clone()))
            }
            _ => Ok(LibValue::Nil),
        }
    }
}

pub fn deserialize_accounts(bytes: &[u8]) -> Result<Vec<SavedAccount>> {
    if bytes.is_empty() || bytes[0] != 1 {
        return Err(Error::Invalid("unsupported LibSerialize version".into()));
    }
    let mut reader = LibSerializeReader::new(&bytes[1..]);
    let root = reader.read_value()?;
    let LibValue::Array(entries) = root else {
        return Ok(Vec::new());
    };
    let mut accounts = Vec::new();
    for entry in entries {
        if let LibValue::Array(items) = entry {
            if items.len() >= 2 {
                let username = match &items[0] {
                    LibValue::Str(s) => s.clone(),
                    _ => continue,
                };
                let password = match &items[1] {
                    LibValue::Str(s) => s.clone(),
                    _ => continue,
                };
                let auto_login = match items.get(2) {
                    Some(LibValue::Bool(b)) => Some(*b),
                    _ => None,
                };
                accounts.push(SavedAccount {
                    username,
                    password,
                    auto_login,
                });
            }
        }
    }
    Ok(accounts)
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len <= 15 {
        buf.push(((len << 4) | 2) as u8);
    } else if len <= 255 {
        buf.push(112); // STR_8 (14 << 3)
        buf.push(len as u8);
    } else {
        buf.push(120); // STR_16 (15 << 3)
        buf.push((len & 0xFF) as u8);
        buf.push(((len >> 8) & 0xFF) as u8);
    }
    buf.extend_from_slice(bytes);
}

pub fn serialize_accounts(accounts: &[SavedAccount]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(1); // LibSerialize version 1
    let n = accounts.len();
    if n <= 15 {
        buf.push(((n << 4) | 10) as u8);
    } else {
        buf.push(160); // ARRAY_8 (20 << 3)
        buf.push(n.min(255) as u8);
    }
    for acc in accounts {
        let entry_len = if acc.auto_login.is_some() { 3 } else { 2 };
        buf.push(((entry_len << 4) | 10) as u8);
        write_string(&mut buf, &acc.username);
        write_string(&mut buf, &acc.password);
        if let Some(auto) = acc.auto_login {
            buf.push(if auto { 96 } else { 104 }); // BOOL_T = 96 (12 << 3), BOOL_F = 104 (13 << 3)
        }
    }
    buf
}

pub fn glue_config_path(client: &Path) -> PathBuf {
    client.join("WTF").join("Custom").join("GlueConfig.json")
}

pub fn read_glue_config(client: &Path) -> Result<Vec<SavedAccount>> {
    let path = glue_config_path(client);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| Error::Invalid(format!("failed to read GlueConfig.json: {e}")))?;
    let compressed = decode_for_print(text.trim());
    if compressed.is_empty() {
        return Ok(Vec::new());
    }
    let decompressed = decompress_deflate(&compressed)?;
    deserialize_accounts(&decompressed)
}

pub fn write_glue_config(client: &Path, accounts: &[SavedAccount]) -> Result<()> {
    let path = glue_config_path(client);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serialize_accounts(accounts);
    let compressed = compress_deflate(&serialized)?;
    let encoded = encode_for_print(&compressed);
    fsx::atomic_write(&path, encoded.as_bytes())
}

/// Prefill the client login box with the given username and password.
///
/// If `GlueConfig.json` exists, any previous entries for this username are updated and moved to the
/// head of the list, preserving all other saved accounts. If it does not exist, a new config is created.
pub fn prefill_login(client: &Path, username: &str, password: &str) -> Result<()> {
    if username.is_empty() || password.is_empty() {
        return Ok(());
    }
    let mut accounts = read_glue_config(client).unwrap_or_default();
    let auto_login = accounts
        .iter()
        .find(|a| a.username.eq_ignore_ascii_case(username))
        .and_then(|a| a.auto_login);
    accounts.retain(|a| !a.username.eq_ignore_ascii_case(username));
    accounts.insert(
        0,
        SavedAccount {
            username: username.to_string(),
            password: password.to_string(),
            auto_login,
        },
    );
    write_glue_config(client, &accounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_encoding_roundtrip() {
        let sample = b"hello world 1234567890!@#$%^&*()_+";
        let enc = encode_for_print(sample);
        let dec = decode_for_print(&enc);
        assert_eq!(dec, sample);
    }

    #[test]
    fn test_deflate_roundtrip() {
        let sample = b"repeated text repeated text repeated text 12345";
        let comp = compress_deflate(sample).unwrap();
        let decomp = decompress_deflate(&comp).unwrap();
        assert_eq!(decomp, sample);
    }

    #[test]
    fn test_known_python_vector() {
        let encoded = "JrP0QPqCn4qcnyxdcmJaCm4Gba";
        let comp = decode_for_print(encoded);
        let decomp = decompress_deflate(&comp).unwrap();
        let accounts = deserialize_accounts(&decomp).unwrap();
        assert_eq!(
            accounts,
            vec![SavedAccount {
                username: "TESTUSER".into(),
                password: "TESTPASS".into(),
                auto_login: None,
            }]
        );
    }

    #[test]
    fn test_accounts_roundtrip() {
        let accounts = vec![
            SavedAccount {
                username: "Alice".into(),
                password: "secret_password".into(),
                auto_login: None,
            },
            SavedAccount {
                username: "Bob".into(),
                password: "another_password".into(),
                auto_login: Some(true),
            },
        ];
        let ser = serialize_accounts(&accounts);
        let de = deserialize_accounts(&ser).unwrap();
        assert_eq!(de, accounts);
    }

    #[test]
    fn test_prefill_login_logic() {
        let tmp = tempfile::tempdir().unwrap();
        let client_path = tmp.path();

        prefill_login(client_path, "PlayerOne", "pass1").unwrap();
        let accounts = read_glue_config(client_path).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "PlayerOne");
        assert_eq!(accounts[0].password, "pass1");

        // Adding PlayerTwo moves it to head
        prefill_login(client_path, "PlayerTwo", "pass2").unwrap();
        let accounts = read_glue_config(client_path).unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].username, "PlayerTwo");
        assert_eq!(accounts[1].username, "PlayerOne");

        // Re-adding PlayerOne updates password and moves it back to head
        prefill_login(client_path, "playerone", "newpass").unwrap();
        let accounts = read_glue_config(client_path).unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].username, "playerone");
        assert_eq!(accounts[0].password, "newpass");
        assert_eq!(accounts[1].username, "PlayerTwo");
    }

    #[test]
    fn test_real_ascension_glue_config_payload() {
        let raw = "JXzPvbsKEQ5xu6zMC2jNrI5LAooKEUjMzo6LC(5wGiPLFkPKFX6wyMfME(MRBEPME2rwLwb5J)o7OpxGzTMKZ5xuAzwuQjM3Mm3ctm2ulnZmJDmjTckN8toXCUaBw5ujPwCjfbIWqJmobTkkGqCpuxdoKMki)IkjXKYjvTskGq8WFFD9cCPvsfKtIvMArfiqao7Pfo7TLoAKr(7zGHzMNAfuIPKBM5Db(am";
        let comp = decode_for_print(raw);
        let decomp = decompress_deflate(&comp).unwrap();
        let accounts = deserialize_accounts(&decomp).unwrap();
        assert_eq!(accounts.len(), 9);
        assert_eq!(accounts[0].username, "georgickchanel@gmail.com");
        assert_eq!(accounts[0].password, "foddos-qiqgo7-negkYz");
        assert_eq!(accounts[1].username, "LOCAL");
        assert_eq!(accounts[1].password, "LOCAL");
        assert_eq!(accounts[2].username, "Corfirean");
        assert_eq!(accounts[2].password, "784359663Ab");
        assert_eq!(accounts[3].username, "local");
        assert_eq!(accounts[3].password, "local");
        assert_eq!(accounts[4].username, "test");
        assert_eq!(accounts[4].password, "test123");
        assert_eq!(accounts[4].auto_login, Some(true));
        assert_eq!(accounts[5].username, "PTGUEST");
        assert_eq!(accounts[5].password, "portable");
        assert_eq!(accounts[6].username, "PTHOME");
        assert_eq!(accounts[6].password, "portable");
        assert_eq!(accounts[7].username, "player");
        assert_eq!(accounts[7].password, "PCI8CK9A22OIQV6I");
        assert_eq!(accounts[8].username, "admin");
        assert_eq!(accounts[8].password, "admin");
    }
}
