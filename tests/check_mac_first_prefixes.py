#!/usr/bin/env python3
"""Mac-native, no-recovery check of every interrupted LAPFS apply prefix."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile


def run(*args):
    return subprocess.run(list(map(str, args)), capture_output=True, text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('prefix_dir', type=Path)
    parser.add_argument('--fixture-expected', type=Path)
    args = parser.parse_args()
    root = args.prefix_dir.resolve()
    manifest = json.loads((root / 'manifest.json').read_text())
    expected = json.loads(args.fixture_expected.read_text())['files'] if args.fixture_expected else {}
    rows = []
    for item in manifest['rows']:
        with tempfile.TemporaryDirectory(prefix='lapfs-mac-first-') as td:
            image = Path(td) / 'prefix.dmg'
            with gzip.open(root / item['image'], 'rb') as src, image.open('wb') as dst:
                shutil.copyfileobj(src, dst)
            attach = subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-plist', str(image)],
                                    capture_output=True)
            row = {'step': item['step'], 'attach_rc': attach.returncode}
            if attach.returncode == 0:
                entities = plistlib.loads(attach.stdout)['system-entities']
                parent = next(e['dev-entry'] for e in entities
                              if e.get('content-hint') == 'GUID_partition_scheme')
                try:
                    container = next(e['dev-entry'] for e in entities
                                     if e.get('content-hint', '').startswith('EF57347C'))
                    fsck = run('/sbin/fsck_apfs', '-n', container)
                    log = fsck.stdout + fsck.stderr
                    (root / f"{item['step']:04d}-fsck.log").write_text(log)
                    row['fsck_rc'] = fsck.returncode
                    row['fsck_clean'] = (fsck.returncode == 0 and
                                         'appears to be OK' in log and
                                         not any(s in log.lower() for s in
                                                 ('error:', 'warning:', 'overallocation', 'underallocation')))
                    mount = next((Path(e['mount-point']) for e in entities
                                  if e.get('mount-point')), None)
                    if mount:
                        data = (mount / manifest['file'].lstrip('/')).read_bytes()
                        digest = hashlib.sha256(data).hexdigest()
                        row['file_sha256'] = digest
                        row['file_atomic'] = digest in (manifest['old_sha256'],
                                                        manifest['new_sha256'])
                        row['fixture_hashes_ok'] = all(
                            (mount / name).is_file() and
                            hashlib.sha256((mount / name).read_bytes()).hexdigest() == spec['sha256']
                            for name, spec in expected.items())
                    else:
                        row.update(file_atomic=False, fixture_hashes_ok=False)
                finally:
                    detached = run('hdiutil', 'detach', parent)
                    row['detach_rc'] = detached.returncode
            else:
                row['attach_error'] = attach.stderr.decode(errors='replace')[-500:]
            row['passed'] = all((row.get('fsck_clean'), row.get('file_atomic'),
                                 row.get('fixture_hashes_ok'), row.get('detach_rc') == 0))
            rows.append(row)
            print(json.dumps(row), flush=True)
    report = {'status': 'passed' if rows and all(r['passed'] for r in rows) else 'failed',
              'checked': len(rows), 'failed_steps': [r['step'] for r in rows if not r['passed']],
              'rows': rows}
    (root / 'mac-results.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({'status': report['status'], 'checked': len(rows),
                      'failed_steps': report['failed_steps']}))
    raise SystemExit(0 if report['status'] == 'passed' else 1)


if __name__ == '__main__':
    main()
