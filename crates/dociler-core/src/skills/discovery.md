# Document Discovery and Workspace Cataloging

## Workspace Boundaries
Document discovery operations locate, catalog, and inventory supported documents strictly within the current workspace root directory.

## Privacy and Ignore Rules
Strictly respect `.gitignore` rules, `.docilerignore`, and built-in privacy filters. Never inspect, index, or expose:
- Hidden files and dot-directories (e.g., `.git`, `.config`, `.ssh`)
- Build artifacts, compiler caches, dependency directories (`target/`, `node_modules/`, `vendor/`)
- Secret keys, authentication tokens, credentials, or private configuration files
- Files outside the canonical workspace boundary; reject escaping symlinks

## Cataloging Standards
Present document listings accurately with their relative paths, detected document formats, and structural section headings when requested. Maintain discovery as a read-only inspection; never create, rename, or delete files during cataloging.
