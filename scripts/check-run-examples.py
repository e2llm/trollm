#!/usr/bin/env python3
"""Verify the published v342 evidence, unchanged bundle and recorded metrics."""
import hashlib
import io
import json
import math
from pathlib import Path
import statistics
import sys
import tarfile
import zipfile

from run_metrics import summarize


def context_stats(calls):
    values = sorted(call['tokensIn'] for call in calls)
    return {'calls': len(values), 'minimumInputTokens': values[0],
            'medianInputTokens': statistics.median(values),
            'p95InputTokensNearestRank': values[math.ceil(.95 * len(values)) - 1],
            'maximumInputTokens': values[-1],
            'meanInputTokens': sum(values) / len(values)}


def verify(root):
    expected = {}
    for line in (root / 'SHA256SUMS').read_text().splitlines():
        digest, name = line.split('  ', 1)
        path = Path(name)
        if path.is_absolute() or '..' in path.parts or name in expected:
            raise ValueError('unsafe or duplicate checksum path')
        expected[name] = digest
    actual = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in root.rglob('*') if p.is_file() and p.name != 'SHA256SUMS'}
    if actual != expected:
        raise ValueError('published example inventory/checksums differ')
    bundle = (root / 'amazon-cart-v342.tar').read_bytes()
    with tarfile.open(fileobj=io.BytesIO(bundle)) as archive:
        files = {}
        for member in archive:
            path = Path(member.name)
            if (not member.isfile() or path.is_absolute() or '..' in path.parts or
                    member.name in files or member.size > 1024 * 1024):
                raise ValueError('unsupported bundle member')
            files[member.name] = archive.extractfile(member).read()
            if files[member.name] != (root / 'flow' / member.name).read_bytes():
                raise ValueError('readable flow differs from historical bundle')
        manifest = json.loads(files['manifest.json'])
        for member in manifest['files']:
            if hashlib.sha256(files[member['path']]).hexdigest() != member['sha256']:
                raise ValueError('historical bundle member hash mismatch')
    for device in ('a04e', 's21'):
        directory = root / device
        identity = json.loads((directory / 'identities.json').read_text())
        if hashlib.sha256(bundle).hexdigest() != identity['bundleSha256']:
            raise ValueError('run bundle identity mismatch')
        result = json.loads((directory / 'result.json').read_text())
        run = result['runId']
        with zipfile.ZipFile(directory / 'run-evidence.zip') as archive:
            main = archive.read(run + '/events.jsonl').decode('utf-8')
            proxy = archive.read(run + '/proxy.events.jsonl').decode('utf-8')
        metrics = summarize([io.StringIO(main), io.StringIO(proxy)])
        recorded = json.loads((directory / 'metrics.json').read_text())
        if metrics != recorded:
            raise ValueError(device + ': recorded metrics differ from journals')
        events = [json.loads(line) for line in main.splitlines() if line.strip()]
        starts = [e for e in events if e['type'] == 'run_started']
        ends = [e for e in events if e['type'] == 'run_ended']
        if len(starts) != 1 or len(ends) != 1 or ends[0]['outcome'] != result['status']:
            raise ValueError(device + ': terminal outcome differs')
        if (ends[0]['ts'] - starts[0]['ts']) / 1000 != result['elapsedSeconds']:
            raise ValueError(device + ': journal duration differs')
        calls = [e for e in events if e['type'] == 'model_call']
        contexts = json.loads((directory / 'context-stats.json').read_text())
        for slot in ('all', 'coder', 'reasoner'):
            selected = calls if slot == 'all' else [c for c in calls if c['slot'] == slot]
            if context_stats(selected) != contexts[slot]:
                raise ValueError(device + ': context statistics differ')
    print('v342 examples: checksums, flow bytes, journal outcomes, metrics and context statistics match')


if __name__ == '__main__':
    try:
        verify(Path(__file__).resolve().parents[1] / 'examples' / 'run-342')
    except (OSError, ValueError, KeyError, zipfile.BadZipFile, tarfile.TarError) as error:
        print('run example verification failed: ' + str(error), file=sys.stderr)
        raise SystemExit(1)
