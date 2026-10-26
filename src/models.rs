//! `sync/models.toml`: model capability tags, validated here and rendered to the JSON file
//! agent-terminal and `cross-agent status` read (`model-tags.json`, contract v1). This module is
//! the ONLY validator; the readers are lenient.

use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const TIERS: [&str; 3] = ["fast", "standard", "deep"];
pub const DRIVERS: [&str; 3] = ["claude", "agy", "codex"];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsFile {
    #[serde(default)]
    pub vocabulary: BTreeMap<String, String>,
    #[serde(default, rename = "model")]
    pub models: Vec<ModelEntry>,
    /// Optional `[effort]` table, rendered as `effort_policy`.
    #[serde(default)]
    pub effort: Option<EffortPolicy>,
}

/// Which reasoning effort each tier should use, on a ladder; `reserved` rungs are never picked by
/// readers' clamping.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortPolicy {
    pub ladder: Vec<String>,
    pub fast: String,
    pub standard: String,
    pub deep: String,
    #[serde(default)]
    pub reserved: Vec<String>,
}

impl EffortPolicy {
    fn validate(&self) -> Result<()> {
        let at = |field: &str, why: String| format!("models.toml: [effort] field '{field}': {why}");
        if self.ladder.is_empty() {
            bail!("{}", at("ladder", "must not be empty".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.ladder {
            if !seen.insert(r.as_str()) {
                bail!("{}", at("ladder", format!("duplicate rung '{r}'")));
            }
        }
        for v in &self.reserved {
            if !self.ladder.contains(v) {
                bail!("{}", at("reserved", format!("'{v}' is not in ladder")));
            }
        }
        // A tier's effort is what readers recommend: a reserved rung there would be recommended.
        for (field, v) in [
            ("fast", &self.fast),
            ("standard", &self.standard),
            ("deep", &self.deep),
        ] {
            if !self.ladder.contains(v) {
                bail!("{}", at(field, format!("'{v}' is not in ladder")));
            }
            if self.reserved.contains(v) {
                bail!("{}", at(field, format!("'{v}' is reserved")));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(default)]
    pub driver: Option<String>,
    pub tier: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub best_for: Option<String>,
}

/// `pre*`, `*suf`, `*infix*` or exact: at most one leading and one trailing `*`, and a non-empty core.
fn pattern_ok(m: &str) -> bool {
    let core = m.strip_prefix('*').unwrap_or(m);
    let core = core.strip_suffix('*').unwrap_or(core);
    !core.is_empty() && !core.contains('*')
}

impl ModelsFile {
    /// Errors name the 1-based `[[model]]` entry and the offending field.
    pub fn validate(&self) -> Result<()> {
        if let Some(e) = &self.effort {
            e.validate()?;
        }
        for (i, e) in self.models.iter().enumerate() {
            let n = i + 1;
            let at = |field: &str, why: String| {
                format!("models.toml: [[model]] #{n} field '{field}': {why}")
            };
            if !pattern_ok(&e.pattern) {
                bail!(
                    "{}",
                    at(
                        "match",
                        format!(
                            "'{}' must be non-empty with `*` only as a leading/trailing wildcard",
                            e.pattern
                        )
                    )
                );
            }
            if let Some(d) = &e.driver {
                if !DRIVERS.contains(&d.as_str()) {
                    bail!(
                        "{}",
                        at("driver", format!("'{d}' is not one of {DRIVERS:?}"))
                    );
                }
            }
            if !TIERS.contains(&e.tier.as_str()) {
                bail!(
                    "{}",
                    at("tier", format!("'{}' is not one of {TIERS:?}", e.tier))
                );
            }
            if e.tags.is_empty() {
                bail!("{}", at("tags", "must not be empty".into()));
            }
            let mut seen = std::collections::HashSet::new();
            for t in &e.tags {
                if !self.vocabulary.contains_key(t) {
                    bail!("{}", at("tags", format!("'{t}' is not in [vocabulary]")));
                }
                if !seen.insert(t.as_str()) {
                    bail!("{}", at("tags", format!("duplicate tag '{t}'")));
                }
            }
        }
        Ok(())
    }

    /// Contract v1 JSON, pretty-printed with a trailing newline.
    pub fn render(&self) -> Result<String> {
        let models: Vec<Value> = self
            .models
            .iter()
            .map(|e| {
                let mut m = json!({
                    "match": e.pattern,
                    "driver": e.driver,
                    "tier": e.tier,
                    "tags": e.tags,
                });
                if let Some(b) = &e.best_for {
                    m["best_for"] = json!(b);
                }
                m
            })
            .collect();
        let mut doc = json!({ "version": 1, "vocabulary": self.vocabulary, "models": models });
        if let Some(e) = &self.effort {
            doc["effort_policy"] = json!({
                "ladder": e.ladder,
                "fast": e.fast,
                "standard": e.standard,
                "deep": e.deep,
                "reserved": e.reserved,
            });
        }
        Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &str) -> ModelsFile {
        toml::from_str(&format!(
            "[vocabulary]\ncode = \"c\"\nfast = \"f\"\n{extra}"
        ))
        .unwrap()
    }

    const OK: &str =
        "[[model]]\nmatch = \"*opus*\"\ntier = \"deep\"\ntags = [\"code\"]\nbest_for = \"x\"\n";

    #[test]
    fn valid_file_renders_contract_v1() {
        let f = parse(OK);
        f.validate().unwrap();
        let v: Value = serde_json::from_str(&f.render().unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["vocabulary"]["code"], "c");
        assert_eq!(v["models"][0]["match"], "*opus*");
        assert!(v["models"][0]["driver"].is_null());
        assert_eq!(v["models"][0]["best_for"], "x");
    }

    #[test]
    fn errors_name_entry_and_field() {
        let cases = [
            (
                "match = \"\"\ntier = \"fast\"\ntags = [\"code\"]",
                "#1 field 'match'",
            ),
            (
                "match = \"a*b\"\ntier = \"fast\"\ntags = [\"code\"]",
                "field 'match'",
            ),
            (
                "match = \"**\"\ntier = \"fast\"\ntags = [\"code\"]",
                "field 'match'",
            ),
            (
                "match = \"a\"\ntier = \"huge\"\ntags = [\"code\"]",
                "field 'tier'",
            ),
            (
                "match = \"a\"\ndriver = \"gemini\"\ntier = \"fast\"\ntags = [\"code\"]",
                "field 'driver'",
            ),
            ("match = \"a\"\ntier = \"fast\"\ntags = []", "field 'tags'"),
            (
                "match = \"a\"\ntier = \"fast\"\ntags = [\"nope\"]",
                "'nope' is not in [vocabulary]",
            ),
            (
                "match = \"a\"\ntier = \"fast\"\ntags = [\"code\", \"code\"]",
                "duplicate tag",
            ),
        ];
        for (body, want) in cases {
            let err = parse(&format!("[[model]]\n{body}\n"))
                .validate()
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{body}: {err}");
        }
        let second = parse(&format!(
            "{OK}[[model]]\nmatch = \"a\"\ntier = \"zzz\"\ntags = [\"code\"]\n"
        ));
        assert!(second
            .validate()
            .unwrap_err()
            .to_string()
            .contains("#2 field 'tier'"));
    }

    const EFFORT: &str = "[effort]\nladder = [\"low\", \"medium\", \"high\", \"max\"]\nfast = \"low\"\nstandard = \"medium\"\ndeep = \"high\"\nreserved = [\"max\"]\n";

    #[test]
    fn effort_policy_renders_and_is_omitted_when_absent() {
        let f = parse(&format!("{OK}{EFFORT}"));
        f.validate().unwrap();
        let v: Value = serde_json::from_str(&f.render().unwrap()).unwrap();
        assert_eq!(v["effort_policy"]["ladder"][3], "max");
        assert_eq!(v["effort_policy"]["deep"], "high");
        assert_eq!(v["effort_policy"]["reserved"][0], "max");
        let none: Value = serde_json::from_str(&parse(OK).render().unwrap()).unwrap();
        assert!(none.get("effort_policy").is_none());
    }

    #[test]
    fn effort_policy_errors_name_the_field() {
        let l = "ladder = [\"low\", \"high\"]\n";
        let cases = [
            ("ladder = []\nfast = \"low\"\nstandard = \"low\"\ndeep = \"low\"\n", "field 'ladder'"),
            ("ladder = [\"low\", \"low\"]\nfast = \"low\"\nstandard = \"low\"\ndeep = \"low\"\n", "duplicate rung"),
            (&format!("{l}fast = \"nope\"\nstandard = \"low\"\ndeep = \"low\"\n"), "field 'fast'"),
            (&format!("{l}fast = \"low\"\nstandard = \"nope\"\ndeep = \"low\"\n"), "field 'standard'"),
            (&format!("{l}fast = \"low\"\nstandard = \"low\"\ndeep = \"nope\"\n"), "field 'deep'"),
            (&format!("{l}fast = \"low\"\nstandard = \"low\"\ndeep = \"high\"\nreserved = [\"max\"]\n"), "field 'reserved'"),
            (&format!("{l}fast = \"low\"\nstandard = \"low\"\ndeep = \"high\"\nreserved = [\"high\"]\n"), "field 'deep': 'high' is reserved"),
        ];
        for (body, want) in cases {
            let err = parse(&format!("[effort]\n{body}"))
                .validate()
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{body}: {err}");
        }
    }

    #[test]
    fn wildcard_forms_accepted() {
        for m in ["exact", "pre*", "*suf", "*in*"] {
            assert!(pattern_ok(m), "{m}");
        }
    }
}
