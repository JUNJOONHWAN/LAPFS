#!/usr/bin/env python3
"""Independently validate the disposable rsync APFS image on macOS."""
from pathlib import Path
import hashlib
import json
import plistlib
import subprocess
import time
import sys

out = Path(sys.argv[1]).resolve()
root = Path(__file__).resolve().parents[1]
image = out / 'image.dmg'
expected = json.loads((root / 'fixtures/expected.json').read_text())['files']
rsync = json.loads((out / 'result.json').read_text())
assert rsync['status'] == 'passed' and len(rsync['runs']) == 5 and all(r['rc'] == 0 for r in rsync['runs'])
entities = plistlib.loads(subprocess.check_output([
    'hdiutil', 'attach', '-readonly', '-nobrowse', '-plist', str(image)
]))['system-entities']
parent = next(e['dev-entry'] for e in entities if e.get('content-hint') == 'GUID_partition_scheme')
try:
    dev = next(e['dev-entry'] for e in entities if e.get('content-hint', '').startswith('EF57347C'))
    mount = Path(next(e['mount-point'] for e in entities if 'mount-point' in e))
    fsck = subprocess.check_output(['/sbin/fsck_apfs', '-n', dev], stderr=subprocess.STDOUT).decode()
    (out / 'apple-fsck.log').write_text(fsck)
    assert 'appears to be OK' in fsck
    assert not any(s in fsck.lower() for s in ['error', 'warning', 'corrupt', 'overallocation', 'invalid'])
    for name, record in expected.items():
        p = mount / name
        assert p.stat().st_size == record['bytes'], name
        assert hashlib.sha256(p.read_bytes()).hexdigest() == record['sha256'], name
    p = mount / 'syncset/payload.txt'
    assert hashlib.sha256(p.read_bytes()).hexdigest() == rsync['payload_sha256']
    assert p.stat().st_mtime_ns == rsync['mtime_ns']
    assert p.stat().st_mode & 0o777 == 0o640
    assert (mount / 'syncset/link').is_symlink()
    assert (mount / 'syncset/link').readlink().as_posix() == rsync['link']
    assert hashlib.sha256((mount / 'syncset/nested/other.bin').read_bytes()).hexdigest() == rsync['nested_sha256']
    assert (mount / 'syncset/large.bin').stat().st_size == rsync['large_bytes']
    assert hashlib.sha256((mount / 'syncset/large.bin').read_bytes()).hexdigest() == rsync['large_sha256']
    assert not (mount / 'syncset/orphan.txt').exists()
    assert not (mount / 'syncset/orphan-link').exists()
    assert not (mount / 'syncset/orphan-dir').exists()
    assert not list((mount / 'syncset').glob('.payload.txt.*'))
    result = {
        'apple_fsck': 'clean', 'original_files_verified': len(expected),
        'rsync_file_sha256': rsync['payload_sha256'],
        'mtime_ns': p.stat().st_mtime_ns, 'mode': oct(p.stat().st_mode & 0o777),
        'image_sha256': hashlib.sha256(image.read_bytes()).hexdigest(),
    }
    (out / 'verification.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))
finally:
    for attempt in range(10):
        detached = subprocess.run(['hdiutil', 'detach', parent], text=True, capture_output=True)
        if detached.returncode == 0: break
        time.sleep(1)
    else:
        raise RuntimeError(detached.stderr)
