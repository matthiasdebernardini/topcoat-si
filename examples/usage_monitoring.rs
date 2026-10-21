use time::{Duration, OffsetDateTime};
use topcoat_si::{
    Anthropic, CostGroupBy, CostReportParams, KnownModel, MessageCreateParams, UsageGroupBy,
    UsageReportParams,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Anthropic::new(None)?;
    let model = KnownModel::ClaudeHaiku55;

    let message = client
        .send(MessageCreateParams::simple(
            "Say hello in five words.",
            model,
        ))
        .await?;
    let usage = &message.usage;
    println!("usage: {usage:#?}");

    let micro_cents = model.cost_micro_cents(usage);
    // 1 dollar = 100 cents = 100_000_000 micro-cents.
    println!("cost: ${:.6}", micro_cents as f64 / 100_000_000.0);
    println!("rate limits: {:#?}", client.last_rate_limits());

    let Ok(admin_key) = std::env::var("ANTHROPIC_ADMIN_KEY") else {
        println!("Set ANTHROPIC_ADMIN_KEY to also print the organization usage and cost reports.");
        return Ok(());
    };
    let admin = Anthropic::new(Some(admin_key))?;
    let starting_at = OffsetDateTime::now_utc() - Duration::days(7);

    let usage_report = admin
        .usage_report(
            UsageReportParams::new(starting_at)
                .with_bucket_width(topcoat_si::BucketWidth::OneDay)
                .group_by(UsageGroupBy::Model),
        )
        .await?;
    for bucket in &usage_report.data {
        for result in &bucket.results {
            println!(
                "{} {}: input {} output {}",
                bucket.starting_at.date(),
                result.model.as_deref().unwrap_or("-"),
                result.total_input_tokens(),
                result.output_tokens,
            );
        }
    }

    let cost_report = admin
        .cost_report(CostReportParams::new(starting_at).group_by(CostGroupBy::Description))
        .await?;
    for bucket in &cost_report.data {
        for result in &bucket.results {
            println!(
                "{} {}: {} {} cents",
                bucket.starting_at.date(),
                result.description.as_deref().unwrap_or("-"),
                result.amount,
                result.currency,
            );
        }
    }
    Ok(())
}
