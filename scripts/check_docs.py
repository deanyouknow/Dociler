"""Check handoff Markdown using Python's standard library.

Checks inline relative file links, heading order, and README coverage. Does not
fetch external links, check fragment anchors, or parse reference-style links.
"""

from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    files = sorted(ROOT.glob("*.md")) + sorted((ROOT / "docs").glob("*.md"))
    errors = []
    links = 0
    readme_targets = set()
    for file in files:
        previous = 0
        titles = 0
        fence = None
        for number, line in enumerate(file.read_text(encoding="utf-8").splitlines(), 1):
            marker = re.match(r"(`{3,}|~{3,})", line.lstrip())
            if marker:
                delimiter = marker.group(1)
                if fence is None:
                    fence = delimiter
                elif delimiter[0] == fence[0] and len(delimiter) >= len(fence):
                    fence = None
                continue
            if fence is not None:
                continue
            heading = re.match(r"^(#{1,6})\s+", line)
            if heading:
                level = len(heading.group(1))
                titles += level == 1
                if previous and level > previous + 1:
                    errors.append(f"{file.relative_to(ROOT)}:{number}: heading skips a level")
                previous = level
            for link in re.findall(r"\[[^\]]*\]\(([^)]+)\)", line):
                parsed = urlsplit(link.strip("<>"))
                if parsed.scheme or parsed.netloc or not parsed.path:
                    continue
                target = (file.parent / unquote(parsed.path)).resolve()
                links += 1
                if not target.exists():
                    errors.append(f"{file.relative_to(ROOT)}:{number}: missing link target {link}")
                if file == ROOT / "README.md":
                    readme_targets.add(target)
        if titles != 1:
            errors.append(f"{file.relative_to(ROOT)}: expected one H1, found {titles}")
        if fence is not None:
            errors.append(f"{file.relative_to(ROOT)}: unclosed fenced block")

    required = [ROOT / name for name in ("AGENTS.md", "CHECKPOINT.md", "IMPLEMENTATION_PLAN.md", "LICENSE")]
    required += sorted((ROOT / "docs").glob("*.md"))
    for file in required:
        if not file.exists():
            errors.append(f"missing required file: {file.relative_to(ROOT)}")
        if file not in readme_targets:
            errors.append(f"README must link to {file.relative_to(ROOT)}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Passed: {len(files)} Markdown files, {links} relative links, README coverage.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

