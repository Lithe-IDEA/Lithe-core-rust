//! Prices from a models.dev table, and what calls cost at them.

use lithe_protocol::ErrorCode;

use crate::model::Tokens;
use crate::pricing::{call_cost, CallCost, Price, PriceTable, MAX_PRICE_TABLE_BYTES};

const TABLE: &str = r#"{
    "vendor-a": {
        "name": "Vendor A",
        "models": {
            "shared": { "cost": { "input": 1, "output": 2 } },
            "split": { "cost": { "input": 1, "output": 2 } },
            "tiered": {
                "cost": {
                    "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75,
                    "context_over_200k": {
                        "input": 6, "output": 22.5, "cache_read": 0.6, "cache_write": 7.5
                    }
                }
            },
            "thinking": { "cost": { "input": 1, "output": 4, "reasoning": 8 } },
            "text-price": { "cost": { "input": "free", "output": 2 } },
            "negative": { "cost": { "input": -1, "output": 2 } },
            "no-cost": {}
        }
    },
    "vendor-b": {
        "models": {
            "shared": { "cost": { "input": 1, "output": 2 } },
            "split": { "cost": { "input": 5, "output": 2 } }
        }
    },
    "no-models": { "name": "Nothing priced" }
}"#;

fn table() -> PriceTable {
    PriceTable::from_models_dev(TABLE.as_bytes()).expect("read the price table")
}

fn price<'t>(table: &'t PriceTable, model: &str) -> &'t Price {
    table.lookup(None, model).expect("a price")
}

fn tokens(input: u64, output: u64) -> Tokens {
    Tokens {
        input: Some(input),
        output: Some(output),
        ..Tokens::default()
    }
}

fn assert_cost(cost: Option<CallCost>, usd: f64, approximate: bool) {
    let cost = cost.expect("a priced call");
    assert!((cost.usd - usd).abs() < 1e-12, "{} is not {usd}", cost.usd);
    assert_eq!(cost.approximate, approximate);
}

#[test]
fn models_without_a_valid_price_are_left_out() {
    let table = table();
    assert_eq!(table.len(), 6);
    assert!(!table.is_empty());
    for model in ["text-price", "negative", "no-cost"] {
        assert!(table.lookup(Some("vendor-a"), model).is_none(), "{model}");
    }
    assert!(PriceTable::from_models_dev(b"{}")
        .expect("read the price table")
        .is_empty());
}

#[test]
fn a_model_without_a_provider_needs_a_single_price() {
    let table = table();
    assert_eq!(
        table.lookup(None, "shared"),
        table.lookup(Some("vendor-b"), "shared")
    );
    assert!(table.lookup(None, "shared").is_some());
    // The providers charge differently, so the call cannot be priced.
    assert!(table.lookup(None, "split").is_none());
    assert_eq!(
        table
            .lookup(Some("vendor-b"), "split")
            .map(|price| price.base.input),
        Some(5.0)
    );
    assert!(table.lookup(None, "tiered").is_some());
    assert!(table.lookup(Some("vendor-c"), "shared").is_none());
    assert!(table.lookup(None, "unknown").is_none());
}

#[test]
fn cache_tokens_are_priced_at_their_rates() {
    let table = table();
    let tiered = price(&table, "tiered");
    let mut used = Tokens {
        cache_read: Some(10_000),
        cache_write_5m: Some(1_000),
        ..tokens(1_000, 2_000)
    };
    // 1000 × 3 + 2000 × 15 + 10000 × 0.3 + 1000 × 3.75 millionths of a dollar.
    assert_cost(call_cost(tiered, &used), 0.039_75, false);
    // The table's one cache-write price is the five-minute one.
    used.cache_write_1h = Some(1_000);
    assert_cost(call_cost(tiered, &used), 0.043_5, true);
}

#[test]
fn prompts_over_a_tier_use_its_rates() {
    let table = table();
    let tiered = price(&table, "tiered");
    assert_eq!(tiered.tiers.len(), 1);
    // A prompt of exactly the tier size is priced at the base rates.
    assert_cost(call_cost(tiered, &tokens(200_000, 0)), 0.6, false);
    // Cache reads count toward the prompt size: 150001 × 6 + 1000 × 22.5 +
    // 50000 × 0.6.
    let over = Tokens {
        cache_read: Some(50_000),
        ..tokens(150_001, 1_000)
    };
    assert_cost(call_cost(tiered, &over), 0.952_506, false);
}

#[test]
fn cache_tokens_without_a_cache_price_are_priced_as_input() {
    let table = table();
    let shared = price(&table, "shared");
    let used = Tokens {
        cache_read: Some(100),
        cache_write_5m: Some(50),
        ..tokens(10, 10)
    };
    // (10 + 100 + 50) × 1 + 10 × 2.
    assert_cost(call_cost(shared, &used), 0.000_18, true);
    let none_cached = Tokens {
        cache_read: Some(0),
        ..tokens(10, 10)
    };
    assert_cost(call_cost(shared, &none_cached), 0.000_03, false);
}

#[test]
fn reasoning_is_priced_apart_when_the_table_prices_it() {
    let table = table();
    let used = Tokens {
        reasoning: Some(400),
        ..tokens(0, 1_000)
    };
    // 600 × 4 + 400 × 8.
    assert_cost(call_cost(price(&table, "thinking"), &used), 0.005_6, false);
    assert_cost(
        call_cost(price(&table, "thinking"), &tokens(0, 1_000)),
        0.004,
        false,
    );
    assert_cost(call_cost(price(&table, "shared"), &used), 0.002, false);
}

#[test]
fn a_call_without_input_or_output_is_not_priced() {
    let table = table();
    let shared = price(&table, "shared");
    let output_only = Tokens {
        output: Some(10),
        ..Tokens::default()
    };
    let input_only = Tokens {
        input: Some(10),
        ..Tokens::default()
    };
    assert_eq!(call_cost(shared, &output_only), None);
    assert_eq!(call_cost(shared, &input_only), None);
}

#[test]
fn context_tiers_are_read_from_the_tier_list() {
    let table = PriceTable::from_models_dev(
        br#"{ "v": { "models": { "m": { "cost": {
            "input": 1, "output": 2,
            "tiers": [
                { "tier": { "type": "context", "size": 128000 }, "input": 2, "output": 4 },
                { "tier": { "type": "batch" }, "input": 0.5, "output": 1 },
                { "tier": { "type": "context", "size": 32000 }, "input": 1.5, "output": 3 },
                { "tier": { "type": "context" }, "input": 9, "output": 9 }
            ]
        } } } } }"#,
    )
    .expect("read the price table");
    let price = price(&table, "m");
    let sizes: Vec<u64> = price
        .tiers
        .iter()
        .map(|tier| tier.above_prompt_tokens)
        .collect();
    assert_eq!(sizes, [32_000, 128_000]);
    assert_eq!(price.rates_for(32_000).input, 1.0);
    assert_eq!(price.rates_for(32_001).input, 1.5);
    assert_eq!(price.rates_for(128_001).input, 2.0);
}

#[test]
fn unreadable_tables_are_refused() {
    let error = PriceTable::from_models_dev(b"{ not json").expect_err("invalid JSON");
    assert!(matches!(error.code, ErrorCode::ParseFailed));
    let error = PriceTable::from_models_dev(b"[]").expect_err("not a map of providers");
    assert!(matches!(error.code, ErrorCode::ParseFailed));
    let error = PriceTable::from_models_dev(&vec![b' '; MAX_PRICE_TABLE_BYTES + 1])
        .expect_err("a table too large");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
}
