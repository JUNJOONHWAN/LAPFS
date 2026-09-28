#!/usr/bin/env python3
"""Independent Mac validation of multi-level catalog output and all original hashes."""
import time
def detach(device):
 for attempt in range(10):
  result=subprocess.run(['hdiutil','detach',device],capture_output=True,text=True)
  if result.returncode==0:return
  if 'busy' not in result.stderr.lower() and '사용 중' not in result.stderr:break
  time.sleep(1)
 raise RuntimeError(result.stderr)

from pathlib import Path
import sys,json,hashlib,plistlib,subprocess
root=Path(__file__).resolve().parents[1];out=Path(sys.argv[1]).resolve()
r=json.loads((out/'result.json').read_text());image=out/'large.dmg'
assert r['status']=='passed' and hashlib.sha256(image.read_bytes()).hexdigest()==r['image_sha256']
e=plistlib.loads(subprocess.check_output(['hdiutil','attach','-readonly','-nobrowse','-plist',str(image)]))['system-entities']
parent=next(x['dev-entry'] for x in e if x.get('content-hint')=='GUID_partition_scheme')
try:
 dev=next(x['dev-entry'] for x in e if x.get('content-hint','').startswith('EF57347C'))
 mount=Path(next(x['mount-point'] for x in e if 'mount-point' in x))
 log=subprocess.check_output(['/sbin/fsck_apfs','-n',dev],stderr=subprocess.STDOUT).decode();(out/'apple-fsck.log').write_text(log)
 assert 'appears to be OK' in log and not any(x in log.lower() for x in ['error','warning','corrupt','overallocation','invalid'])
 expected=json.loads(Path(sys.argv[2]).read_text())['files']
 for name,v in {**expected,**r['expected']}.items():
  assert (mount/name).stat().st_size==v['bytes'] and hashlib.sha256((mount/name).read_bytes()).hexdigest()==v['sha256'],name
 assert len(expected)==5000 and len(r['expected'])==300
 # Confirm deleted and renamed old names are absent, not just surviving hashes.
 for i in range(600):
  old=f'new-{i:05d}-'+('split-boundary-'*9)+'.bin'
  if old not in r['expected']:assert not (mount/old).exists(),old
 for i in range(0,600,3):
  renamed=f'renamed-{i:05d}-'+('separator-'*12)+'.bin'
  if renamed not in r['expected']:assert not (mount/renamed).exists(),renamed
finally:detach(parent)
r.update(apple_fsck='clean',native_file_hashes='original5000_and_all_300_changed_match_and_deleted_names_absent')
(out/'result-native-verified.json').write_text(json.dumps(r,indent=2)+'\n');print('Apple fsck and all file hashes passed')
