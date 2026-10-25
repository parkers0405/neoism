// Shared deterministic payloads: producer uses official 0.6.1, consumer uses
// production 0.8.3 and event_storage. No application/runtime dependency here.
pub const PAGE_DOCUMENTS: usize = 384;

pub fn page_body(index: usize) -> String {
    let mut body = format!("fixturepage pageprobe{index:04} ");
    for word in 0..128 {
        body.push_str(&format!("lex{index:04}word{word:04} "));
    }
    body
}

pub fn event_json(compressed: bool) -> String {
    let (id, sequence, padding) = if compressed {
        ("evt_fixture_blob", 202, "codec payload 雪 ".repeat(512))
    } else {
        ("evt_fixture_text", 101, "legacy payload 雪".to_owned())
    };
    format!(
        "{{ \"id\":\"{id}\", \"type\":\"message.part.updated\", \"sequence\":{sequence}, \"properties\":{{\"sessionID\":\"ses_fixture\",\"part\":{{\"type\":\"text\",\"text\":\"{padding}\"}},\"opaque\":\"  unchanged  \"}}, \"unknown\":{{\"preserve\":true}} }}"
    )
}
