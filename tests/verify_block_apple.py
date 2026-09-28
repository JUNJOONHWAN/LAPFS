#!/usr/bin/env python3
"""Independent native Mac check of an already-retrieved disposable block QA image."""
from pathlib import Path
import hashlib,json,plistlib,subprocess,sys
out=Path(sys.argv[1]).resolve()
expected=json.loads(Path(sys.argv[2]).read_text())['files']
image=out/'disposable.dmg'
result=json.loads((out/'result.json').read_text())
assert result['status']=='passed' and result['physical_usb_writes']==0
assert hashlib.sha256(image.read_bytes()).hexdigest()==sys.argv[3], 'Transferred image differs from DGX'
def run(args):return subprocess.check_output([str(x) for x in args],stderr=subprocess.STDOUT)
entities=plistlib.loads(run(['hdiutil','attach','-readonly','-plist',image]))['system-entities']
parent=next(e['dev-entry'] for e in entities if e.get('content-hint')=='GUID_partition_scheme')
try:
 dev=next(e['dev-entry'] for e in entities if e.get('content-hint','').startswith('EF57347C'))
 mount=Path(next(e['mount-point'] for e in entities if 'mount-point' in e))
 fsck=run(['/sbin/fsck_apfs','-n',dev]).decode();(out/'block-apple-fsck.log').write_text(fsck)
 assert 'appears to be OK' in fsck and not any(x in fsck.lower() for x in ['error','warning','corrupt','overallocation','is invalid','is not valid'])
 assert hashlib.sha256((mount/'block-test.bin').read_bytes()).hexdigest()==result['import_sha256']
 for p,v in expected.items():
  assert (mount/p).stat().st_size==v['bytes'],p
  assert hashlib.sha256((mount/p).read_bytes()).hexdigest()==v['sha256'],p
 if result.get('rw_mount_sha256'):
  assert hashlib.sha256((mount/'rw-block.bin').read_bytes()).hexdigest()==result['rw_mount_sha256']
  result['native_mac_rw_mount']='sha256 matched'
 assert not (mount/'block-test-dir').exists(), 'Cancelled transaction leaked into committed volume'
 result.update(apple_fsck='clean',native_mac_original_files_verified=len(expected),native_mac_import='sha256 matched',cancelled_directory_absent=True,transferred_image_sha256=sys.argv[3])
finally:run(['hdiutil','detach',parent])
(out/'block-result-verified.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))
