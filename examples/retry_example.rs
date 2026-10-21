use std::time::Duration;
use topcoat_si::{
    Anthropic, Error, ErrorKind, ErrorType, KnownModel, MessageCreateParams, MessageParam, Model,
    Service,
};

/// Says what to do about a failed request, using the three questions an `Error` answers: what
/// failed (`kind`), which service, and whether sending again might work (`is_retryable`).
fn advise(err: &Error) -> String {
    if err.is_retryable() {
        return match err.retry_after() {
            Some(wait) => format!("retryable: the service asked for {wait:?}"),
            None => "retryable: try again with a backoff of your own".to_string(),
        };
    }
    match err.kind() {
        ErrorKind::Api {
            service,
            status: Some(401),
            ..
        } => format!("check the {service} API key"),
        ErrorKind::Api {
            service, status, ..
        } => {
            format!("{service} refused with {status:?}; sending the same request again won't help")
        }
        ErrorKind::Validation { param, .. } => format!("fix the request ({param:?})"),
        ErrorKind::MaxTokens => "raise max_tokens".to_string(),
        // `ErrorKind` is non-exhaustive, so a wildcard arm is required.
        _ => "not retryable".to_string(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a client with custom retry settings: up to 5 retries, 30 seconds per attempt.
    let client = Anthropic::try_new(None)?
        .with_max_retries(5)
        .with_timeout(Duration::from_secs(30))?;

    // The Debug output shows the timeout and max_retries the client will use.
    println!("Client configured with retry settings: {client:?}");
    println!("- Retried: errors whose is_retryable() is true, except as noted below");
    println!("- Never resent automatically: a Timeout");
    println!(
        "- Interrupted (the response started, then stopped): only GET requests resend it, \
         since the server has already answered a POST or DELETE"
    );
    println!(
        "- Delay before retry n (from 0): 0.5 s * 2^n capped at 8 s, plus up to 25% jitter, or \
         the server's Retry-After when that is longer"
    );
    println!("- A Retry-After over 60 seconds fails at once");
    println!();

    // `Error::api` simulates a 429 without a network, as a downstream test would.
    let simulated = Error::api(
        Service::Anthropic,
        Some(429),
        Some(ErrorType::RateLimitError),
        "Rate limited",
        Some(Duration::from_secs(20)),
    );
    println!("Simulated: {simulated}");
    println!("  -> {}", advise(&simulated));
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
            // The client has already retried what it may; this is what is left.
            println!("Request failed: {e}");
            println!("  request id: {:?}", e.request_id());
            println!("  -> {}", advise(&e));
        }
    }

    Ok(())
}
