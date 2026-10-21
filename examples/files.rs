use topcoat_si::{
    Anthropic, ContentBlock, DocumentBlock, KnownModel, MessageCreateParams, MessageParam,
    MessageParamContent, MessageRole, TextBlock,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Anthropic::try_new(None)?;

    let text = "Topcoat SI is a Rust SDK for the Anthropic Messages API.\n";
    let file = client
        .upload_file("topcoat-si.txt", "text/plain", text.as_bytes(), None)
        .await?;
    println!("uploaded {} ({} bytes)", file.id, file.size_bytes);

    let content = vec![
        ContentBlock::Document(DocumentBlock::from_file_id(&file.id)),
        ContentBlock::Text(TextBlock::new("Summarize this document in one sentence.")),
    ];
    let params = MessageCreateParams::new(
        512,
        vec![MessageParam::new(
            MessageParamContent::Array(content),
            MessageRole::User,
        )],
        KnownModel::ClaudeHaiku55.into(),
    );
    let answer = client.send(&params).await;

    // Delete the file whether or not the request succeeded.
    let deleted = client.delete_file(&file.id).await?;
    println!("deleted {}", deleted.id);

    for block in &answer?.content {
        if let ContentBlock::Text(text) = block {
            println!("{}", text.text);
        }
    }
    Ok(())
}
