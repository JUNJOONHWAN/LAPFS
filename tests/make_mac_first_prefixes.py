#!/usr/bin/env python3
"""Generate every interrupted APFS apply prefix on disposable images only.

Build LAPFS with --features fault-injection. Run the output through
check_mac_first_prefixes.py on macOS WITHOUT mount-recover or journal recovery.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile


def call(exe, *args, env=None):
    p = subprocess.run([str(exe), *map(str, args)], capture_output=True, text=True, env=env)
    if p.returncode:
        raise RuntimeError(f"{args[0]}: {p.returncode}: {p.stdout}{p.stderr}")
    return p.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--fixture-gz', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    exe = args.binary.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    base = out / 'base.dmg'
    with gzip.open(args.fixture_gz, 'rb') as src, base.open('wb') as dst:
        shutil.copyfileobj(src, dst)
    assert base.is_file() and base.stat().st_size == 134217728
    offset = 20480
    old = b'A' * 8192
    new = b'U' * 4096 + b'A' * 4096
    source = out / 'initial.bin'
    source.write_bytes(old)
    action = out / 'initial.json'
    action.write_text(json.dumps([{'op': 'put', 'source': str(source), 'path': '/crash.bin'}]))
    call(exe, 'prepare', base, offset, action, out / 'initial-journal', 64, 0)
    call(exe, 'apply', out / 'initial-journal')
    patch = out / 'patch.bin'
    patch.write_bytes(b'U' * 4096)
    rows = []
    for step in range(1, 10000):
        with tempfile.TemporaryDirectory(prefix='apply-', dir=out) as td:
            case = Path(td)
            image = case / 'image.dmg'
            shutil.copyfile(base, image)
            action = case / 'action.json'
            action.write_text(json.dumps([
                {'op': 'write_at', 'source': str(patch), 'path': '/crash.bin', 'offset': 0}
            ]))
            journal = case / 'journal'
            call(exe, 'prepare', image, offset, action, journal, 64, 0)
            operations = json.loads(call(exe, 'status', journal))['operations']
            if step > operations:
                break
            env = dict(os.environ, SPARK_APFS_KILL_AFTER_APPLY_OP=str(step))
            proc = subprocess.run([str(exe), 'apply', str(journal)],
                                  capture_output=True, text=True, env=env)
            if proc.returncode != -signal.SIGKILL:
                raise RuntimeError(f'fault point {step} not reached: {proc.returncode}: {proc.stderr}')
            compressed = out / f'{step:04d}.dmg.gz'
            with image.open('rb') as src, gzip.open(compressed, 'wb', compresslevel=1) as dst:
                shutil.copyfileobj(src, dst)
            rows.append({'step': step, 'image': compressed.name,
                         'compressed_bytes': compressed.stat().st_size})
    result = {'status': 'generated_unverified', 'operations': len(rows),
              'container_offset': offset, 'file': '/crash.bin',
              'old_sha256': hashlib.sha256(old).hexdigest(),
              'new_sha256': hashlib.sha256(new).hexdigest(), 'rows': rows}
    (out / 'manifest.json').write_text(json.dumps(result, indent=2))
    with base.open('rb') as src, gzip.open(out / 'base.dmg.gz', 'wb', compresslevel=1) as dst:
        shutil.copyfileobj(src, dst)
    base.unlink()
    shutil.rmtree(out / 'initial-journal')
    print(json.dumps({'output': str(out), 'operations': len(rows)}))


if __name__ == '__main__':
    main()
