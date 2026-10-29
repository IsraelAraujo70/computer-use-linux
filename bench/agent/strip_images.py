#!/usr/bin/env python3
"""Replace base64 image payloads in agent JSONL logs with a size placeholder (keeps logs committable)."""
import re
import sys
from pathlib import Path

PATTERN = re.compile(r'"(data|image_url|url)":\s*"(data:image/[a-z]+;base64,)?[A-Za-z0-9+/=]{2000,}"')


def strip(path: Path) -> None:
    text = path.read_text(errors="replace")
    new = PATTERN.sub(lambda m: f'"{m.group(1)}": "<image {len(m.group(0))} bytes stripped>"', text)
    if new != text:
        path.write_text(new)


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        strip(Path(arg))
