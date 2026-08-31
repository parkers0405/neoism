# Patch UI Sandbox: Round Eight

This directory is deliberately inert. Its files exist only for testing patch,
diff, syntax highlighting, file-card interfaces, and repeated updates.

- No workspace manifest references this directory.
- No production code imports these files.
- The directory can be removed after UI testing.
- The HTML, JavaScript, and CSS fixtures were replaced by TypeScript and SCSS.
- The Rust fixture now includes trait formatting and iterator output.

## Sample states

- Added line
- Changed text from a follow-up patch
- `inline code` and **formatted text**

| State | Expected accent |
| --- | --- |
| Removed | Red |
| Changed | Amber |
| Added | Green |

```text
This fenced block tests nested syntax inside a Markdown patch.
line two: ordinary content
line three: generic syntax Result&lt;T, E&gt;
```
