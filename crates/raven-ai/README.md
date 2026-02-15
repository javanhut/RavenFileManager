# raven-ai

AI-powered features for RavenFileManager. All features are **local-only** -- no LLM, no network calls.

## Modules

### nl_search - Natural Language Search

Keyword-based parser that converts natural language queries into structured `FilterSpec` objects.

```rust
let parsed = parse_nl_query("large images from last week");
// parsed.file_types == [Images]
// parsed.min_size == Some(104857600)  // 100MB
// parsed.modified_after == Some(7 days ago)
```

Supported keywords:
- **Types**: images, videos, audio, documents, archives, directories
- **Sizes**: large (>100MB), small (<1MB), empty, `bigger than 5mb`, `under 500kb`
- **Dates**: today, yesterday, recent (<7d), old (>1yr), `last week`, `last month`, `this year`
- **Languages**: python (.py), rust (.rs), javascript (.js/.jsx/.mjs), typescript, java, go, etc.

### tags - Tag Types

Defines `TagRule`, `TagMatchRule`, `TagDatabase`, and `ManualTag` types. Includes 9 default smart tags:

| Tag | Rule |
|-----|------|
| Large Files | Size > 100MB |
| Images | Extensions: jpg, png, gif, bmp, svg, webp, ... |
| Documents | Extensions: pdf, doc, docx, odt, txt, md, ... |
| Code | Extensions: rs, py, js, ts, java, go, c, cpp, ... |
| Archives | Extensions: zip, tar, gz, 7z, rar, ... |
| Videos | Extensions: mp4, mkv, avi, mov, ... |
| Audio | Extensions: mp3, flac, ogg, wav, ... |
| Old Files | Modified > 1 year ago |
| Recent | Modified < 7 days ago |

### tag_engine - Tag Engine

Matches files against tag rules and manages manual tag assignments.

```rust
let engine = TagEngine::new(PathBuf::from("~/.config/raven/tags.toml"));
let tags = engine.tags_for_entry(&file_entry);  // ["Images", "Recent"]
let counts = engine.tag_counts(&entries);        // [("Images", 15), ("Code", 8), ...]
```

### duplicates - Duplicate Detection

Two-phase algorithm for finding duplicate files:

1. **Walk phase**: Groups files by size, discards unique sizes
2. **Hash phase**: Computes blake3 hash for same-size files, groups by hash

Features: cancellable, progress streaming, configurable min size, recursive/non-recursive.

### organize - Smart Organization

Analyzes directory contents and suggests organization into subfolders.

Strategies:
- **ByExtensionType**: Groups files by category (Images/, Documents/, Code/, etc.)
- **ByDate**: Groups files by modification year (requires 10+ files spanning 2+ years)
- **ByProject**: Detects project directories (Cargo.toml, package.json, .git) and skips them

## Tests

76 tests covering all modules.
