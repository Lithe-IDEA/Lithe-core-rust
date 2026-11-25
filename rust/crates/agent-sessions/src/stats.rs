//! Statistics of calls: token totals, cache hit rate, cost and output speed.
//!
//! Every client shows these numbers from here, so they have one definition:
//!
//! - token totals add the counts calls report, leaving absent counts out;
//! - the cache hit rate is cache reads over all input (uncached input, cache
//!   reads and cache writes), over the calls that report cache counts;
//! - cost is what the tokens would cost at API prices ([`CostBasis`]); calls
//!   without a price are counted apart, never as free;
//! - speed is output tokens per second after the first token; without the
//!   time of the first token, a long call gives a lower bound.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::CallRecord;
use crate::pricing::{call_cost, PriceTable};
use crate::store::CallEntry;

/// Fastest plausible output; faster measurements come from bad timestamps.
pub const MAX_TOKENS_PER_SECOND: f64 = 10_000.0;

/// Fewest output tokens of a call that a speed ranking uses: shorter answers
/// are dominated by latency.
pub const MIN_RANKED_OUTPUT_TOKENS: u64 = 100;

/// Fewest output tokens and milliseconds a call without a first-token time
/// needs for its overall rate to say something about speed.
const MIN_LOWER_BOUND_OUTPUT_TOKENS: u64 = 1000;
const MIN_LOWER_BOUND_DURATION_MS: u64 = 1000;

const MILLISECONDS_PER_DAY: i64 = 86_400_000;

/// How [`summarize`] groups calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    /// By calendar day of the start time, at a UTC offset in seconds (east is
    /// positive).
    Day {
        /// The offset of the user's time zone.
        utc_offset_seconds: i32,
    },
    /// By agent.
    Agent,
    /// By the model that answered, or the requested one when the answer did
    /// not name it.
    Model,
    /// By session.
    Session,
    /// By the working directory of the session.
    Cwd,
}

/// The group a [`Summary`] covers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GroupKey {
    /// A calendar day, as `YYYY-MM-DD`.
    Day {
        /// The date.
        date: String,
    },
    /// An agent.
    Agent {
        /// The agent.
        agent: String,
    },
    /// A model; absent for calls that name none.
    Model {
        /// The model.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A session of an agent.
    #[serde(rename_all = "camelCase")]
    Session {
        /// The agent.
        agent: String,
        /// The session.
        session_id: String,
    },
    /// A working directory; absent for sessions that do not record it.
    Cwd {
        /// The directory.
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
}

/// Sums of the token counts calls report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    /// Uncached input tokens.
    pub input: u64,
    /// Output tokens, reasoning included.
    pub output: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to five-minute cache entries.
    pub cache_write_5m: u64,
    /// Input tokens written to one-hour cache entries.
    pub cache_write_1h: u64,
    /// Reasoning tokens, where agents record them.
    pub reasoning: u64,
}

/// What a cost stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CostBasis {
    /// The tokens priced at the provider's API prices, whatever the user
    /// actually pays; a subscription is not billed per call.
    ApiPriceEquivalent,
}

/// The cost of a group of calls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTotal {
    /// US dollars, over the priced calls.
    pub usd: f64,
    /// What `usd` stands for.
    pub basis: CostBasis,
    /// Calls with a price.
    pub priced_calls: u64,
    /// Calls without a price or without input and output counts; `usd` leaves
    /// them out.
    pub unpriced_calls: u64,
    /// Priced calls with a count priced at a substitute (see
    /// [`CallCost::approximate`](crate::pricing::CallCost::approximate)).
    pub approximate_calls: u64,
}

/// Statistics of one group of calls.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// The group.
    pub key: GroupKey,
    /// Calls in the group.
    pub calls: u64,
    /// Calls that failed.
    pub failed_calls: u64,
    /// Calls the agent made to compact its context.
    pub compaction_calls: u64,
    /// Token totals.
    pub tokens: TokenTotals,
    /// Cache reads over all input of the calls that report cache counts, or
    /// `None` when none does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate: Option<f64>,
    /// The cost at API prices, when a price table was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostTotal>,
    /// The start of the first call, in Unix milliseconds.
    pub first_started_at_ms: i64,
    /// The start of the last call, in Unix milliseconds.
    pub last_started_at_ms: i64,
}

/// Adds calls into groups one at a time, so a large history never has to be
/// in memory at once.
#[derive(Debug)]
pub struct Summarizer<'a> {
    group_by: GroupBy,
    prices: Option<&'a PriceTable>,
    groups: BTreeMap<GroupKey, Accumulator>,
}

#[derive(Debug)]
struct Accumulator {
    summary: Summary,
    cache_read: u64,
    cache_input: u64,
}

impl<'a> Summarizer<'a> {
    /// Groups calls by `group_by`, pricing them with `prices` when given.
    pub fn new(group_by: GroupBy, prices: Option<&'a PriceTable>) -> Self {
        Self {
            group_by,
            prices,
            groups: BTreeMap::new(),
        }
    }

    /// Adds a call.
    pub fn add(&mut self, entry: &CallEntry) {
        let call = &entry.call;
        let key = group_key(self.group_by, entry);
        let accumulator = self
            .groups
            .entry(key.clone())
            .or_insert_with(|| Accumulator {
                summary: Summary {
                    key,
                    calls: 0,
                    failed_calls: 0,
                    compaction_calls: 0,
                    tokens: TokenTotals::default(),
                    cache_hit_rate: None,
                    cost: self.prices.map(|_| CostTotal {
                        usd: 0.0,
                        basis: CostBasis::ApiPriceEquivalent,
                        priced_calls: 0,
                        unpriced_calls: 0,
                        approximate_calls: 0,
                    }),
                    first_started_at_ms: call.started_at_ms,
                    last_started_at_ms: call.started_at_ms,
                },
                cache_read: 0,
                cache_input: 0,
            });
        let summary = &mut accumulator.summary;
        summary.calls += 1;
        summary.failed_calls += u64::from(call.error_category.is_some());
        summary.compaction_calls += u64::from(call.compaction);
        summary.first_started_at_ms = summary.first_started_at_ms.min(call.started_at_ms);
        summary.last_started_at_ms = summary.last_started_at_ms.max(call.started_at_ms);

        let tokens = &call.tokens;
        let totals = &mut summary.tokens;
        for (total, count) in [
            (&mut totals.input, tokens.input),
            (&mut totals.output, tokens.output),
            (&mut totals.cache_read, tokens.cache_read),
            (&mut totals.cache_write_5m, tokens.cache_write_5m),
            (&mut totals.cache_write_1h, tokens.cache_write_1h),
            (&mut totals.reasoning, tokens.reasoning),
        ] {
            *total = total.saturating_add(count.unwrap_or(0));
        }
        if tokens.reports_cache() {
            let read = tokens.cache_read.unwrap_or(0);
            accumulator.cache_read = accumulator.cache_read.saturating_add(read);
            accumulator.cache_input = [
                tokens.input,
                Some(read),
                tokens.cache_write_5m,
                tokens.cache_write_1h,
            ]
            .into_iter()
            .flatten()
            .fold(accumulator.cache_input, u64::saturating_add);
        }

        if let (Some(prices), Some(cost)) = (self.prices, summary.cost.as_mut()) {
            let priced = model_name(call)
                .and_then(|model| prices.lookup(call.provider.as_deref(), model))
                .and_then(|price| call_cost(price, tokens));
            match priced {
                Some(priced) => {
                    cost.usd += priced.usd;
                    cost.priced_calls += 1;
                    cost.approximate_calls += u64::from(priced.approximate);
                }
                None => cost.unpriced_calls += 1,
            }
        }
    }

    /// The groups, in order of their keys.
    pub fn finish(self) -> Vec<Summary> {
        self.groups
            .into_values()
            .map(|accumulator| {
                let mut summary = accumulator.summary;
                summary.cache_hit_rate = (accumulator.cache_input > 0)
                    .then(|| accumulator.cache_read as f64 / accumulator.cache_input as f64);
                summary
            })
            .collect()
    }
}

/// Groups `entries` by `group_by`; see [`Summarizer`].
pub fn summarize<'e>(
    entries: impl IntoIterator<Item = &'e CallEntry>,
    group_by: GroupBy,
    prices: Option<&PriceTable>,
) -> Vec<Summary> {
    let mut summarizer = Summarizer::new(group_by, prices);
    for entry in entries {
        summarizer.add(entry);
    }
    summarizer.finish()
}

/// The output speed of a call, in tokens per second.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "tokensPerSecond")]
pub enum Speed {
    /// Output tokens over the time from the first token to the last.
    Measured(f64),
    /// Output tokens over the whole call, the wait for the first token
    /// included: the actual speed is at least this.
    AtLeast(f64),
}

impl Speed {
    /// The tokens per second.
    pub fn tokens_per_second(self) -> f64 {
        match self {
            Speed::Measured(speed) | Speed::AtLeast(speed) => speed,
        }
    }
}

/// The speed of a call, or `None` when it failed, lacks the counts, or its
/// timing says nothing reliable.
pub fn call_speed(call: &CallRecord) -> Option<Speed> {
    if call.error_category.is_some() {
        return None;
    }
    let output = call.tokens.output.filter(|output| *output > 0)?;
    let duration = call.duration_ms?;
    let speed = match call.first_token_ms {
        Some(first) if duration > first => Speed::Measured(per_second(output, duration - first)),
        _ if output >= MIN_LOWER_BOUND_OUTPUT_TOKENS && duration >= MIN_LOWER_BOUND_DURATION_MS => {
            Speed::AtLeast(per_second(output, duration))
        }
        _ => return None,
    };
    (speed.tokens_per_second() <= MAX_TOKENS_PER_SECOND).then_some(speed)
}

/// The median speed of a model.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedRank {
    /// The model that answered, or the requested one.
    pub model: String,
    /// Calls the median covers.
    pub calls: u64,
    /// The median, in tokens per second.
    pub median_tokens_per_second: f64,
    /// Whether a lower bound is among the speeds, so the median may be low.
    pub lower_bound: bool,
}

/// Ranks models by median speed, fastest first, over calls with at least
/// [`MIN_RANKED_OUTPUT_TOKENS`] output tokens and a speed.
pub fn rank_speed<'c>(calls: impl IntoIterator<Item = &'c CallRecord>) -> Vec<SpeedRank> {
    let mut by_model: BTreeMap<&str, (Vec<f64>, bool)> = BTreeMap::new();
    for call in calls {
        if call.tokens.output.unwrap_or(0) < MIN_RANKED_OUTPUT_TOKENS {
            continue;
        }
        let (Some(model), Some(speed)) = (model_name(call), call_speed(call)) else {
            continue;
        };
        let (speeds, lower_bound) = by_model.entry(model).or_default();
        speeds.push(speed.tokens_per_second());
        *lower_bound |= matches!(speed, Speed::AtLeast(_));
    }
    let mut ranks: Vec<SpeedRank> = by_model
        .into_iter()
        .map(|(model, (mut speeds, lower_bound))| SpeedRank {
            model: model.to_string(),
            calls: speeds.len() as u64,
            median_tokens_per_second: median(&mut speeds),
            lower_bound,
        })
        .collect();
    ranks.sort_by(|left, right| {
        right
            .median_tokens_per_second
            .total_cmp(&left.median_tokens_per_second)
            .then_with(|| left.model.cmp(&right.model))
    });
    ranks
}

/// The model a call is counted under: the one that answered, or the one asked
/// for when the answer did not name it.
pub fn model_name(call: &CallRecord) -> Option<&str> {
    call.model.as_deref().or(call.requested_model.as_deref())
}

/// The median of non-empty `values`; the mean of the middle two for an even count.
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn per_second(tokens: u64, milliseconds: u64) -> f64 {
    tokens as f64 * 1000.0 / milliseconds as f64
}

fn group_key(group_by: GroupBy, entry: &CallEntry) -> GroupKey {
    match group_by {
        GroupBy::Day { utc_offset_seconds } => GroupKey::Day {
            date: civil_date(entry.call.started_at_ms, utc_offset_seconds),
        },
        GroupBy::Agent => GroupKey::Agent {
            agent: entry.agent.clone(),
        },
        GroupBy::Model => GroupKey::Model {
            model: model_name(&entry.call).map(str::to_string),
        },
        GroupBy::Session => GroupKey::Session {
            agent: entry.agent.clone(),
            session_id: entry.call.session_id.clone(),
        },
        GroupBy::Cwd => GroupKey::Cwd {
            cwd: entry.cwd.clone(),
        },
    }
}

/// The calendar date, as `YYYY-MM-DD`, of `at_ms` at a UTC offset.
pub(crate) fn civil_date(at_ms: i64, utc_offset_seconds: i32) -> String {
    let local_ms = at_ms.saturating_add(i64::from(utc_offset_seconds) * 1000);
    let days = local_ms.div_euclid(MILLISECONDS_PER_DAY);
    // Days since 1970-01-01 to a proleptic Gregorian date, counting in
    // 400-year eras that start on March 1st so leap days end each year.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
