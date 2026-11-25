//! Model prices from a models.dev table, and what a call would cost at them.
//!
//! Agents that run on a subscription do not bill per call, so the cost here
//! is what the same tokens would cost at the provider's API prices. The table
//! is models.dev's `api.json`: providers by ID, each with models by ID, each
//! with a `cost` in US dollars per million tokens.

use std::collections::HashMap;

use lithe_protocol::{CoreError, ErrorCode};
use serde_json::Value;

use crate::model::Tokens;

/// Largest price table accepted, in bytes.
pub const MAX_PRICE_TABLE_BYTES: usize = 32 * 1024 * 1024;

/// The prompt size above which models.dev's `context_over_200k` prices apply.
const CONTEXT_OVER_200K_TOKENS: u64 = 200_000;

/// Prices of one model in one context range, in US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Uncached input.
    pub input: f64,
    /// Output, reasoning included unless `reasoning` is set.
    pub output: f64,
    /// Reasoning output, when priced apart from other output.
    pub reasoning: Option<f64>,
    /// Input read from the prompt cache.
    pub cache_read: Option<f64>,
    /// Input written to the prompt cache.
    pub cache_write: Option<f64>,
}

/// Prices that apply when a prompt is larger than `above_prompt_tokens`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tier {
    /// Prompt tokens (input, cache reads and cache writes) the prompt must exceed.
    pub above_prompt_tokens: u64,
    /// The prices.
    pub rates: Rates,
}

/// Prices of one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    /// Prices of prompts no larger than the first tier.
    pub base: Rates,
    /// Prices of larger prompts, by ascending `above_prompt_tokens`.
    pub tiers: Vec<Tier>,
}

impl Price {
    /// The prices of a prompt of `prompt_tokens`: those of the largest tier it
    /// exceeds, or the base prices.
    pub fn rates_for(&self, prompt_tokens: u64) -> &Rates {
        self.tiers
            .iter()
            .rev()
            .find(|tier| prompt_tokens > tier.above_prompt_tokens)
            .map_or(&self.base, |tier| &tier.rates)
    }
}

/// What a call would cost at API prices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallCost {
    /// The cost in US dollars.
    pub usd: f64,
    /// Whether a count was priced at a substitute: cache tokens without a
    /// cache price are priced as input, and one-hour cache writes at the
    /// table's single cache-write price.
    pub approximate: bool,
}

/// Prices by provider and model.
#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    prices: HashMap<String, HashMap<String, Price>>,
    /// For a model without a known provider: the provider whose price to use,
    /// or `None` when providers price it differently.
    by_model: HashMap<String, Option<String>>,
}

impl PriceTable {
    /// Reads models.dev's `api.json`. Models without a valid input and output
    /// price are left out, and so are tiers of a kind other than context size.
    pub fn from_models_dev(json: &[u8]) -> Result<Self, CoreError> {
        if json.len() > MAX_PRICE_TABLE_BYTES {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The model price table is too large",
            ));
        }
        let root: Value = serde_json::from_slice(json).map_err(|_| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "The model price table is not valid JSON",
            )
        })?;
        let Some(providers) = root.as_object() else {
            return Err(CoreError::new(
                ErrorCode::ParseFailed,
                "The model price table is not a map of providers",
            ));
        };
        let mut table = PriceTable::default();
        for (provider_id, provider) in providers {
            let Some(models) = provider.get("models").and_then(Value::as_object) else {
                continue;
            };
            let mut prices = HashMap::new();
            for (model_id, model) in models {
                if let Some(price) = model.get("cost").and_then(price) {
                    prices.insert(model_id.clone(), price);
                }
            }
            if !prices.is_empty() {
                table.prices.insert(provider_id.clone(), prices);
            }
        }
        table.index_models();
        Ok(table)
    }

    /// The price of `model` from `provider`. Without a provider, the model's
    /// price when every provider that lists it charges the same; otherwise
    /// the call cannot be priced.
    pub fn lookup(&self, provider: Option<&str>, model: &str) -> Option<&Price> {
        let provider = match provider {
            Some(provider) => provider,
            None => self.by_model.get(model)?.as_deref()?,
        };
        self.prices.get(provider)?.get(model)
    }

    /// Models with a price, counted once per provider.
    pub fn len(&self) -> usize {
        self.prices.values().map(HashMap::len).sum()
    }

    /// Whether the table has no prices.
    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    fn index_models(&mut self) {
        let mut providers: Vec<&String> = self.prices.keys().collect();
        providers.sort();
        let mut by_model: HashMap<String, Option<String>> = HashMap::new();
        for provider in providers {
            for (model, price) in &self.prices[provider] {
                match by_model.get(model) {
                    None => {
                        by_model.insert(model.clone(), Some(provider.clone()));
                    }
                    Some(Some(first)) if &self.prices[first][model] != price => {
                        by_model.insert(model.clone(), None);
                    }
                    Some(_) => {}
                }
            }
        }
        self.by_model = by_model;
    }
}

/// What a call would cost, or `None` without its input or output count.
pub fn call_cost(price: &Price, tokens: &Tokens) -> Option<CallCost> {
    let input = tokens.input?;
    let output = tokens.output?;
    let cache_read = tokens.cache_read.unwrap_or(0);
    let write_5m = tokens.cache_write_5m.unwrap_or(0);
    let write_1h = tokens.cache_write_1h.unwrap_or(0);
    let prompt = input
        .saturating_add(cache_read)
        .saturating_add(write_5m)
        .saturating_add(write_1h);
    let rates = price.rates_for(prompt);
    let mut approximate = false;
    let mut substitute = |count: u64, rate: Option<f64>, exact: bool| -> f64 {
        if count == 0 {
            return 0.0;
        }
        match rate {
            Some(rate) => {
                approximate |= !exact;
                count as f64 * rate
            }
            None => {
                approximate = true;
                count as f64 * rates.input
            }
        }
    };
    let mut micro_usd = input as f64 * rates.input;
    micro_usd += match (tokens.reasoning, rates.reasoning) {
        (Some(reasoning), Some(rate)) => {
            output.saturating_sub(reasoning) as f64 * rates.output + reasoning as f64 * rate
        }
        _ => output as f64 * rates.output,
    };
    micro_usd += substitute(cache_read, rates.cache_read, true);
    micro_usd += substitute(write_5m, rates.cache_write, true);
    micro_usd += substitute(write_1h, rates.cache_write, false);
    Some(CallCost {
        usd: micro_usd / 1_000_000.0,
        approximate,
    })
}

fn price(cost: &Value) -> Option<Price> {
    let base = rates(cost)?;
    let mut tiers = Vec::new();
    if let Some(rates) = cost.get("context_over_200k").and_then(rates) {
        tiers.push(Tier {
            above_prompt_tokens: CONTEXT_OVER_200K_TOKENS,
            rates,
        });
    }
    for tier in cost
        .get("tiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(kind) = tier.get("tier") else {
            continue;
        };
        if kind.get("type").and_then(Value::as_str) != Some("context") {
            continue;
        }
        let (Some(size), Some(rates)) = (kind.get("size").and_then(Value::as_u64), rates(tier))
        else {
            continue;
        };
        tiers.push(Tier {
            above_prompt_tokens: size,
            rates,
        });
    }
    tiers.sort_by_key(|tier| tier.above_prompt_tokens);
    tiers.dedup_by_key(|tier| tier.above_prompt_tokens);
    Some(Price { base, tiers })
}

fn rates(cost: &Value) -> Option<Rates> {
    Some(Rates {
        input: rate(cost, "input")?,
        output: rate(cost, "output")?,
        reasoning: rate(cost, "reasoning"),
        cache_read: rate(cost, "cache_read"),
        cache_write: rate(cost, "cache_write"),
    })
}

fn rate(cost: &Value, field: &str) -> Option<f64> {
    cost.get(field)
        .and_then(Value::as_f64)
        .filter(|rate| rate.is_finite() && *rate >= 0.0)
}
