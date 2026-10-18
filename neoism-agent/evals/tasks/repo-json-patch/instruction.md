# Transactional JSON patch

Private synthetic Python stdlib repository task, not a public benchmark or upstream issue. Repair `patch.apply_patch(document, operations)` using the pointer/operations/patch modules. Inputs are JSON values and must never be mutated or aliased by the returned document.

Implement add, remove, replace, test, copy, move in list order on a private document copy. Success: `{"ok":true,"document":updated}`. Failure: `{"ok":false,"document":original,"index":zero_based_operation_index,"error":code}`; discard ALL previous changes. If operations itself is not a list, index is null and code is `invalid`. Empty list succeeds for any JSON document.

Each operation is an object with known string `op` and string `path`. add/replace/test require a `value` key (null allowed); copy/move require string `from`. Extra fields are ignored. Malformed operation or pointer syntax yields `invalid`. Missing target/parent/source, wrong container type, illegal array index, or move into a proper descendant yields `path`. A test of an existing unequal value yields `test`. Validate each operation when reached: structural/pointer validation precedes path access; do not prevalidate later operations. First failing operation wins.

Pointers are empty (whole document) or start with `/`. Split on `/` first, then decode each token: `~1` -> slash, `~0` -> tilde, and every other tilde escape is invalid. Empty object keys are legal. No URI fragment syntax. Objects use exact string keys. Arrays accept only `0` or nonzero decimal digits without leading zero; existing access requires index < length; add permits index == length and `-` to append, ONLY in its final destination token. No automatic parent creation.

add to object sets/overwrites; add to array INSERTS, not replaces. remove/replace/test require existing target. Root add/replace replace the document, root remove produces null. copy deep-copies its existing source then adds to destination. move reads/removes source then adds using POST-removal indices; same decoded source/destination is a no-op but source must exist; a proper descendant destination is forbidden. Root source can only move to root. copy/move validate both pointer strings before resolving either.

JSON equality for test is structural, object key order irrelevant, array order significant; booleans differ from numbers, while numeric 1 and 1.0 are equal. Null differs from all other types. No dependencies, files, or network access are needed.
