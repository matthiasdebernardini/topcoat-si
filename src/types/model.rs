use serde::{Deserialize, Serialize, Serializer};
use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use crate::types::{CacheCreation, InferenceSpeed, ServiceTier, Usage, UsageIteration};

/// Represents an Anthropic model identifier.
///
/// This can be a predefined model version or a custom string value
/// for models that may be added in the future.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Model {
    /// Known model versions
    Known(KnownModel),

    /// Custom model identifier (for future models or private models)
    Custom(String),
}

/// The current generation of Anthropic models.
///
/// This enum holds only the current generation.  Older or preview models are passed as
/// [`Model::Custom`]; they round-trip unchanged but have no built-in pricing.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KnownModel {
    /// Claude Opus 5.5 (released 2026-09-22)
    ClaudeOpus55,
    /// Claude Sonnet 5.5 (released 2026-09-28)
    ClaudeSonnet55,
    /// Claude Haiku 5.5 (released 2026-10-07)
    ClaudeHaiku55,
    /// Claude Fable 5.1
    ClaudeFable51,
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Model::Known(known_model) => write!(f, "{known_model}"),
            Model::Custom(custom) => write!(f, "{custom}"),
        }
    }
}

/// Token rates for a model in nano-cents per token.
///
/// A nano-cent is one-billionth of a cent (1 micro-cent = 1,000 nano-cents), or
/// 1/100,000,000,000 of a dollar.  To convert from Anthropic's published pricing (dollars
/// per million tokens) to nano-cents per token, multiply by 100,000:
///
/// ```text
/// $3/MTok × (10,000,000,000 nano-cents/$) ÷ 1,000,000 tokens = 300,000 nano-cents/token
/// ```
///
/// | Price ($/MTok) | Nano-cents/token |
/// |----------------|------------------|
/// | $0.10          | 10,000           |
/// | $1             | 100,000          |
/// | $3             | 300,000          |
/// | $15            | 1,500,000        |
///
/// Rates from <https://platform.claude.com/docs/en/about-claude/pricing> as of 2026-10-19.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TokenRates {
    /// Cost per input token in nano-cents.
    pub input: u64,
    /// Cost per output token in nano-cents.
    pub output: u64,
    /// Cost per 5-minute cache write token in nano-cents (1.25x input).
    pub cache_write_5m: u64,
    /// Cost per 1-hour cache write token in nano-cents (2x input).
    pub cache_write_1h: u64,
    /// Cost per cache read token in nano-cents.
    pub cache_read: u64,
}

impl TokenRates {
    /// Rates where every token kind costs `rate` nano-cents.
    #[must_use]
    pub const fn flat(rate: u64) -> Self {
        Self {
            input: rate,
            output: rate,
            cache_write_5m: rate,
            cache_write_1h: rate,
            cache_read: rate,
        }
    }

    /// Message Batches rates: 50% off every field, rounded up.
    #[must_use]
    pub const fn batch(self) -> Self {
        Self {
            input: self.input.div_ceil(2),
            output: self.output.div_ceil(2),
            cache_write_5m: self.cache_write_5m.div_ceil(2),
            cache_write_1h: self.cache_write_1h.div_ceil(2),
            cache_read: self.cache_read.div_ceil(2),
        }
    }

    /// Data-residency pricing for `inference_geo: "us"`: 1.1x on every token type, rounded
    /// up and saturating at `u64::MAX`.
    #[must_use]
    pub const fn us_inference(self) -> Self {
        const fn scale(rate: u64) -> u64 {
            let scaled = (rate as u128 * 11).div_ceil(10);
            if scaled > u64::MAX as u128 {
                u64::MAX
            } else {
                scaled as u64
            }
        }
        Self {
            input: scale(self.input),
            output: scale(self.output),
            cache_write_5m: scale(self.cache_write_5m),
            cache_write_1h: scale(self.cache_write_1h),
            cache_read: scale(self.cache_read),
        }
    }

    /// Cost of `usage` in micro-cents, rounded up.
    ///
    /// Negative token counts count as zero.  When `usage.cache_creation` is present its
    /// 5-minute and 1-hour counts are priced separately, and any excess of
    /// `cache_creation_input_tokens` over their sum is priced as 5-minute writes; otherwise
    /// all `cache_creation_input_tokens` are priced as 5-minute writes.  Returns `u64::MAX`
    /// on overflow.  This prices the top-level counts only; see
    /// [`KnownModel::cost_micro_cents`] for the iteration-aware total.
    #[must_use]
    pub fn cost_micro_cents(&self, usage: &Usage) -> u64 {
        let (write_5m, write_1h) = cache_writes(
            usage.cache_creation_input_tokens.unwrap_or(0),
            usage.cache_creation,
        );
        self.cost_of_tokens(
            clamp(usage.input_tokens),
            clamp(usage.output_tokens),
            write_5m,
            write_1h,
            clamp(usage.cache_read_input_tokens.unwrap_or(0)),
        )
    }

    /// Cost in micro-cents, rounded up, of the given token counts.  Returns `u64::MAX` on
    /// overflow.
    fn cost_of_tokens(
        &self,
        input: u64,
        output: u64,
        cache_write_5m: u64,
        cache_write_1h: u64,
        cache_read: u64,
    ) -> u64 {
        let parts = [
            (input, self.input),
            (output, self.output),
            (cache_write_5m, self.cache_write_5m),
            (cache_write_1h, self.cache_write_1h),
            (cache_read, self.cache_read),
        ];
        let nano_cents = parts
            .into_iter()
            .map(|(tokens, rate)| u128::from(tokens) * u128::from(rate))
            .fold(0, u128::saturating_add);
        u64::try_from(nano_cents.div_ceil(1000)).unwrap_or(u64::MAX)
    }
}

/// A token count as `u64`, with negative counts as zero.
fn clamp(tokens: i32) -> u64 {
    u64::try_from(tokens).unwrap_or(0)
}

/// The 5-minute and 1-hour cache-write counts for `total` cache-creation tokens.
///
/// Without a breakdown every write is a 5-minute write.  With one, the excess of `total` over
/// the breakdown's sum is added to the 5-minute count.
fn cache_writes(total: i32, breakdown: Option<CacheCreation>) -> (u64, u64) {
    let Some(breakdown) = breakdown else {
        return (clamp(total), 0);
    };
    let write_5m = clamp(breakdown.ephemeral_5m_input_tokens);
    let write_1h = clamp(breakdown.ephemeral_1h_input_tokens);
    let excess = clamp(total).saturating_sub(write_5m.saturating_add(write_1h));
    (write_5m.saturating_add(excess), write_1h)
}

impl KnownModel {
    /// Prompt size above which Claude Haiku 5.5 bills the whole request at the higher tier.
    pub const HAIKU_55_TIER_THRESHOLD_TOKENS: u64 = 100_000;

    /// Returns the token rates for this model in nano-cents per token.
    ///
    /// For Claude Haiku 5.5 these are the rates for prompts of at most
    /// [`Self::HAIKU_55_TIER_THRESHOLD_TOKENS`] tokens; use
    /// [`token_rates_for_prompt`](Self::token_rates_for_prompt) for the tier-aware rates.
    ///
    /// # Example
    ///
    /// ```rust
    /// use carpa::KnownModel;
    ///
    /// let rates = KnownModel::ClaudeSonnet55.token_rates();
    /// assert_eq!(rates.input, 200_000);   // $2/MTok
    /// assert_eq!(rates.output, 1_000_000); // $10/MTok
    /// ```
    #[must_use]
    pub fn token_rates(&self) -> TokenRates {
        let rates = |input, output, cache_write_5m, cache_write_1h, cache_read| TokenRates {
            input,
            output,
            cache_write_5m,
            cache_write_1h,
            cache_read,
        };
        match self {
            // $10/$50, $12.50/$20, $0.25
            KnownModel::ClaudeFable51 => rates(1_000_000, 5_000_000, 1_250_000, 2_000_000, 25_000),
            // $4/$20, $5/$8, $0.20
            KnownModel::ClaudeOpus55 => rates(400_000, 2_000_000, 500_000, 800_000, 20_000),
            // $2/$10, $2.50/$4, $0.10
            KnownModel::ClaudeSonnet55 => rates(200_000, 1_000_000, 250_000, 400_000, 10_000),
            // $0.10/$0.50, $0.125/$0.20, $0.01 (prompt <= 100,000 tokens)
            KnownModel::ClaudeHaiku55 => rates(10_000, 50_000, 12_500, 20_000, 1_000),
        }
    }

    /// Returns the token rates that apply to a request with `prompt_tokens` prompt tokens.
    ///
    /// Claude Haiku 5.5 bills the whole request at $0.50/$2.50 (and matching cache rates)
    /// when the prompt exceeds [`Self::HAIKU_55_TIER_THRESHOLD_TOKENS`]; every other model
    /// returns [`token_rates`](Self::token_rates).
    #[must_use]
    pub fn token_rates_for_prompt(&self, prompt_tokens: u64) -> TokenRates {
        match self {
            KnownModel::ClaudeHaiku55 if prompt_tokens > Self::HAIKU_55_TIER_THRESHOLD_TOKENS => {
                TokenRates {
                    input: 50_000,
                    output: 250_000,
                    cache_write_5m: 62_500,
                    cache_write_1h: 100_000,
                    cache_read: 5_000,
                }
            }
            _ => self.token_rates(),
        }
    }

    /// Fast-mode rates (`speed: "fast"`), or `None` when the model has no fast mode.
    ///
    /// Prompt-caching multipliers (1.25x, 2x, 0.05x or 0.1x) stack on the fast base price.
    #[must_use]
    pub fn fast_token_rates(&self) -> Option<TokenRates> {
        let rates = |input, output, cache_write_5m, cache_write_1h, cache_read| {
            Some(TokenRates {
                input,
                output,
                cache_write_5m,
                cache_write_1h,
                cache_read,
            })
        };
        match self {
            KnownModel::ClaudeOpus55 => rates(800_000, 4_000_000, 1_000_000, 1_600_000, 40_000),
            _ => None,
        }
    }

    /// The per-token rates for a request of `prompt_tokens` prompt tokens, taking the pricing
    /// modifiers (speed, geography, service tier) from `usage`.
    ///
    /// Starts from [`fast_token_rates`](Self::fast_token_rates) when `usage.speed` is fast and
    /// the model has a fast table, otherwise from
    /// [`token_rates_for_prompt`](Self::token_rates_for_prompt).  Then applies
    /// [`TokenRates::us_inference`] when `usage.inference_geo` is `"us"` and
    /// [`TokenRates::batch`] when `usage.service_tier` is batch.
    #[must_use]
    pub fn rates_for_prompt_and_modifiers(&self, prompt_tokens: u64, usage: &Usage) -> TokenRates {
        let fast = match usage.speed {
            Some(InferenceSpeed::Fast) => self.fast_token_rates(),
            _ => None,
        };
        let mut rates = fast.unwrap_or_else(|| self.token_rates_for_prompt(prompt_tokens));
        if usage.inference_geo.as_deref() == Some("us") {
            rates = rates.us_inference();
        }
        if usage.service_tier == Some(ServiceTier::Batch) {
            rates = rates.batch();
        }
        rates
    }

    /// Cost of `usage` in micro-cents, rounded up.
    ///
    /// Applies the pricing modifiers recorded in `usage`:
    ///
    /// 1. the Claude Haiku 5.5 long-prompt tier, chosen by the prompt-token count;
    /// 2. fast-mode rates when `usage.speed` is fast and the model has them;
    /// 3. the 1.1x US data-residency multiplier when `usage.inference_geo` is `"us"`;
    /// 4. the 50% Message Batches discount when `usage.service_tier` is batch.
    ///
    /// Cache writes are priced per lifetime (5-minute and 1-hour) when
    /// `usage.cache_creation` is present, with any excess of `cache_creation_input_tokens`
    /// over the breakdown priced at the 5-minute rate.
    ///
    /// When `usage.iterations` holds at least one known-kind iteration (`message`,
    /// `fallback_message`, `compaction` or `advisor_message`), the billed total is the sum
    /// across those iterations, because the top-level counts exclude compaction iterations.
    /// Each iteration is priced on its own `model` when that is a [`Model::Known`], otherwise
    /// on `self`, with the same speed, geography and tier modifiers as `usage` and the
    /// Claude Haiku 5.5 tier chosen from that iteration's prompt tokens.  An iteration's
    /// cache writes are priced per lifetime when it carries a `cache_creation` breakdown and
    /// at the 5-minute rate otherwise.  When any iteration has an unknown kind, the result is
    /// the larger of the top-level cost and the sum over the known iterations.
    pub fn cost_micro_cents(&self, usage: &Usage) -> u64 {
        let iterations = usage.iterations.as_deref().unwrap_or_default();
        let has_unknown = iterations
            .iter()
            .any(|iteration| matches!(iteration, UsageIteration::Unknown));
        let mut known = iterations.iter().filter_map(|iteration| match iteration {
            UsageIteration::Message(it)
            | UsageIteration::FallbackMessage(it)
            | UsageIteration::Compaction(it)
            | UsageIteration::AdvisorMessage(it) => Some(it),
            UsageIteration::Unknown => None,
        });
        let top_level = || {
            self.rates_for_prompt_and_modifiers(usage.prompt_tokens(), usage)
                .cost_micro_cents(usage)
        };
        let Some(first) = known.next() else {
            return top_level();
        };
        let iterations_cost = std::iter::once(first)
            .chain(known)
            .map(|it| {
                let model = match &it.model {
                    Some(Model::Known(model)) => model,
                    _ => self,
                };
                let (input, output) = (clamp(it.input_tokens), clamp(it.output_tokens));
                let read = clamp(it.cache_read_input_tokens);
                let (write_5m, write_1h) =
                    cache_writes(it.cache_creation_input_tokens, it.cache_creation);
                // The write counts sum to the larger of the total and the breakdown's sum, so
                // a breakdown without a total still counts toward the Haiku 5.5 tier.
                let prompt_tokens = input
                    .saturating_add(write_5m.saturating_add(write_1h))
                    .saturating_add(read);
                model
                    .rates_for_prompt_and_modifiers(prompt_tokens, usage)
                    .cost_of_tokens(input, output, write_5m, write_1h, read)
            })
            .fold(0, u64::saturating_add);
        if has_unknown {
            iterations_cost.max(top_level())
        } else {
            iterations_cost
        }
    }
}

impl fmt::Display for KnownModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KnownModel::ClaudeOpus55 => write!(f, "claude-opus-5-5"),
            KnownModel::ClaudeSonnet55 => write!(f, "claude-sonnet-5-5"),
            KnownModel::ClaudeHaiku55 => write!(f, "claude-haiku-5-5"),
            KnownModel::ClaudeFable51 => write!(f, "claude-fable-5-1"),
        }
    }
}

impl Serialize for Model {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize the model as a string using Display implementation
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Model {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        // Anything that is not a known model is a custom model
        Ok(KnownModel::from_str(&s)
            .map(Model::Known)
            .unwrap_or(Model::Custom(s)))
    }
}

impl From<KnownModel> for Model {
    fn from(model: KnownModel) -> Self {
        Model::Known(model)
    }
}

/// Error returned when parsing a string that is not a known model id.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseKnownModelError {
    /// The string that is not a known model id.
    pub invalid_value: String,
}

impl fmt::Display for ParseKnownModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unknown model: {}", self.invalid_value)
    }
}

impl std::error::Error for ParseKnownModelError {}

impl FromStr for KnownModel {
    type Err = ParseKnownModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude-opus-5-5" => Ok(KnownModel::ClaudeOpus55),
            "claude-sonnet-5-5" => Ok(KnownModel::ClaudeSonnet55),
            "claude-haiku-5-5" => Ok(KnownModel::ClaudeHaiku55),
            "claude-fable-5-1" => Ok(KnownModel::ClaudeFable51),
            _ => Err(ParseKnownModelError {
                invalid_value: s.to_string(),
            }),
        }
    }
}

impl FromStr for Model {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match KnownModel::from_str(s) {
            Ok(known_model) => Ok(Model::Known(known_model)),
            Err(_) => Ok(Model::Custom(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IterationUsage;

    #[test]
    fn known_model_serialization() {
        let model = Model::Known(KnownModel::ClaudeSonnet55);
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, r#""claude-sonnet-5-5""#);

        let model = Model::Known(KnownModel::ClaudeFable51);
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, r#""claude-fable-5-1""#);
    }

    #[test]
    fn custom_model_serialization() {
        let model = Model::Custom("claude-4-custom".to_string());
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, r#""claude-4-custom""#);
    }

    #[test]
    fn model_deserialization() {
        let json = r#""claude-sonnet-5-5""#;
        let model: Model = serde_json::from_str(json).unwrap();
        assert_eq!(model, Model::Known(KnownModel::ClaudeSonnet55));

        let json = r#""claude-4-custom""#;
        let model: Model = serde_json::from_str(json).unwrap();
        assert_eq!(model, Model::Custom("claude-4-custom".to_string()));
    }

    #[test]
    fn claude_55_models_round_trip() {
        for (known, id) in [
            (KnownModel::ClaudeOpus55, "claude-opus-5-5"),
            (KnownModel::ClaudeSonnet55, "claude-sonnet-5-5"),
            (KnownModel::ClaudeHaiku55, "claude-haiku-5-5"),
            (KnownModel::ClaudeFable51, "claude-fable-5-1"),
        ] {
            let model = Model::Known(known);
            assert_eq!(model.to_string(), id);
            assert_eq!(
                serde_json::from_str::<Model>(&format!("\"{id}\"")).unwrap(),
                model
            );
            assert_eq!(id.parse::<KnownModel>(), Ok(known));
        }
    }

    #[test]
    fn retired_models_are_custom() {
        for id in [
            "claude-3-7-sonnet-latest",
            "claude-3-7-sonnet-20250219",
            "claude-sonnet-4-20250514",
            "claude-sonnet-4-0",
            "claude-4-sonnet-20250514",
            "claude-opus-4-0",
            "claude-opus-4-20250514",
            "claude-4-opus-20250514",
            "claude-opus-4-1-20250805",
            "claude-3-opus-latest",
            "claude-3-opus-20240229",
            "claude-3-haiku-20240307",
        ] {
            let model: Model = serde_json::from_str(&format!("\"{id}\"")).unwrap();
            assert_eq!(model, Model::Custom(id.to_string()));
            assert_eq!(serde_json::to_string(&model).unwrap(), format!("\"{id}\""));
        }
    }

    #[test]
    fn older_generation_ids_are_custom() {
        for id in ["claude-haiku-4-5", "claude-opus-5"] {
            let model: Model = serde_json::from_str(&format!("\"{id}\"")).unwrap();
            assert_eq!(model, Model::Custom(id.to_string()));
            assert_eq!(
                id.parse::<KnownModel>(),
                Err(ParseKnownModelError {
                    invalid_value: id.to_string()
                })
            );
            assert_eq!(serde_json::to_string(&model).unwrap(), format!("\"{id}\""));
        }
    }

    #[test]
    fn display() {
        let model = Model::Known(KnownModel::ClaudeSonnet55);
        assert_eq!(model.to_string(), "claude-sonnet-5-5");

        let model = Model::Custom("claude-4-custom".to_string());
        assert_eq!(model.to_string(), "claude-4-custom");
    }

    fn rates(input: u64, output: u64, w5: u64, w1: u64, read: u64) -> TokenRates {
        TokenRates {
            input,
            output,
            cache_write_5m: w5,
            cache_write_1h: w1,
            cache_read: read,
        }
    }

    #[test]
    fn token_rates_fable_51() {
        assert_eq!(
            KnownModel::ClaudeFable51.token_rates(),
            rates(1_000_000, 5_000_000, 1_250_000, 2_000_000, 25_000)
        );
    }

    #[test]
    fn token_rates_55_generation() {
        assert_eq!(
            KnownModel::ClaudeOpus55.token_rates(),
            rates(400_000, 2_000_000, 500_000, 800_000, 20_000)
        );
        assert_eq!(
            KnownModel::ClaudeSonnet55.token_rates(),
            rates(200_000, 1_000_000, 250_000, 400_000, 10_000)
        );
    }

    #[test]
    fn token_rates_haiku_55_tiers() {
        let low = rates(10_000, 50_000, 12_500, 20_000, 1_000);
        let high = rates(50_000, 250_000, 62_500, 100_000, 5_000);
        assert_eq!(KnownModel::ClaudeHaiku55.token_rates(), low);
        assert_eq!(
            KnownModel::ClaudeHaiku55.token_rates_for_prompt(100_000),
            low
        );
        assert_eq!(
            KnownModel::ClaudeHaiku55.token_rates_for_prompt(100_001),
            high
        );
        // Other models are not tiered.
        assert_eq!(
            KnownModel::ClaudeSonnet55.token_rates_for_prompt(1_000_000),
            KnownModel::ClaudeSonnet55.token_rates()
        );
    }

    #[test]
    fn batch_halves_every_field() {
        let batch = KnownModel::ClaudeSonnet55.token_rates().batch();
        assert_eq!(batch, rates(100_000, 500_000, 125_000, 200_000, 5_000));
        assert_eq!(TokenRates::flat(7).batch(), TokenRates::flat(4));
    }

    #[test]
    fn batch_rounds_up_so_a_rate_of_one_stays_one() {
        assert_eq!(TokenRates::flat(1).batch(), TokenRates::flat(1));
    }

    #[test]
    fn us_inference_rounds_up_and_saturates() {
        assert_eq!(TokenRates::flat(1).us_inference(), TokenRates::flat(2));
        assert_eq!(
            TokenRates::flat(u64::MAX).us_inference(),
            TokenRates::flat(u64::MAX)
        );
    }

    #[test]
    fn max_rates_with_large_counts_saturate() {
        let usage = Usage::new(i32::MAX, i32::MAX)
            .with_cache_creation_input_tokens(i32::MAX)
            .with_cache_creation(CacheCreation::new(i32::MAX, i32::MAX))
            .with_cache_read_input_tokens(i32::MAX);
        assert_eq!(
            TokenRates::flat(u64::MAX).cost_micro_cents(&usage),
            u64::MAX
        );
        assert_eq!(
            TokenRates::flat(u64::MAX).cost_of_tokens(u64::MAX, u64::MAX, u64::MAX, u64::MAX, 1),
            u64::MAX
        );
    }

    #[test]
    fn nano_cents_beyond_u64_still_fit_after_division() {
        let input = u64::MAX / 100;
        assert_eq!(
            rates(200, 0, 0, 0, 0).cost_of_tokens(input, 0, 0, 0, 0),
            (u128::from(input) * 200).div_ceil(1000) as u64
        );
    }

    #[test]
    fn cache_creation_excess_over_breakdown_is_priced_at_5m() {
        // Sonnet 5.5: breakdown 2000 5m + 3000 1h, total 6000 so 1000 excess at 5m.
        // (2000 + 1000) * 250_000 + 3000 * 400_000 = 1_950_000_000 nano = 1_950_000 micro.
        let usage = Usage::new(0, 0)
            .with_cache_creation_input_tokens(6000)
            .with_cache_creation(CacheCreation::new(2000, 3000));
        assert_eq!(
            KnownModel::ClaudeSonnet55.cost_micro_cents(&usage),
            1_950_000
        );
    }

    #[test]
    fn cost_with_one_hour_cache_write() {
        // Sonnet 5.5 nano-cents: input 1000 x 200,000 = 200,000,000
        // output 100 x 1,000,000 = 100,000,000
        // 5m write 2000 x 250,000 = 500,000,000
        // 1h write 3000 x 400,000 = 1,200,000,000
        // read 4000 x 10,000 = 40,000,000
        // total 2,040,000,000 nano-cents = 2,040,000 micro-cents
        let usage = Usage::new(1000, 100)
            .with_cache_creation_input_tokens(5000)
            .with_cache_creation(CacheCreation::new(2000, 3000))
            .with_cache_read_input_tokens(4000);
        assert_eq!(
            KnownModel::ClaudeSonnet55.cost_micro_cents(&usage),
            2_040_000
        );
    }

    #[test]
    fn cost_without_breakdown_uses_5m_write_rate() {
        // 1000 tokens x 250,000 nano-cents = 250,000,000 nano = 250,000 micro-cents
        let usage = Usage::new(0, 0).with_cache_creation_input_tokens(1000);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 250_000);
    }

    #[test]
    fn cost_rounds_up_to_micro_cents() {
        // 1 Haiku 5.5 5m cache write token = 12,500 nano-cents = 12.5 micro-cents -> 13
        let usage = Usage::new(0, 0).with_cache_creation_input_tokens(1);
        assert_eq!(KnownModel::ClaudeHaiku55.cost_micro_cents(&usage), 13);
        assert_eq!(
            KnownModel::ClaudeHaiku55.cost_micro_cents(&Usage::new(0, 0)),
            0
        );
    }

    #[test]
    fn cost_clamps_negative_and_saturates() {
        let usage = Usage::new(-5, -5);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 0);

        let usage = Usage::new(i32::MAX, i32::MAX);
        assert_eq!(
            TokenRates::flat(u64::MAX).cost_micro_cents(&usage),
            u64::MAX
        );
    }

    #[test]
    fn haiku_55_cost_picks_high_tier_above_threshold() {
        // 100,000 prompt tokens: low tier, 100,000 x 10,000 nano = 1,000,000 micro-cents
        let at = Usage::new(100_000, 0);
        assert_eq!(KnownModel::ClaudeHaiku55.cost_micro_cents(&at), 1_000_000);

        // 100,001 prompt tokens: high tier, 100,001 x 50,000 nano = 5,000,050,000 nano
        // = 5,000,050 micro-cents
        let above = Usage::new(100_001, 0);
        assert_eq!(
            KnownModel::ClaudeHaiku55.cost_micro_cents(&above),
            5_000_050
        );

        // Cache reads count toward the prompt: 1 input + 100,000 read = 100,001 tokens.
        // High tier: 1 x 50,000 + 100,000 x 5,000 = 500,050,000 nano = 500,050 micro-cents.
        let cached = Usage::new(1, 0).with_cache_read_input_tokens(100_000);
        assert_eq!(KnownModel::ClaudeHaiku55.cost_micro_cents(&cached), 500_050);
    }

    #[test]
    fn opus_55_fast_mode_cost() {
        // 1000 input * 800_000 + 100 output * 4_000_000 = 1_200_000_000 nano-cents
        // = 1_200_000 micro-cents.
        let usage = Usage::new(1000, 100).with_speed(InferenceSpeed::Fast);
        assert_eq!(KnownModel::ClaudeOpus55.cost_micro_cents(&usage), 1_200_000);
    }

    #[test]
    fn haiku_55_us_inference_uses_high_tier_times_1_1() {
        // 150_000 prompt tokens exceed the 100_000 threshold: input 50_000 * 11 / 10 = 55_000.
        // 150_000 * 55_000 = 8_250_000_000 nano-cents = 8_250_000 micro-cents.
        let usage = Usage::new(150_000, 0).with_inference_geo("us");
        assert_eq!(
            KnownModel::ClaudeHaiku55.cost_micro_cents(&usage),
            8_250_000
        );
    }

    #[test]
    fn sonnet_55_batch_halves() {
        // Batch rates: input 100_000, output 500_000.
        // 1000 * 100_000 + 100 * 500_000 = 150_000_000 nano-cents = 150_000 micro-cents.
        let usage = Usage::new(1000, 100).with_service_tier(ServiceTier::Batch);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 150_000);
    }

    #[test]
    fn fast_speed_without_fast_table_uses_standard_rates() {
        // Sonnet 5.5: 1000 * 200_000 + 100 * 1_000_000 = 300_000_000 nano-cents.
        let usage = Usage::new(1000, 100).with_speed(InferenceSpeed::Fast);
        assert_eq!(KnownModel::ClaudeSonnet55.fast_token_rates(), None);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 300_000);
    }

    fn iteration(model: Option<Model>, input: i32, output: i32) -> IterationUsage {
        IterationUsage {
            model,
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: 0,
            cache_creation: None,
            cache_read_input_tokens: 0,
        }
    }

    #[test]
    fn iteration_one_hour_writes_use_the_one_hour_rate() {
        // Sonnet 5.5 iteration: 100 5m writes, 200 1h writes, 300 total so 0 excess.
        // 100 * 250_000 + 200 * 400_000 = 105_000_000 nano = 105_000 micro.
        let mut it = iteration(None, 0, 0);
        it.cache_creation_input_tokens = 300;
        it.cache_creation = Some(CacheCreation::new(100, 200));
        let mut usage = Usage::new(0, 0);
        usage.iterations = Some(vec![UsageIteration::Message(it.clone())]);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 105_000);

        // 50 more tokens than the breakdown: the excess is priced at 5m.
        // 105_000_000 + 50 * 250_000 = 117_500_000 nano = 117_500 micro.
        it.cache_creation_input_tokens = 350;
        usage.iterations = Some(vec![UsageIteration::Message(it)]);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 117_500);
    }

    #[test]
    fn unknown_iteration_takes_max_of_top_level_and_known_sum() {
        // Top level: 1000 * 200_000 + 100 * 1_000_000 = 300_000 micro.
        // Known iteration: 10 * 200_000 = 2_000 micro.  The top level wins.
        let mut usage = Usage::new(1000, 100);
        usage.iterations = Some(vec![
            UsageIteration::Unknown,
            UsageIteration::Message(iteration(None, 10, 0)),
        ]);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 300_000);

        // Known iteration larger than the top level: the iteration sum wins.
        usage.iterations = Some(vec![
            UsageIteration::Unknown,
            UsageIteration::Message(iteration(None, 5000, 0)),
        ]);
        assert_eq!(
            KnownModel::ClaudeSonnet55.cost_micro_cents(&usage),
            1_000_000
        );
    }

    #[test]
    fn iterations_sum_when_top_level_counts_are_zero() {
        // Sonnet 5.5 message: 1000 * 200_000 + 100 * 1_000_000 = 300_000_000 nano-cents.
        // Compaction on Opus 5.5 with 50 cache-write tokens at the 5m rate:
        // 500 * 400_000 + 10 * 2_000_000 + 50 * 500_000 = 245_000_000 nano-cents.
        let mut usage = Usage::new(0, 0);
        let mut compaction = iteration(Some(KnownModel::ClaudeOpus55.into()), 500, 10);
        compaction.cache_creation_input_tokens = 50;
        usage.iterations = Some(vec![
            UsageIteration::Message(iteration(
                Some(KnownModel::ClaudeSonnet55.into()),
                1000,
                100,
            )),
            UsageIteration::Compaction(compaction),
            UsageIteration::Unknown,
        ]);
        assert_eq!(
            KnownModel::ClaudeSonnet55.cost_micro_cents(&usage),
            300_000 + 245_000
        );
    }

    #[test]
    fn iteration_with_custom_or_missing_model_uses_self() {
        // Sonnet 5.5: 1000 * 200_000 + 100 * 1_000_000 = 300_000_000 nano-cents each.
        let mut usage = Usage::new(0, 0);
        usage.iterations = Some(vec![
            UsageIteration::Message(iteration(Some(Model::Custom("private".into())), 1000, 100)),
            UsageIteration::FallbackMessage(iteration(None, 1000, 100)),
        ]);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 600_000);
    }

    #[test]
    fn iterations_apply_modifiers_and_per_iteration_haiku_tier() {
        // Batch Haiku 5.5: the 150_000-token iteration uses the high tier halved
        // (input 25_000), the 1_000-token one the low tier halved (input 5_000).
        let mut usage = Usage::new(0, 0).with_service_tier(ServiceTier::Batch);
        usage.iterations = Some(vec![
            UsageIteration::Message(iteration(None, 150_000, 0)),
            UsageIteration::Message(iteration(None, 1_000, 0)),
        ]);
        // 150_000 * 25_000 + 1_000 * 5_000 = 3_755_000_000 nano-cents.
        assert_eq!(
            KnownModel::ClaudeHaiku55.cost_micro_cents(&usage),
            3_755_000
        );
    }

    #[test]
    fn iteration_breakdown_without_total_counts_toward_haiku_tier() {
        // 50_000 input + 60_000 5m writes exceed the 100_000 threshold: high tier.
        // 50_000 * 50_000 + 60_000 * 62_500 = 6_250_000_000 nano-cents.
        let mut it = iteration(None, 50_000, 0);
        it.cache_creation = Some(CacheCreation::new(60_000, 0));
        let mut usage = Usage::new(0, 0);
        usage.iterations = Some(vec![UsageIteration::Message(it)]);
        assert_eq!(
            KnownModel::ClaudeHaiku55.cost_micro_cents(&usage),
            6_250_000
        );
    }

    #[test]
    fn only_unknown_iterations_fall_back_to_top_level() {
        let mut usage = Usage::new(1000, 100);
        usage.iterations = Some(vec![UsageIteration::Unknown]);
        assert_eq!(KnownModel::ClaudeSonnet55.cost_micro_cents(&usage), 300_000);
    }

    #[test]
    fn us_inference_scales_every_field() {
        assert_eq!(TokenRates::flat(10).us_inference(), TokenRates::flat(11));
    }
}
