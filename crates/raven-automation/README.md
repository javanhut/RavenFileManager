# raven-automation

Rules-based automation engine for RavenFileManager.

Watches directories and automatically triggers actions when files match configured conditions.

## Components

- **AutomationEngine** - Manages rules, watches directories, evaluates conditions, triggers actions
- **Conditions** - Extension match, name pattern (regex), size range, age threshold
- **Actions** - Move, copy, rename, tag, notify
- **Scheduler** - Periodic rule evaluation
- **Config** - Loads rules from TOML files in `~/.config/raven/rules/`

## Rule Format

Rules are defined as TOML files:

```toml
id = "sort-downloads"
name = "Sort Downloads"
enabled = true

[[conditions]]
type = "extension"
extensions = ["jpg", "png", "gif"]

[[actions]]
type = "move"
destination = "~/Pictures/Downloads"
```

## Tests

68 tests covering conditions, actions, engine lifecycle, and rule evaluation.
