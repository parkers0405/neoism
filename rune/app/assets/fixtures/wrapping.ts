    const message = renderMessage({ title: "Greedy word wrapping keeps quoted words together and uses the last boundary that fits the viewport, without preferring a comma halfway across the row.", body: "All continuations keep the same source indentation." });
    const result = client.items.filter(item => item.enabled && item.visible).map(item => formatItem(item.name, item.description, item.metadata)).join(", ");

// Comments and strings use the same language-independent wrapping boundaries. Narrowing the pane changes the row count, not bracket alignment or source text.
    const unicode = "日本語の文章も折り返します。 Combining é and emoji 👩‍💻 remain whole when a long line reaches the right edge of the viewport.";

const identifier_without_break_opportunities_01234567890123456789012345678901234567890123456789012345678901234567890123456789 = true;