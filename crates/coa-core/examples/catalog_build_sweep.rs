//! Build the AzerothCore catalogue's modules against a plugin-host realm's SDK and record, per module, the
//! evidence: commit, analysis, build result, compiler errors and the core functions the host does not export.
//! A rerun skips modules that already have a result; `summary` aggregates the results.
//!
//! usage: catalog_build_sweep run <realm> <work dir> <how many, by stars>
//!        catalog_build_sweep summary <work dir>
//! The realm needs only Core/worldserver.exe (a plugin host) and its .manager/sdk; use a disposable copy.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct Outcome {
    slug: String,
    stars: u32,
    category: String,
    pushed_at: Option<String>,
    commit: Option<String>,
    /// "built" | "link-missing-exports" | "compile-error" | "not-a-plugin" | "analysis-failed" | "build-error"
    result: String,
    detail: Option<String>,
    classification: Option<String>,
    seconds: f64,
    /// Core functions the plugin needs that worldserver.exe does not export (MSVC decorated names).
    missing_exports: Vec<String>,
    /// Demangled form of each missing export, for reading.
    missing_readable: Vec<String>,
    compile_errors: Vec<String>,
}

fn file_name(slug: &str) -> String {
    slug.replace('/', "__")
}

/// The decorated name and its readable form out of an LNK2019/LNK2001 message.
fn missing_symbol(message: &str) -> Option<(String, String)> {
    let readable = message.split('"').nth(1).unwrap_or("").to_string();
    let start = message.find("(?")?;
    let end = message[start..].find(')')? + start;
    Some((message[start + 1..end].to_string(), readable))
}

fn run(realm: &Path, work: &Path, count: usize) {
    let results = work.join("results");
    std::fs::create_dir_all(&results).unwrap();
    let sources = work.join("sources");
    let catalogue = coa_core::catalogue::fetch_or_load_catalogue(&work.join("catalogue"), false).expect("catalogue");
    let mut items: Vec<_> = catalogue
        .items
        .into_iter()
        .filter(|i| !matches!(i.coa_status, coa_core::catalogue::CoaCompatibilityStatus::Incompatible | coa_core::catalogue::CoaCompatibilityStatus::Blocked))
        .collect();
    items.sort_by(|a, b| b.stars.cmp(&a.stars));
    let tc = coa_core::build_engine::detect_toolchain_for(Some(realm));
    assert!(tc.msvc_available && tc.cmake_available && tc.ninja_available && tc.sdk_dir.is_some(), "toolchain/SDK for the realm: {tc:?}");

    for (n, item) in items.iter().take(count).enumerate() {
        let out = results.join(format!("{}.json", file_name(&item.full_name)));
        if out.is_file() {
            continue;
        }
        let started = std::time::Instant::now();
        let mut o = Outcome {
            slug: item.full_name.clone(),
            stars: item.stars,
            category: item.category.clone(),
            pushed_at: item.pushed_at.clone(),
            ..Default::default()
        };
        println!("[{}/{}] {} ★{}", n + 1, count.min(items.len()), item.full_name, item.stars);
        match coa_core::analyzer::analyze_github_repository(&item.html_url, Some(&sources)) {
            Err(e) => {
                o.result = "analysis-failed".into();
                o.detail = Some(e.to_string());
            }
            Ok(report) => {
                o.commit = Some(report.commit.clone());
                o.classification = Some(format!("{:?}", report.classification));
                let path = PathBuf::from(&report.repository_path);
                let path = if path.is_dir() { path } else {
                    sources.join("github").join(&item.owner).join(&item.name).join(&report.commit)
                };
                match coa_core::build_engine::build_module_with_bundle(&path, &[], realm, None, None) {
                    Err(e) => {
                        let text = e.to_string();
                        o.result = if text.contains("NO_LOADER_FOUND") || text.contains("No C++") || text.contains("MANUAL_ADAPTER") {
                            "not-a-plugin".into()
                        } else {
                            "build-error".into()
                        };
                        o.detail = Some(text);
                    }
                    Ok(b) if b.success => o.result = "built".into(),
                    Ok(b) => {
                        for e in &b.errors {
                            if e.error_code == "LNK2019" || e.error_code == "LNK2001" {
                                if let Some((mangled, readable)) = missing_symbol(&e.message) {
                                    if !o.missing_exports.contains(&mangled) {
                                        o.missing_exports.push(mangled);
                                        o.missing_readable.push(readable);
                                    }
                                }
                            } else if !e.error_code.starts_with("LNK") {
                                o.compile_errors.push(format!("{} {}:{} {}", e.error_code, e.file.rsplit(['/', '\\']).next().unwrap_or(""), e.line, e.message.lines().next().unwrap_or("")));
                            }
                        }
                        o.result = if o.compile_errors.is_empty() && !o.missing_exports.is_empty() {
                            "link-missing-exports".into()
                        } else {
                            "compile-error".into()
                        };
                        o.compile_errors.truncate(10);
                    }
                }
            }
        }
        o.seconds = started.elapsed().as_secs_f64();
        println!("    -> {} ({:.0}s){}", o.result, o.seconds, if o.missing_exports.is_empty() { String::new() } else { format!(", {} missing exports", o.missing_exports.len()) });
        std::fs::write(&out, serde_json::to_vec_pretty(&o).unwrap()).unwrap();
        // source downloads of finished modules are not needed again
        let _ = std::fs::remove_dir_all(sources.join("github").join(&item.owner).join(&item.name));
    }
}

fn summary(work: &Path) {
    let mut all: Vec<Outcome> = std::fs::read_dir(work.join("results")).unwrap()
        .flatten()
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    all.sort_by(|a, b| b.stars.cmp(&a.stars));
    let mut by_result: BTreeMap<&str, usize> = BTreeMap::new();
    for o in &all {
        *by_result.entry(o.result.as_str()).or_default() += 1;
    }
    let mut symbols: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for o in all.iter().filter(|o| o.result == "link-missing-exports") {
        for (m, r) in o.missing_exports.iter().zip(&o.missing_readable) {
            symbols.entry(m.clone()).or_insert_with(|| (r.clone(), Vec::new())).1.push(o.slug.clone());
        }
    }
    let mut lines = vec![format!("modules: {}", all.len())];
    for (r, n) in &by_result {
        lines.push(format!("  {r}: {n}"));
    }
    lines.push(format!("distinct missing exports (modules that only lack exports): {}", symbols.len()));
    let mut sym: Vec<_> = symbols.iter().collect();
    sym.sort_by(|a, b| b.1 .1.len().cmp(&a.1 .1.len()));
    for (_, (readable, mods)) in sym.iter().take(40) {
        lines.push(format!("  {:>3}  {}", mods.len(), readable));
    }
    // why modules do not compile: a header they need is not there (a dependency: playerbots, a library), or
    // the core's API differs from the one they were written for
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for o in all.iter().filter(|o| o.result == "compile-error") {
        let header = o.compile_errors.iter().find(|e| e.starts_with("C1083")).and_then(|e| {
            let start = e.find(": ")? + 2;
            Some(e[start..].split(':').next()?.trim().to_string())
        });
        let kind = match header {
            Some(h) => format!("missing header {h}"),
            None => format!("core API differs ({})", o.compile_errors.first().map(|e| e.split(' ').next().unwrap_or("")).unwrap_or("")),
        };
        *kinds.entry(kind).or_default() += 1;
    }
    lines.push(String::new());
    lines.push("compile errors by cause:".into());
    let mut k: Vec<_> = kinds.into_iter().collect();
    k.sort_by(|a, b| b.1.cmp(&a.1));
    for (kind, n) in k.iter().take(25) {
        lines.push(format!("  {n:>3}  {kind}"));
    }
    lines.push(String::new());
    for o in &all {
        lines.push(format!("{:<22} ★{:<5} {:<48} {}", o.result, o.stars, o.slug,
            if o.result == "link-missing-exports" { format!("{} exports", o.missing_exports.len()) }
            else { o.detail.clone().or_else(|| o.compile_errors.first().cloned()).unwrap_or_default().chars().take(110).collect() }));
    }
    let text = lines.join("\n");
    println!("{text}");
    std::fs::write(work.join("summary.txt"), &text).unwrap();
    // the exports a core build would add, one decorated name per line
    let def: Vec<&String> = symbols.keys().collect();
    std::fs::write(work.join("missing-exports.txt"), def.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("run") => run(Path::new(&a[1]), Path::new(&a[2]), a[3].parse().unwrap()),
        Some("summary") => summary(Path::new(&a[1])),
        _ => eprintln!("usage: catalog_build_sweep run <realm> <work> <count> | summary <work>"),
    }
}
