fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let document = neoism_cloud_runtime::canonical_openapi();
    let mut output = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &document)?;
    writeln!(output)?;
    Ok(())
}
