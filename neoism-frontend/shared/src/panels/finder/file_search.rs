// File-mode helpers: exact-match scoring + sorting.

use super::types::{FileResult, Result_};

pub(super) fn exact_file_results<'a, I>(query: &str, paths: I) -> Vec<(i32, Result_)>
where
    I: IntoIterator<Item = &'a str>,
{
    let query = query.trim();
    if query.is_empty() {
        return paths
            .into_iter()
            .take(500)
            .map(|path| {
                (
                    0,
                    Result_::File(FileResult {
                        path: path.to_string(),
                    }),
                )
            })
            .collect();
    }

    let smart_case = query.chars().any(|c| c.is_ascii_uppercase());
    let mut scored: Vec<(i32, Result_)> = paths
        .into_iter()
        .filter_map(|path| {
            let score = exact_file_match_score(query, path, smart_case)?;
            Some((
                score,
                Result_::File(FileResult {
                    path: path.to_string(),
                }),
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path().cmp(b.1.path())));
    scored.truncate(500);
    scored
}

pub(super) fn exact_file_match_score(
    query: &str,
    path: &str,
    smart_case: bool,
) -> Option<i32> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if exact_eq(path, query, smart_case) {
        return Some(12_000 - path.len() as i32);
    }
    if exact_eq(file_name, query, smart_case) {
        return Some(11_000 - path.len() as i32);
    }
    if let Some(pos) = exact_find(file_name, query, smart_case) {
        return Some(10_000 - pos as i32 * 10 - path.len() as i32);
    }
    if let Some(pos) = exact_find(path, query, smart_case) {
        return Some(9_000 - pos as i32 - path.len() as i32);
    }
    None
}

#[inline]
fn exact_eq(haystack: &str, needle: &str, smart_case: bool) -> bool {
    if smart_case {
        haystack == needle
    } else {
        haystack.eq_ignore_ascii_case(needle)
    }
}

#[inline]
fn exact_find(haystack: &str, needle: &str, smart_case: bool) -> Option<usize> {
    if smart_case {
        haystack.find(needle)
    } else {
        ascii_case_insensitive_find(haystack, needle)
    }
}

fn ascii_case_insensitive_find(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}
