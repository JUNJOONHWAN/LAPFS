#!/usr/bin/env python3
"""Independent Apple fsck/hash check of every buffered-crash result image."""
from pathlib import Path
import json,hashlib,plistlib,subprocess,sys
p=Path(sys.argv[1]).resolve();rows=json.loads((p/'results.json').read_text())
for row in rows:
 e=plistlib.loads(subprocess.check_output(['hdiutil','attach','-readonly','-plist',row['image']]))['system-entities']
 parent=next(x['dev-entry'] for x in e if x.get('content-hint')=='GUID_partition_scheme')
 try:
  dev=next(x['dev-entry'] for x in e if x.get('content-hint','').startswith('EF57347C'));mp=Path(next(x['mount-point'] for x in e if 'mount-point' in x))
  log=subprocess.check_output(['/sbin/fsck_apfs','-n',dev],stderr=subprocess.STDOUT).decode();(p/(row['point']+'-fsck.log')).write_text(log)
  assert 'appears to be OK' in log and not any(x in log.lower() for x in ['warning','corrupt','error','overallocation','invalid'])
  actual=(mp/'crash.txt').read_bytes();assert len(actual)==row['bytes'] and hashlib.sha256(actual).hexdigest()==row['sha256']
  row.update(apple_fsck='clean',native_hash='matched')
 finally:subprocess.run(['hdiutil','detach',parent],check=True,stdout=subprocess.DEVNULL)
(p/'results-native-verified.json').write_text(json.dumps(rows,indent=2));print('Native Apple fsck + SHA passed all',len(rows),'crash cases')
