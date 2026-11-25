//! Summaries, speeds and dates of calls.

use crate::model::{ByteRange, CallRecord, Tokens};
use crate::pricing::PriceTable;
use crate::stats::{
    call_speed, civil_date, rank_speed, summarize, CostBasis, CostTotal, GroupBy, GroupKey, Speed,
    Summarizer, TokenTotals,
};
use crate::store::CallEntry;

const PRICES: &str = r#"{ "vendor": { "models": {
    "m-a": { "cost": { "input": 1, "output": 2, "cache_read": 0.1, "cache_write": 1.25 } }
} } }"#;

fn record(key: &str, at: i64) -> CallRecord {
    CallRecord {
        key: key.to_string(),
        session_id: "s1".to_string(),
        thread_id: None,
        started_at_ms: at,
        requested_model: None,
        model: None,
        context_window: None,
        effort: None,
        provider: None,
        tokens: Tokens::default(),
        first_token_ms: None,
        duration_ms: None,
        compaction: false,
        request_id: None,
        error_category: None,
        range: ByteRange { start: 0, end: 1 },
    }
}

fn entry(call: CallRecord) -> CallEntry {
    CallEntry {
        agent: "test-agent".to_string(),
        path: "/sessions/a.jsonl".to_string(),
        cwd: Some("/w".to_string()),
        call,
    }
}

fn timed(model: &str, output: u64, first_token_ms: Option<u64>, duration_ms: u64) -> CallRecord {
    CallRecord {
        model: Some(model.to_string()),
        tokens: Tokens {
            output: Some(output),
            ..Tokens::default()
        },
        first_token_ms,
        duration_ms: Some(duration_ms),
        ..record("c", 0)
    }
}

fn model_of(key: &GroupKey) -> Option<&str> {
    match key {
        GroupKey::Model { model } => model.as_deref(),
        other => panic!("not a model group: {other:?}"),
    }
}

#[test]
fn calls_are_summed_by_model() {
    let prices = PriceTable::from_models_dev(PRICES.as_bytes()).expect("read the prices");
    let cached = CallRecord {
        model: Some("m-a".to_string()),
        tokens: Tokens {
            input: Some(1_000),
            output: Some(100),
            cache_read: Some(10_000),
            cache_write_5m: Some(1_000),
            ..Tokens::default()
        },
        compaction: true,
        ..record("c1", 3_000)
    };
    // Counted under the model asked for, as the answer named none; it reports
    // no cache counts, so it is left out of the hit rate.
    let requested = CallRecord {
        requested_model: Some("m-a".to_string()),
        tokens: Tokens {
            input: Some(500),
            output: Some(50),
            ..Tokens::default()
        },
        ..record("c2", 1_000)
    };
    let failed = CallRecord {
        model: Some("m-b".to_string()),
        tokens: Tokens {
            input: Some(10),
            output: Some(10),
            ..Tokens::default()
        },
        error_category: Some("rate_limited".to_string()),
        ..record("c3", 2_000)
    };
    let unnamed = record("c4", 4_000);
    let entries: Vec<CallEntry> = [cached, requested, failed, unnamed]
        .into_iter()
        .map(entry)
        .collect();

    let summaries = summarize(&entries, GroupBy::Model, Some(&prices));
    let models: Vec<Option<&str>> = summaries
        .iter()
        .map(|summary| model_of(&summary.key))
        .collect();
    assert_eq!(models, [None, Some("m-a"), Some("m-b")]);

    let unnamed = &summaries[0];
    assert_eq!(unnamed.calls, 1);
    assert_eq!(unnamed.cache_hit_rate, None);
    assert_eq!(unnamed.cost.map(|cost| cost.unpriced_calls), Some(1));

    let priced = &summaries[1];
    assert_eq!(
        (priced.calls, priced.failed_calls, priced.compaction_calls),
        (2, 0, 1)
    );
    assert_eq!(
        priced.tokens,
        TokenTotals {
            input: 1_500,
            output: 150,
            cache_read: 10_000,
            cache_write_5m: 1_000,
            cache_write_1h: 0,
            reasoning: 0,
        }
    );
    assert_eq!(priced.cache_hit_rate, Some(10_000.0 / 12_000.0));
    let cost = priced.cost.expect("a cost");
    // (1000 + 100 × 2 + 10000 × 0.1 + 1000 × 1.25) + (500 + 50 × 2) millionths.
    assert!((cost.usd - 0.004_05).abs() < 1e-12, "{}", cost.usd);
    assert_eq!(
        CostTotal { usd: 0.0, ..cost },
        CostTotal {
            usd: 0.0,
            basis: CostBasis::ApiPriceEquivalent,
            priced_calls: 2,
            unpriced_calls: 0,
            approximate_calls: 0,
        }
    );
    assert_eq!(
        (priced.first_started_at_ms, priced.last_started_at_ms),
        (1_000, 3_000)
    );

    let unpriced = &summaries[2];
    assert_eq!((unpriced.calls, unpriced.failed_calls), (1, 1));
    let cost = unpriced.cost.expect("a cost");
    assert_eq!(
        (cost.usd, cost.priced_calls, cost.unpriced_calls),
        (0.0, 0, 1)
    );

    // Without prices there is no cost at all, rather than a cost of zero.
    let by_agent = summarize(&entries, GroupBy::Agent, None);
    assert_eq!(by_agent.len(), 1);
    assert_eq!(
        by_agent[0].key,
        GroupKey::Agent {
            agent: "test-agent".to_string()
        }
    );
    assert_eq!(by_agent[0].calls, 4);
    assert_eq!(by_agent[0].cost, None);
    assert_eq!(by_agent[0].cache_hit_rate, Some(10_000.0 / 12_000.0));
}

#[test]
fn calls_group_by_session_and_directory() {
    let mut elsewhere = entry(CallRecord {
        session_id: "s2".to_string(),
        ..record("c2", 0)
    });
    elsewhere.cwd = None;
    let entries = vec![entry(record("c1", 0)), elsewhere, entry(record("c3", 0))];

    let sessions: Vec<GroupKey> = summarize(&entries, GroupBy::Session, None)
        .into_iter()
        .map(|summary| summary.key)
        .collect();
    let session = |id: &str| GroupKey::Session {
        agent: "test-agent".to_string(),
        session_id: id.to_string(),
    };
    assert_eq!(sessions, [session("s1"), session("s2")]);

    let directories: Vec<(GroupKey, u64)> = summarize(&entries, GroupBy::Cwd, None)
        .into_iter()
        .map(|summary| (summary.key, summary.calls))
        .collect();
    assert_eq!(
        directories,
        [
            (GroupKey::Cwd { cwd: None }, 1),
            (
                GroupKey::Cwd {
                    cwd: Some("/w".to_string())
                },
                2
            ),
        ]
    );
}

#[test]
fn days_follow_the_user_time_zone() {
    // 2026-11-24T20:00:00Z and 2026-11-25T02:00:00Z.
    let entries = [
        entry(record("c1", 1_795_550_400_000)),
        entry(record("c2", 1_795_572_000_000)),
    ];
    let days = |utc_offset_seconds: i32| {
        let mut summarizer = Summarizer::new(GroupBy::Day { utc_offset_seconds }, None);
        for entry in &entries {
            summarizer.add(entry);
        }
        summarizer
            .finish()
            .into_iter()
            .map(|summary| match summary.key {
                GroupKey::Day { date } => (date, summary.calls),
                other => panic!("not a day: {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    let day = |date: &str, calls: u64| (date.to_string(), calls);
    assert_eq!(days(0), [day("2026-11-24", 1), day("2026-11-25", 1)]);
    assert_eq!(days(8 * 3_600), [day("2026-11-25", 2)]);
    assert_eq!(days(-5 * 3_600), [day("2026-11-24", 2)]);
}

#[test]
fn civil_dates_cover_leap_days_and_times_before_1970() {
    assert_eq!(civil_date(0, 0), "1970-01-01");
    assert_eq!(civil_date(-1, 0), "1969-12-31");
    assert_eq!(civil_date(951_782_400_000, 0), "2000-02-29");
    assert_eq!(civil_date(946_684_799_000, 0), "1999-12-31");
    assert_eq!(civil_date(946_684_799_000, 1), "2000-01-01");
    // 2100 is not a leap year.
    assert_eq!(civil_date(4_107_542_400_000, 0), "2100-03-01");
    assert_eq!(civil_date(4_107_542_400_000, -1), "2100-02-28");
}

#[test]
fn speed_is_measured_from_the_first_token() {
    assert_eq!(
        call_speed(&timed("m", 1_000, Some(500), 2_500)),
        Some(Speed::Measured(500.0))
    );
    // Without a first-token time, a long enough call gives a lower bound.
    assert_eq!(
        call_speed(&timed("m", 1_000, None, 2_000)),
        Some(Speed::AtLeast(500.0))
    );
    // So does a first token at the very end.
    assert_eq!(
        call_speed(&timed("m", 1_000, Some(2_000), 2_000)),
        Some(Speed::AtLeast(500.0))
    );
    assert_eq!(call_speed(&timed("m", 999, None, 2_000)), None);
    assert_eq!(call_speed(&timed("m", 1_000, None, 999)), None);
}

#[test]
fn unreliable_speeds_are_left_out() {
    let failed = CallRecord {
        error_category: Some("overloaded".to_string()),
        ..timed("m", 1_000, Some(500), 2_500)
    };
    assert_eq!(call_speed(&failed), None);
    assert_eq!(call_speed(&timed("m", 0, Some(0), 1_000)), None);
    assert_eq!(
        call_speed(&timed("m", 10_000, Some(0), 1_000)),
        Some(Speed::Measured(10_000.0))
    );
    assert_eq!(call_speed(&timed("m", 10_001, Some(0), 1_000)), None);
    let untimed = CallRecord {
        duration_ms: None,
        ..timed("m", 1_000, Some(500), 2_500)
    };
    assert_eq!(call_speed(&untimed), None);
}

#[test]
fn models_rank_by_median_speed() {
    let calls = [
        timed("fast", 200, Some(0), 2_000),
        timed("fast", 200, Some(0), 1_000),
        timed("fast", 300, Some(0), 1_000),
        timed("slow", 100, Some(0), 1_000),
        timed("slow", 1_000, None, 10_000),
        // Fast, but too short to rank.
        timed("slow", 50, Some(0), 100),
        CallRecord {
            error_category: Some("overloaded".to_string()),
            ..timed("broken", 1_000, Some(0), 1_000)
        },
    ];
    let ranks: Vec<(String, u64, f64, bool)> = rank_speed(&calls)
        .into_iter()
        .map(|rank| {
            (
                rank.model,
                rank.calls,
                rank.median_tokens_per_second,
                rank.lower_bound,
            )
        })
        .collect();
    assert_eq!(
        ranks,
        [
            ("fast".to_string(), 3, 200.0, false),
            ("slow".to_string(), 2, 100.0, true),
        ]
    );

    let tied = [
        timed("b", 100, Some(0), 1_000),
        timed("a", 100, Some(0), 1_000),
    ];
    let models: Vec<String> = rank_speed(&tied)
        .into_iter()
        .map(|rank| rank.model)
        .collect();
    assert_eq!(models, ["a", "b"]);
}
