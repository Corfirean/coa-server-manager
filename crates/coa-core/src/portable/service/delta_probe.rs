//! Field-by-field difference of two characters, for the messages of test failures.

pub(crate) fn diff(
    a: &crate::portable::model::PortableCharacter,
    b: &crate::portable::model::PortableCharacter,
) -> Vec<String> {
    let mut out = Vec::new();
    walk(
        "",
        &serde_json::to_value(a).unwrap(),
        &serde_json::to_value(b).unwrap(),
        &mut out,
    );
    out
}

fn walk(path: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value::{Array, Null, Object};
    match (a, b) {
        (Object(x), Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                walk(
                    &format!("{path}/{k}"),
                    x.get(k).unwrap_or(&Null),
                    y.get(k).unwrap_or(&Null),
                    out,
                );
            }
        }
        (Array(x), Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                walk(&format!("{path}[{i}]"), p, q, out);
            }
        }
        _ if a != b => out.push(format!(
            "{path}: {} -> {}",
            a.to_string().chars().take(160).collect::<String>(),
            b.to_string().chars().take(160).collect::<String>()
        )),
        _ => {}
    }
}
