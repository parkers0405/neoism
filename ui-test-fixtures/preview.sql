CREATE TABLE patch_rows (
    line_number INTEGER PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('added', 'changed', 'removed')),
    content TEXT NOT NULL
);

INSERT INTO patch_rows (line_number, state, content)
VALUES
    (1, 'added', 'new fixture line'),
    (2, 'changed', 'modified fixture line'),
    (3, 'removed', 'deleted fixture line');

UPDATE patch_rows
SET content = 'modified again in round ten'
WHERE state = 'changed';

SELECT state, COUNT(*) AS row_count
FROM patch_rows
GROUP BY state
ORDER BY state;

BEGIN TRANSACTION;
DELETE FROM patch_rows WHERE state = 'removed';
ROLLBACK;