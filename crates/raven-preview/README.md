# raven-preview

File preview generation for RavenFileManager.

Generates previews dispatched by file type: syntax-highlighted text, image metadata, and directory summaries.

## Providers

| Provider | Description |
|----------|-------------|
| `TextPreviewProvider` | Syntax highlighting via `syntect` |
| `ImagePreviewProvider` | Image dimensions and metadata via `image` |
| `DirectoryPreviewProvider` | Item count and total size |
| `PreviewRouter` | Selects provider based on file extension/MIME type |

Preview data is returned as `PreviewData` variants (Text, Image, Directory, Unsupported).

## Tests

13 tests covering preview generation and routing.
