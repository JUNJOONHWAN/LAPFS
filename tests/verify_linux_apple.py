#!/usr/bin/env python3
"""Independent Mac validation of retained Linux FUSE output and original hashes."""
from pathlib import Path
import sys,json,hashlib,plistlib,subprocess
root=Path(__file__).resolve().parents[1];out=Path(sys.argv[1]).resolve()
r=json.loads((out/'result.json').read_text());image=out/'native.dmg'
assert r['status']=='passed' and hashlib.sha256(image.read_bytes()).hexdigest()==r['image_sha256']
e=plistlib.loads(subprocess.check_output(['hdiutil','attach','-readonly','-plist',str(image)]))['system-entities']
parent=next(x['dev-entry'] for x in e if x.get('content-hint')=='GUID_partition_scheme')
try:
 dev=next(x['dev-entry'] for x in e if x.get('content-hint','').startswith('EF57347C'))
 mount=Path(next(x['mount-point'] for x in e if 'mount-point' in x))
 log=subprocess.check_output(['/sbin/fsck_apfs','-n',dev],stderr=subprocess.STDOUT).decode();(out/'apple-fsck.log').write_text(log)
 assert 'appears to be OK' in log and not any(x in log.lower() for x in ['error','warning','corrupt','overallocation','invalid'])
 expected=json.loads((root/'fixtures/expected.json').read_text())['files']
 for name,v in {**expected,**r['expected']}.items():
  assert (mount/name).stat().st_size==v['bytes'] and hashlib.sha256((mount/name).read_bytes()).hexdigest()==v['sha256'],name
 assert not (mount/'remove.txt').exists() and not (mount/'temp.txt').exists()
finally:subprocess.run(['hdiutil','detach',parent],check=True)
r.update(apple_fsck='clean',native_file_hashes='original104_and_all_changed_match')
(out/'result-native-verified.json').write_text(json.dumps(r,indent=2)+'\n');print('Apple fsck and all file hashes passed')
