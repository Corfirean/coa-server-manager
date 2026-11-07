//! Analyze a module by its GitHub link, as "Add from a link" does. usage: analyze_url <url> <cache dir>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = coa_core::analyzer::analyze_github_repository(&a[0], Some(std::path::Path::new(&a[1]))).expect("analysis");
    println!("{} @ {} cache_hit={:?}", r.repository_name, r.commit, r.cache_hit);
    println!("classification {:?}; hooks {}; sql {}; cmake {}", r.classification, r.hooks.len(), r.sql_files.len(), r.has_cmakelists);
}
