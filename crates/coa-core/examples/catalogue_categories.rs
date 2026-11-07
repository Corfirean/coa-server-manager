//! How the module categories fall on a real catalogue: counts per category and a few modules of each.
//! usage: catalogue_categories <catalogue.json downloaded from azerothcore.org>
fn main() {
    let path = std::env::args().nth(1).expect("usage: catalogue_categories <catalogue.json>");
    let raw: coa_core::catalogue::OfficialCatalogueRaw =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let overlay = coa_core::catalogue::load_coa_overlay(None);
    let items = coa_core::catalogue::normalize_catalogue(&raw, &overlay);
    let mut by: std::collections::BTreeMap<String, Vec<(u32, String, String)>> = Default::default();
    for i in &items {
        let c = coa_core::catalogue::categorize(&i.full_name, &i.name, &i.description, &i.topics);
        by.entry(c).or_default().push((i.stars, i.full_name.clone(), i.description.chars().take(70).collect()));
    }
    println!("modules: {}", items.len());
    for c in coa_core::catalogue::categories() {
        let mut list = by.remove(&c.code).unwrap_or_default();
        list.sort_by(|a, b| b.0.cmp(&a.0));
        println!("\n== {} ({}) {}", c.code, c.ru, list.len());
        for (s, n, d) in list.iter().take(6) {
            println!("   {s:>5} {n:<45} {d}");
        }
    }
}
