//! Built-in model table (like Codex's bundled `models.json`): model name → context window and max output, generated
//! from public data by `scripts/gen_model_table.py` into `data/model_table.json` and compiled in.
//! Lookup follows Codex: exact name, then the longest table key that is a prefix of the name (on a `-` / `:` / `@`
//! boundary, so `qwen3.8-27b-local` → `qwen3.8-27b`), each also tried on the part after a namespace (`tencent/Hy4-preview`).
use std::collections::HashMap;
use std::sync::OnceLock;

static TABLE: OnceLock<HashMap<String, (usize, Option<usize>)>> = OnceLock::new();

fn table() -> &'static HashMap<String, (usize, Option<usize>)> {
    TABLE.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(include_str!("../data/model_table.json")).unwrap_or_default();
        v["models"].as_object().map(|o| o.iter().filter_map(|(k, a)| {
            let a = a.as_array()?;
            Some((k.to_lowercase(), (a.first()?.as_u64()? as usize, a.get(1).and_then(|x| x.as_u64()).map(|x| x as usize))))
        }).collect()).unwrap_or_default()
    })
}

/// (context window, max output if known) for a model name, case-insensitive.
pub fn lookup(model: &str) -> Option<(usize, Option<usize>)> {
    let m = model.trim().to_lowercase();
    let short = m.rsplit('/').next().unwrap_or(&m).to_string();
    let t = table();
    let names = [m.as_str(), short.as_str()];
    for n in names { if let Some(v) = t.get(n) { return Some(*v); } }
    for n in names {
        let best = t.iter()
            .filter(|(k, _)| k.len() >= 4 && n.len() > k.len() && n.starts_with(k.as_str()) && matches!(n.as_bytes()[k.len()], b'-' | b'_' | b':' | b'@' | b' '))
            .max_by_key(|(k, _)| k.len());
        if let Some((_, v)) = best { return Some(*v); }
    }
    None
}

pub fn len() -> usize { table().len() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookups() {
        assert!(len() > 1000);
        assert_eq!(lookup("glm-5.3-flash").map(|x| x.0), Some(1_048_576));
        assert_eq!(lookup("GLM-5.3-Flash").map(|x| x.0), Some(1_048_576));
        assert_eq!(lookup("qwen3.8-27b-local").map(|x| x.0), Some(262_144));   // a gateway's local deployment, confirmed by the user
        assert!(lookup("tencent/Hy4-preview").is_some());
        assert_eq!(lookup("gpt-5.5").map(|x| x.0), Some(272_000));   // Codex's own value
        assert_eq!(lookup("no-such-model-xyz"), None);
        assert_eq!(lookup("glm-5.3x"), None);   // no prefix match without a boundary
    }

    #[test]
    fn saved_caps_migration() {
        use crate::types::{Capabilities, ModelRef};
        let mk = |w: usize, src: &str| ModelRef { provider_id: "p".into(), model: "glm-5.3-flash".into(),
            capabilities: Some(Capabilities { context_window: w, context_source: src.into(), ..Capabilities::default() }) };
        // old config holding the old preset value → table; a value the user typed → kept; explicit source → kept
        assert_eq!(mk(32_768, "").caps().context_source, "table");
        assert_eq!(mk(32_768, "").caps().context_window, 1_048_576);
        assert_eq!(mk(65_536, "").caps().context_window, 65_536);
        assert_eq!(mk(65_536, "").caps().context_source, "manual");
        assert_eq!(mk(32_768, "learned").caps().context_window, 32_768);
        let none = ModelRef { provider_id: "p".into(), model: "my-own-finetune".into(), capabilities: None };
        assert_eq!(none.caps().context_source, "default");
    }
}
