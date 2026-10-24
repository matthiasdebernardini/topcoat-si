use futures::StreamExt;
use tokio::pin;
use carpa::{
    Anthropic, ContentBlock, KnownModel, MessageCreateParams, MessageParam, MessageRole, Model,
    Result, TextBlock,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Create a client using the API key from CARPA_API_KEY or ANTHROPIC_API_KEY
    let client = Anthropic::try_new(None)?;

    // Example 1: Non-streaming request built from MessageParam and MessageCreateParams::new
    println!("EXAMPLE 1: Non-streaming request with MessageCreateParams::new");
    println!("--------------------------------------------------------------");

    let message = MessageParam::new(
        "Hello, I'm a human. Can you tell me about yourself?".into(),
        MessageRole::User,
    );
    let system_prompt = "You are Claude, an AI assistant made by Anthropic.".to_string();
    let params = MessageCreateParams::new(
        1000, // max tokens
        vec![message],
        Model::Known(KnownModel::ClaudeSonnet55),
    )
    .with_system(system_prompt);

    let response = client.send(&params).await?;

    println!("Response ID: {}", response.id);
    if let Some(content) = response.content.first() {
        match content {
            ContentBlock::Text(TextBlock { text, .. }) => {
                println!("Response: {text}");
            }
            _ => println!("Received non-text content block"),
        }
    }
    println!();

    // Example 2: Non-streaming request from a prompt string
    println!("EXAMPLE 2: Non-streaming request with MessageCreateParams::simple");
    println!("-----------------------------------------------------------------");

    let params = MessageCreateParams::simple(
        "Hello, I'm a human. Can you tell me about yourself?",
        KnownModel::ClaudeSonnet55,
    )
    .with_system("You are Claude, an AI assistant made by Anthropic.");

    let response = client.send(&params).await?;

    println!("Response ID: {}", response.id);
    if let Some(content) = response.content.first() {
        match content {
            ContentBlock::Text(TextBlock { text, .. }) => {
                println!("Response: {text}");
            }
            _ => println!("Received non-text content block"),
        }
    }
    println!();

    // Example 3: Streaming request from a prompt string
    println!("EXAMPLE 3: Streaming request with MessageCreateParams::simple_streaming");
    println!("-----------------------------------------------------------------------");

    let params =
        MessageCreateParams::simple_streaming("Tell me a short joke.", KnownModel::ClaudeSonnet55);

    let stream = client.stream(&params).await?;

    // Pin the stream so it can be polled
    pin!(stream);

    println!("Streaming response:");
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                // In a real application, you would handle different event types appropriately
                println!("Received event: {event:?}");
            }
            Err(e) => {
                eprintln!("Error: {e}");
            }
        }
    }

    Ok(())
}
