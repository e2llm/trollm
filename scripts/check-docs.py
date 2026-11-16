#!/usr/bin/env python3
"""Check maintained Markdown for control-byte corruption and joined prose limits."""
from pathlib import Path
import re
import sys

root = Path(__file__).resolve().parents[1]
paths = set(root.glob('*.md'))
for directory in ('docs', 'examples/run-342', 'ci/public-mirror/overlays'):
    paths.update((root / directory).rglob('*.md'))
paths.update((root / 'apps').glob('*/README.md'))
joined = re.compile(r'\b(?:at|to|than|most|existing|whose|remains|mode)\d{2,}')
errors = []
for path in sorted(paths):
    text = path.read_text(encoding='utf-8')
    for line, value in enumerate(text.split('\n'), 1):
        if any(ord(c) < 32 and c not in '\t\r' for c in value):
            errors.append(f'{path.relative_to(root)}:{line}: unexpected control byte')
    prose = re.sub(r'^\s*(`{3,}|~{3,})[^\n]*\n.*?^\s*\1\s*$', '', text, flags=re.M | re.S)
    prose = re.sub(r'(`+).*?\1', '', prose)
    for match in joined.finditer(prose):
        errors.append(f'{path.relative_to(root)}: joined prose limit: {match.group()}')
if errors:
    print('\n'.join(errors), file=sys.stderr)
    raise SystemExit(1)
print(f'Markdown hygiene checked: {len(paths)} files')
