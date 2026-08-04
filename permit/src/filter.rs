/// Filter items by patterns using cascading match: exact -> prefix -> substring.
///
/// - Empty patterns returns all items unchanged.
/// - Otherwise tries each level in order; returns the first non-empty result set.
pub fn filter_by_patterns<T, F>(items: Vec<T>, patterns: &[String], key: F) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    if patterns.is_empty() {
        return items;
    }

    type Matcher = fn(&str, &str) -> bool;
    let levels: [Matcher; 3] = [
        |k: &str, p: &str| k == p,
        |k: &str, p: &str| k.starts_with(p),
        |k: &str, p: &str| k.contains(p),
    ];

    // Two passes over the winning level: one to find it, one to keep its items. The old form built
    // a `Vec<usize>` of matching indices and rebuilt the result with `indices.contains(i)`, which is
    // a linear scan of that vec per item -- quadratic in the match count, to answer a question the
    // predicate itself answers directly.
    for level in levels {
        let matches = |item: &T| patterns.iter().any(|p| level(key(item), p));
        if items.iter().any(&matches) {
            return items.into_iter().filter(|item| matches(item)).collect();
        }
    }

    vec![]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_patterns_returns_all() {
        let items = vec!["foo", "bar", "baz"];
        assert_eq!(filter_by_patterns(items, &[], |s| s), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn exact_match_wins() {
        let items = vec!["Bash(git status:*)", "Bash(git push:*)", "Bash(git:*)"];
        let result = filter_by_patterns(items, &p(&["Bash(git status:*)"]), |s| s);
        assert_eq!(result, vec!["Bash(git status:*)"]);
    }

    #[test]
    fn prefix_match_when_no_exact() {
        let items = vec!["Bash(git status:*)", "Bash(git push:*)", "WebSearch", "Edit(**)"];
        let result = filter_by_patterns(items, &p(&["Bash"]), |s| s);
        assert_eq!(result, vec!["Bash(git status:*)", "Bash(git push:*)"]);
    }

    #[test]
    fn substring_match_when_no_prefix() {
        let items = vec!["Bash(git status:*)", "Bash(git push:*)", "WebSearch"];
        let result = filter_by_patterns(items, &p(&["git"]), |s| s);
        assert_eq!(result, vec!["Bash(git status:*)", "Bash(git push:*)"]);
    }

    #[test]
    fn multiple_patterns_same_level() {
        let items = vec!["Bash(git status:*)", "Bash(cargo build:*)", "WebSearch"];
        let result = filter_by_patterns(items, &p(&["Bash(git status:*)", "Bash(cargo build:*)"]), |s| s);
        assert_eq!(result, vec!["Bash(git status:*)", "Bash(cargo build:*)"]);
    }

    #[test]
    fn no_match_returns_empty() {
        let items = vec!["foo", "bar"];
        let result = filter_by_patterns(items, &p(&["zzz"]), |s| s);
        assert!(result.is_empty());
    }
}
