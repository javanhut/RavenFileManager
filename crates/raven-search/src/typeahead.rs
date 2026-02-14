use raven_core::entry::FileEntry;

/// Instant in-memory substring and fuzzy filter for `FileEntry` lists.
///
/// This is designed for interactive typeahead filtering in the UI, where
/// we already have a loaded directory listing and need to narrow it down
/// based on what the user is typing.
#[derive(Debug, Clone)]
pub struct TypeaheadFilter {
    query: String,
    /// Lowercased query cached for repeated use.
    query_lower: String,
}

/// The kind of match that was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// All query characters appear in order (but not necessarily contiguous).
    Fuzzy,
    /// The query is a contiguous substring of the name.
    Substring,
    /// The name starts with the query (strongest match).
    Prefix,
}

/// A scored match result, pairing a `FileEntry` with its match quality.
#[derive(Debug, Clone)]
pub struct TypeaheadMatch {
    pub entry: FileEntry,
    pub kind: MatchKind,
    /// A score for ranking; higher is better.
    pub score: u32,
}

impl TypeaheadFilter {
    /// Create a new filter with the given query string.
    pub fn new(query: impl Into<String>) -> Self {
        let query = query.into();
        let query_lower = query.to_lowercase();
        Self { query, query_lower }
    }

    /// Returns the original query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns true if the query is empty (matches everything).
    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// Filter entries, returning only those whose names match the query.
    ///
    /// Results are sorted by match quality: prefix > substring > fuzzy.
    pub fn filter(&self, entries: &[FileEntry]) -> Vec<FileEntry> {
        if self.query.is_empty() {
            return entries.to_vec();
        }

        let mut matches: Vec<TypeaheadMatch> = entries
            .iter()
            .filter_map(|entry| self.score_entry(entry))
            .collect();

        // Sort by match kind (best first), then by score descending, then name.
        matches.sort_by(|a, b| {
            b.kind
                .cmp(&a.kind)
                .then(b.score.cmp(&a.score))
                .then(a.entry.name.cmp(&b.entry.name))
        });

        matches.into_iter().map(|m| m.entry).collect()
    }

    /// Filter entries and return scored matches for richer UI display.
    pub fn filter_scored(&self, entries: &[FileEntry]) -> Vec<TypeaheadMatch> {
        if self.query.is_empty() {
            return entries
                .iter()
                .map(|entry| TypeaheadMatch {
                    entry: entry.clone(),
                    kind: MatchKind::Prefix,
                    score: 0,
                })
                .collect();
        }

        let mut matches: Vec<TypeaheadMatch> = entries
            .iter()
            .filter_map(|entry| self.score_entry(entry))
            .collect();

        matches.sort_by(|a, b| {
            b.kind
                .cmp(&a.kind)
                .then(b.score.cmp(&a.score))
                .then(a.entry.name.cmp(&b.entry.name))
        });

        matches
    }

    /// Check if a single entry matches and compute its score.
    fn score_entry(&self, entry: &FileEntry) -> Option<TypeaheadMatch> {
        let name_lower = entry.name.to_lowercase();

        // Check prefix match first (strongest).
        if name_lower.starts_with(&self.query_lower) {
            return Some(TypeaheadMatch {
                entry: entry.clone(),
                kind: MatchKind::Prefix,
                // Shorter names score higher for prefix matches (more specific).
                score: 1000u32.saturating_sub(entry.name.len() as u32),
            });
        }

        // Check substring match.
        if name_lower.contains(&self.query_lower) {
            // Earlier position of the match is better.
            let pos = name_lower.find(&self.query_lower).unwrap_or(0) as u32;
            return Some(TypeaheadMatch {
                entry: entry.clone(),
                kind: MatchKind::Substring,
                score: 500u32.saturating_sub(pos),
            });
        }

        // Check fuzzy match: all query characters appear in order.
        if Self::fuzzy_match(&self.query_lower, &name_lower) {
            // Score based on how compact the match is.
            let compactness = Self::fuzzy_compactness(&self.query_lower, &name_lower);
            return Some(TypeaheadMatch {
                entry: entry.clone(),
                kind: MatchKind::Fuzzy,
                score: compactness,
            });
        }

        None
    }

    /// Returns true if all characters of `query` appear in `name` in order.
    fn fuzzy_match(query: &str, name: &str) -> bool {
        let mut name_chars = name.chars();
        for qc in query.chars() {
            let found = name_chars.any(|nc| nc == qc);
            if !found {
                return false;
            }
        }
        true
    }

    /// Compute a compactness score for fuzzy matching.
    ///
    /// A tighter grouping of matched characters yields a higher score.
    fn fuzzy_compactness(query: &str, name: &str) -> u32 {
        let name_chars: Vec<char> = name.chars().collect();
        let mut first_pos = None;
        let mut last_pos = 0;
        let mut name_idx = 0;

        for qc in query.chars() {
            while name_idx < name_chars.len() {
                if name_chars[name_idx] == qc {
                    if first_pos.is_none() {
                        first_pos = Some(name_idx);
                    }
                    last_pos = name_idx;
                    name_idx += 1;
                    break;
                }
                name_idx += 1;
            }
        }

        let first = first_pos.unwrap_or(0);
        let span = last_pos - first + 1;
        // Higher score for tighter matches, penalize by start position.
        let tightness = 200u32.saturating_sub(span as u32);
        let position_bonus = 50u32.saturating_sub(first as u32);
        tightness + position_bonus
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
    use raven_core::path::RavenPath;

    fn make_entry(name: &str) -> FileEntry {
        FileEntry::new(
            name.to_string(),
            RavenPath::local(format!("/test/{}", name)),
            EntryKind::File,
            EntryMetadata::default(),
        )
    }

    #[test]
    fn empty_query_returns_all() {
        let filter = TypeaheadFilter::new("");
        let entries = vec![make_entry("foo.txt"), make_entry("bar.rs")];
        let result = filter.filter(&entries);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn substring_match() {
        let filter = TypeaheadFilter::new("oo");
        let entries = vec![
            make_entry("foo.txt"),
            make_entry("bar.rs"),
            make_entry("boo.md"),
        ];
        let result = filter.filter(&entries);
        assert_eq!(result.len(), 2);
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"foo.txt"));
        assert!(names.contains(&"boo.md"));
    }

    #[test]
    fn case_insensitive_match() {
        let filter = TypeaheadFilter::new("FOO");
        let entries = vec![make_entry("foo.txt"), make_entry("FooBar.rs")];
        let result = filter.filter(&entries);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn prefix_ranked_higher() {
        let filter = TypeaheadFilter::new("lib");
        let entries = vec![
            make_entry("stdlib.rs"),
            make_entry("lib.rs"),
        ];
        let result = filter.filter(&entries);
        assert_eq!(result[0].name, "lib.rs");
    }

    #[test]
    fn fuzzy_match_works() {
        let filter = TypeaheadFilter::new("lbr");
        let entries = vec![
            make_entry("lib.rs"),
            make_entry("foo.txt"),
        ];
        let result = filter.filter(&entries);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "lib.rs");
    }

    #[test]
    fn no_match_returns_empty() {
        let filter = TypeaheadFilter::new("zzz");
        let entries = vec![make_entry("foo.txt"), make_entry("bar.rs")];
        let result = filter.filter(&entries);
        assert!(result.is_empty());
    }
}
