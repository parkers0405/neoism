//! The single member capability table used by runtime dispatch and static preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Receiver {
    Array,
    String,
    Builtin(&'static str),
}

pub(crate) fn array_method(key: &str) -> bool {
    matches!(
        key,
        "map"
            | "filter"
            | "flatMap"
            | "reduce"
            | "slice"
            | "find"
            | "findIndex"
            | "includes"
            | "join"
            | "some"
            | "every"
            | "concat"
            | "flat"
            | "indexOf"
            | "push"
    )
}
pub(crate) fn string_method(key: &str) -> bool {
    matches!(
        key,
        "slice"
            | "includes"
            | "startsWith"
            | "endsWith"
            | "split"
            | "trim"
            | "toLowerCase"
            | "toUpperCase"
            | "repeat"
            | "replace"
            | "indexOf"
    )
}
pub(crate) fn builtin_member(base: &str, key: &str) -> bool {
    match base {
        "JSON" => matches!(key, "parse" | "stringify"),
        "Object" => matches!(key, "keys" | "values" | "entries" | "fromEntries"),
        "console" => matches!(key, "log" | "warn" | "error" | "info"),
        "Promise" => matches!(key, "all" | "allSettled"),
        "Number" => matches!(key, "isFinite" | "isInteger"),
        "Math" => matches!(
            key,
            "abs" | "floor" | "ceil" | "round" | "trunc" | "sqrt" | "pow" | "min" | "max"
        ),
        _ => false,
    }
}
pub(crate) fn allowed(receiver: Receiver, key: &str) -> bool {
    match receiver {
        Receiver::Array => {
            key == "length" || key.parse::<usize>().is_ok() || array_method(key)
        }
        Receiver::String => {
            key == "length" || key.parse::<usize>().is_ok() || string_method(key)
        }
        Receiver::Builtin(base) => builtin_member(base, key),
    }
}
