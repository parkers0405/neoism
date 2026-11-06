//! Explicit opt-in probe: schema and one-row decoding only; never print records.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("database path required");
    let db = turso::Builder::new_local(&path).read_only(true)
        .experimental_index_method(true).build().await?;
    let conn = db.connect()?;
    conn.execute("PRAGMA query_only=1", ()).await?;
    let mut schema = conn.query("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*'", ()).await?;
    let mut count = 0;
    while let Some(row) = schema.next().await? {
        let name: String = row.get(0)?;
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        let mut rows = conn.query(format!("SELECT * FROM {quoted} LIMIT 1"), ()).await?;
        if let Some(row) = rows.next().await? {
            let _ = row.get_value(0)?;
        }
        count += 1;
    }
    println!("Read-only schema and limited browse succeeded for {count} tables; no records printed");
    Ok(())
}