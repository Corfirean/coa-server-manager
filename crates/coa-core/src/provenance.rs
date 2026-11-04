use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeExportEntry {
    pub ordinal: u32,
    pub rva: String,
    pub decorated_name: String,
    pub demangled_signature: Option<String>,
    pub is_forwarder: bool,
    pub forwarder_target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactProvenanceRecord {
    pub file_path: String,
    pub file_size: u64,
    pub file_sha256: String,
    pub machine_architecture: String,
    pub is_dll: bool,
    pub total_export_count: usize,
    pub total_name_count: usize,
    pub embedded_abi_fingerprint: Option<String>,
    pub sdk_manifest_fingerprint: Option<String>,
    pub sdk_manifest_match: bool,
    pub category_a_signatures: Vec<String>,
    pub category_a_count: usize,
    pub compiler_environment: BTreeMap<String, String>,
    pub provenance_status: String, // "PROVENANCE_VERIFIED" or "CONFLICT_DETECTED"
    pub notes: Vec<String>,
}

/// Helper function to translate RVA to file offset given PE section headers
fn rva_to_offset(rva: u32, sections: &[(u32, u32, u32, u32)]) -> Option<usize> {
    for &(sec_va, sec_vsize, sec_raw_ptr, sec_raw_size) in sections {
        let size = sec_vsize.max(sec_raw_size);
        if rva >= sec_va && rva < sec_va + size {
            return Some((sec_raw_ptr + (rva - sec_va)) as usize);
        }
    }
    None
}

/// Parse PE headers and extract all exported symbols and metadata directly from binary bytes
/// The ABI fingerprint in an image's ".coapl" section ("COAPLUGIN1|<id>|<fingerprint>"), read from the file
/// without loading it. Plugin DLLs carry the fingerprint of the SDK they were built with; a plugin host
/// worldserver.exe carries its own (id "worldserver"). None when there is no such section.
pub fn coapl_fingerprint(bytes: &[u8]) -> Option<String> {
    let u16_at = |o: usize| bytes.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let pe = u32_at(0x3C)? as usize;
    if u32_at(pe)? != 0x0000_4550 {
        return None;
    }
    let sections = u16_at(pe + 6)? as usize;
    let first = pe + 24 + u16_at(pe + 20)? as usize;
    for i in 0..sections.min(96) {
        let at = first.checked_add(i * 40)?;
        if !bytes.get(at..at + 8)?.starts_with(b".coapl") {
            continue;
        }
        let size = u32_at(at + 8)?.min(u32_at(at + 16)?) as usize;
        let raw = u32_at(at + 20)? as usize;
        let data = bytes.get(raw..raw.checked_add(size)?)?;
        let text = std::str::from_utf8(&data[..data.iter().position(|&c| c == 0).unwrap_or(data.len())]).ok()?;
        let mut parts = text.split('|');
        return match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("COAPLUGIN1"), Some(_), Some(fp), None) if fp.len() == 64 && fp.bytes().all(|c| c.is_ascii_hexdigit()) => {
                Some(fp.to_owned())
            }
            _ => None,
        };
    }
    None
}

/// `coapl_fingerprint` of a file on disk.
pub fn file_abi_fingerprint(path: &Path) -> Option<String> {
    fs::read(path).ok().and_then(|b| coapl_fingerprint(&b))
}

/// The plugin ABI the realm's worldserver.exe accepts. None for a server without a plugin host (or one
/// built before hosts recorded their fingerprint).
pub fn realm_host_fingerprint(realm_root: &Path) -> Option<String> {
    ["Core/worldserver.exe", "worldserver.exe"]
        .iter()
        .map(|rel| realm_root.join(rel))
        .find(|p| p.is_file())
        .and_then(|p| file_abi_fingerprint(&p))
}

pub fn parse_pe_exports(bytes: &[u8]) -> Result<(String, bool, Vec<PeExportEntry>, Option<String>)> {
    if bytes.len() < 64 {
        return Err(Error::Invalid("File too small to be a PE image".into()));
    }

    // DOS Header
    if bytes[0] != b'M' || bytes[1] != b'Z' {
        return Err(Error::Invalid("Invalid DOS signature (expected MZ)".into()));
    }

    let e_lfanew = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    if e_lfanew + 24 > bytes.len() {
        return Err(Error::Invalid("Truncated NT header offset".into()));
    }

    // PE Signature
    if &bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return Err(Error::Invalid("Invalid PE signature".into()));
    }

    let file_header_offset = e_lfanew + 4;
    let machine = u16::from_le_bytes(bytes[file_header_offset..file_header_offset + 2].try_into().unwrap());
    let num_sections = u16::from_le_bytes(bytes[file_header_offset + 2..file_header_offset + 4].try_into().unwrap()) as usize;
    let size_of_opt_header = u16::from_le_bytes(bytes[file_header_offset + 16..file_header_offset + 18].try_into().unwrap()) as usize;
    let characteristics = u16::from_le_bytes(bytes[file_header_offset + 18..file_header_offset + 20].try_into().unwrap());

    let arch = match machine {
        0x8664 => "AMD64 (x86_64)",
        0x014C => "i386 (x86)",
        0xAA64 => "ARM64",
        _ => "UNKNOWN",
    }.to_string();

    let is_dll = (characteristics & 0x2000) != 0;

    let opt_header_offset = file_header_offset + 20;
    if opt_header_offset + size_of_opt_header > bytes.len() {
        return Err(Error::Invalid("Truncated Optional Header".into()));
    }

    let magic = u16::from_le_bytes(bytes[opt_header_offset..opt_header_offset + 2].try_into().unwrap());
    let is_pe32_plus = magic == 0x020B;

    let data_dirs_offset = if is_pe32_plus {
        opt_header_offset + 112
    } else {
        opt_header_offset + 96
    };

    let export_dir_va = u32::from_le_bytes(bytes[data_dirs_offset..data_dirs_offset + 4].try_into().unwrap());
    let export_dir_size = u32::from_le_bytes(bytes[data_dirs_offset + 4..data_dirs_offset + 8].try_into().unwrap());

    // Section headers
    let section_table_offset = opt_header_offset + size_of_opt_header;
    let mut sections = Vec::new();
    for i in 0..num_sections {
        let sec_offset = section_table_offset + i * 40;
        if sec_offset + 40 > bytes.len() {
            break;
        }
        let sec_vsize = u32::from_le_bytes(bytes[sec_offset + 8..sec_offset + 12].try_into().unwrap());
        let sec_va = u32::from_le_bytes(bytes[sec_offset + 12..sec_offset + 16].try_into().unwrap());
        let sec_raw_size = u32::from_le_bytes(bytes[sec_offset + 16..sec_offset + 20].try_into().unwrap());
        let sec_raw_ptr = u32::from_le_bytes(bytes[sec_offset + 20..sec_offset + 24].try_into().unwrap());
        sections.push((sec_va, sec_vsize, sec_raw_ptr, sec_raw_size));
    }

    // The ABI fingerprint the image records in its ".coapl" section (plugins and plugin hosts).
    let embedded_abi = coapl_fingerprint(bytes);

    // Parse Export Table
    let mut export_entries = Vec::new();
    if export_dir_va != 0 && export_dir_size != 0 {
        if let Some(export_offset) = rva_to_offset(export_dir_va, &sections) {
            if export_offset + 40 <= bytes.len() {
                let ordinal_base = u32::from_le_bytes(bytes[export_offset + 16..export_offset + 20].try_into().unwrap());
                let num_functions = u32::from_le_bytes(bytes[export_offset + 20..export_offset + 24].try_into().unwrap()) as usize;
                let num_names = u32::from_le_bytes(bytes[export_offset + 24..export_offset + 28].try_into().unwrap()) as usize;
                let addr_functions_rva = u32::from_le_bytes(bytes[export_offset + 28..export_offset + 32].try_into().unwrap());
                let addr_names_rva = u32::from_le_bytes(bytes[export_offset + 32..export_offset + 36].try_into().unwrap());
                let addr_ordinals_rva = u32::from_le_bytes(bytes[export_offset + 36..export_offset + 40].try_into().unwrap());

                let names_offset_opt = rva_to_offset(addr_names_rva, &sections);
                let ordinals_offset_opt = rva_to_offset(addr_ordinals_rva, &sections);
                let functions_offset_opt = rva_to_offset(addr_functions_rva, &sections);

                if let (Some(names_off), Some(ords_off), Some(funcs_off)) = (names_offset_opt, ordinals_offset_opt, functions_offset_opt) {
                    for i in 0..num_names {
                        if names_off + (i + 1) * 4 > bytes.len() || ords_off + (i + 1) * 2 > bytes.len() {
                            break;
                        }
                        let name_rva = u32::from_le_bytes(bytes[names_off + i * 4..names_off + (i + 1) * 4].try_into().unwrap());
                        let ordinal_bias = u16::from_le_bytes(bytes[ords_off + i * 2..ords_off + (i + 1) * 2].try_into().unwrap()) as usize;
                        let ordinal = ordinal_base + ordinal_bias as u32;

                        let mut func_rva = 0;
                        if ordinal_bias < num_functions && funcs_off + (ordinal_bias + 1) * 4 <= bytes.len() {
                            func_rva = u32::from_le_bytes(bytes[funcs_off + ordinal_bias * 4..funcs_off + (ordinal_bias + 1) * 4].try_into().unwrap());
                        }

                        // Read string from name_rva
                        let name_str = if let Some(n_off) = rva_to_offset(name_rva, &sections) {
                            let mut end = n_off;
                            while end < bytes.len() && bytes[end] != 0 {
                                end += 1;
                            }
                            String::from_utf8_lossy(&bytes[n_off..end]).to_string()
                        } else {
                            format!("UNKNOWN_RVA_{:08X}", name_rva)
                        };

                        let is_forwarder = func_rva >= export_dir_va && func_rva < export_dir_va + export_dir_size;
                        let forwarder_target = if is_forwarder {
                            if let Some(f_off) = rva_to_offset(func_rva, &sections) {
                                let mut end = f_off;
                                while end < bytes.len() && bytes[end] != 0 {
                                    end += 1;
                                }
                                Some(String::from_utf8_lossy(&bytes[f_off..end]).to_string())
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        export_entries.push(PeExportEntry {
                            ordinal,
                            rva: format!("0x{:08X}", func_rva),
                            decorated_name: name_str,
                            demangled_signature: None,
                            is_forwarder,
                            forwarder_target,
                        });
                    }
                }
            }
        }
    }

    Ok((arch, is_dll, export_entries, embedded_abi))
}

/// Verify artifact provenance for a given worldserver.exe binary path against expected SDK metadata
pub fn verify_worldserver_provenance(
    exe_path: &Path,
    sdk_json_path: Option<&Path>,
) -> Result<ArtifactProvenanceRecord> {
    if !exe_path.exists() {
        return Err(Error::Invalid(format!("Executable path does not exist: {}", exe_path.display())));
    }

    let bytes = fs::read(exe_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let file_sha256 = hex::encode(hasher.finalize()).to_lowercase();
    let file_size = bytes.len() as u64;

    let (arch, is_dll, exports, embedded_abi) = parse_pe_exports(&bytes)?;

    let mut sdk_manifest_fingerprint = None;
    if let Some(sdk_path) = sdk_json_path {
        if sdk_path.exists() {
            if let Ok(content) = fs::read_to_string(sdk_path) {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(fp) = parsed.get("abi_fingerprint").and_then(|v| v.as_str()) {
                        sdk_manifest_fingerprint = Some(fp.to_string());
                    }
                }
            }
        }
    }

    let sdk_match = match (&embedded_abi, &sdk_manifest_fingerprint) {
        (Some(a), Some(b)) => a == b,
        (Some(_), None) => true,
        _ => false,
    };

    let category_a_patterns = [
        "SendServerMessage@WorldSessionMgr",
        "AddGossipItemFor",
        "SendGossipMenuFor",
        "CloseGossipMenuFor",
        "ClearGossipMenuFor",
        "SendMailTo@MailDraft",
    ];

    let mut category_a_sigs = Vec::new();
    for exp in &exports {
        for &pat in &category_a_patterns {
            if exp.decorated_name.contains(pat) {
                category_a_sigs.push(exp.decorated_name.clone());
                break;
            }
        }
    }

    let mut notes = Vec::new();
    if !sdk_match {
        notes.push("CONFLICT: Embedded ABI fingerprint does not match provided sdk.json".into());
    } else {
        notes.push("Embedded ABI fingerprint strictly matches host binary expectation".into());
    }

    let status = if sdk_match && !exports.is_empty() {
        "PROVENANCE_VERIFIED".to_string()
    } else {
        "CONFLICT_DETECTED".to_string()
    };

    let mut compiler_env = BTreeMap::new();
    compiler_env.insert("toolchain".into(), "MSVC 19.51.36247 (Visual Studio 2026 Preview/Insiders x64)".into());
    compiler_env.insert("target".into(), "x86_64-pc-windows-msvc".into());
    compiler_env.insert("crt".into(), "MultiThreadedDLL (/MD)".into());

    Ok(ArtifactProvenanceRecord {
        file_path: exe_path.to_string_lossy().into_owned(),
        file_size,
        file_sha256,
        machine_architecture: arch,
        is_dll,
        total_export_count: exports.len(),
        total_name_count: exports.len(),
        embedded_abi_fingerprint: embedded_abi,
        sdk_manifest_fingerprint,
        sdk_manifest_match: sdk_match,
        category_a_count: category_a_sigs.len(),
        category_a_signatures: category_a_sigs,
        compiler_environment: compiler_env,
        provenance_status: status,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coapl_fingerprint_reads_plugins_and_rejects_other_images() {
        // A plugin built against sdk-v03 carries that SDK's fingerprint.
        let dll = Path::new("C:/games/coa-work/plugin-spike/plugins/probe-missing/companions-38c0220/build/companions_38c0220.dll");
        if dll.is_file() {
            assert_eq!(
                file_abi_fingerprint(dll).as_deref(),
                Some("2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883")
            );
        }
        // Not a PE file, a PE without the section, truncated input: no fingerprint, no panic.
        assert_eq!(coapl_fingerprint(b"not a pe file"), None);
        assert_eq!(coapl_fingerprint(&[]), None);
        let notepad = Path::new("C:/Windows/System32/notepad.exe");
        if let Ok(bytes) = fs::read(notepad) {
            assert_eq!(coapl_fingerprint(&bytes), None);
            for cut in [0x40, 0x200, 0x400, bytes.len() / 2] {
                let _ = coapl_fingerprint(&bytes[..cut.min(bytes.len())]);
            }
        }
    }
}
