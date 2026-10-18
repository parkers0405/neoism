def normalize_text(text):
    """Normalize line endings, trailing ASCII whitespace and blank runs."""
    text = text.replace('\r\n', '\n').replace('\r', '\n')
    lines = []
    for raw_line in text.split('\n'):
        line = raw_line.rstrip(' \t')
        if line:
            lines.append(line)
        elif lines and lines[-1] != '':
            lines.append('')
    if lines and lines[-1] == '':
        lines.pop()
    return '\n'.join(lines) + '\n' if lines else ''
