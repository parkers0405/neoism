fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&neoism_cloud_host::canonical_openapi())?
    );
    Ok(())
}
