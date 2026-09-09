//! Cost weighting. Raw token counts rank conversations by context length, not
//! by what they cost — 95% of raw volume is cache reads at a tenth (or less) of
//! the input rate. Every number the UI shows is weighted by these rates first.
//!
//! Rates are $ per 1M tokens at published API list price. They are a proxy for
//! however Anthropic meters subscription usage; calibration (see `windows.rs`)
//! makes the *relative* numbers exact even when the proxy is off, as long as
//! the proxy is off consistently across models.

use serde::{Deserialize, Serialize};

use super::record::Usage;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

impl Rates {
    /// The standard cache multipliers: 1.25× input for 5-minute writes,
    /// 2× for 1-hour writes, 0.1× for reads.
    pub fn standard(input: f64, output: f64) -> Self {
        Rates {
            input,
            output,
            cache_write_5m: input * 1.25,
            cache_write_1h: input * 2.0,
            cache_read: input * 0.1,
        }
    }

    pub fn cost(&self, u: &Usage) -> f64 {
        (u.input as f64 * self.input
            + u.output as f64 * self.output
            + u.cache_5m as f64 * self.cache_write_5m
            + u.cache_1h as f64 * self.cache_write_1h
            + u.cache_read as f64 * self.cache_read)
            / 1e6
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceEntry {
    /// Exact model id, or a prefix (`claude-opus-4`) that matches a family.
    pub model: String,
    pub rates: Rates,
    /// Counts toward the Fable-specific weekly limit.
    #[serde(default)]
    pub fable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceTable {
    pub entries: Vec<PriceEntry>,
}

impl Default for PriceTable {
    fn default() -> Self {
        let e = |model: &str, rates: Rates, fable: bool| PriceEntry {
            model: model.to_string(),
            rates,
            fable,
        };
        PriceTable {
            entries: vec![
                // Fable 5.1's cache read is a documented exception: flat $0.25.
                e(
                    "claude-fable-5-1",
                    Rates { cache_read: 0.25, ..Rates::standard(10.0, 50.0) },
                    true,
                ),
                e("claude-fable-5", Rates::standard(10.0, 50.0), true),
                e("claude-opus-5", Rates::standard(5.0, 25.0), false),
                e("claude-opus-4-8", Rates::standard(5.0, 25.0), false),
                e("claude-opus-4-7", Rates::standard(5.0, 25.0), false),
                e("claude-opus-4-6", Rates::standard(5.0, 25.0), false),
                e("claude-opus-4", Rates::standard(5.0, 25.0), false),
                e("claude-sonnet-5", Rates::standard(2.0, 10.0), false),
                e("claude-sonnet-4-6", Rates::standard(3.0, 15.0), false),
                e("claude-sonnet-4", Rates::standard(3.0, 15.0), false),
                e("claude-haiku-4-5", Rates::standard(1.0, 5.0), false),
                e("claude-haiku-4", Rates::standard(1.0, 5.0), false),
            ],
        }
    }
}

impl PriceTable {
    /// Exact match first, then the longest prefix that matches. Unknown models
    /// (and `<synthetic>` placeholders) get nothing and are excluded upstream.
    pub fn lookup(&self, model: &str) -> Option<&PriceEntry> {
        if let Some(e) = self.entries.iter().find(|e| e.model == model) {
            return Some(e);
        }
        self.entries
            .iter()
            .filter(|e| model.starts_with(&e.model))
            .max_by_key(|e| e.model.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_beats_prefix() {
        let t = PriceTable::default();
        assert_eq!(t.lookup("claude-fable-5-1").unwrap().rates.cache_read, 0.25);
        assert_eq!(t.lookup("claude-fable-5").unwrap().rates.cache_read, 1.0);
    }

    #[test]
    fn longest_prefix_wins() {
        let t = PriceTable::default();
        // A dated snapshot id we've never seen should still land on its family.
        assert_eq!(t.lookup("claude-opus-4-8-20260101").unwrap().model, "claude-opus-4-8");
        assert_eq!(t.lookup("claude-haiku-4-5-20251001").unwrap().model, "claude-haiku-4-5");
        assert!(t.lookup("<synthetic>").is_none());
    }

    #[test]
    fn cost_matches_hand_calculation() {
        let r = Rates::standard(5.0, 25.0);
        let u = Usage { input: 1_000_000, output: 1_000_000, cache_5m: 0, cache_1h: 1_000_000, cache_read: 1_000_000 };
        assert!((r.cost(&u) - (5.0 + 25.0 + 10.0 + 0.5)).abs() < 1e-9);
    }
}
