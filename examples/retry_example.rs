use std::time::Duration;
use topcoat_si::{Anthropic, KnownModel, MessageCreateParams, MessageParam, Model};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a client with custom retry settings: up to 5 retries, 30 seconds per attempt.
    let client = Anthropic::try_new(None)?
        .with_max_retries(5)
        .with_timeout(Duration::from_secs(30))?;

    // The Debug output shows the timeout and max_retries the client will use.
    println!("Client configured with retry settings: {client:?}");
    println!("- Retried: connection failures and HTTP 408, 409, 429 and 5xx responses");
    println!(
        "- Backoff before retry n (from 0): 0.5 s * 2^n capped at 8 s, plus up to 25% jitter, \
         never above 8 s"
    );
    println!(
        "- A longer Retry-After header is waited out instead; one over 60 seconds fails at once"
    );
    println!();

    let message = MessageParam::user("Hello, how are you?");

    let params = MessageCreateParams::new(
        1000, // max tokens
        vec![message],
        Model::Known(KnownModel::ClaudeSonnet55),
    );

    println!("Sending message with automatic retry on failures...");
    match client.send(&params).await {
        Ok(message) => {
            println!("Success! Message received:");
            for content in &message.content {
                if let Some(text_block) = content.as_text() {
                    println!("{}", text_block.text);
                }
            }
        }
        Err(e) => {
            println!("Request failed: {e}");
            if e.is_rate_limit() {
                println!(
                    "This was a rate limit error - it was retried up to max_retries times, unless Retry-After was over 60 s"
                );
            } else if e.is_retryable() {
                println!(
                    "This was a retryable error - the client would have automatically retried"
                );
            } else {
                println!("This was a non-retryable error - no retry was attempted");
            }
        }
    }

    Ok(())
}
