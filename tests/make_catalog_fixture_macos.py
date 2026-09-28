#!/usr/bin/env python3
"""Create a disposable 512 MiB APFS image with 5,000 files using Apple's driver."""
from pathlib import Path
import argparse,hashlib,json,plistlib,subprocess,gzip,shutil
p=argparse.ArgumentParser(description=__doc__);p.add_argument('output');a=p.parse_args()
w=Path(a.output).resolve();w.mkdir(parents=True,exist_ok=False)
image=w/'large.dmg'
subprocess.run(['hdiutil','create','-size','512m','-fs','APFS','-volname','LAPFS_COW_QA','-layout','GPTSPUD',str(image)],check=True)
e=plistlib.loads(subprocess.check_output(['hdiutil','attach','-plist',str(image)]))['system-entities']
parent=next(x['dev-entry'] for x in e if x.get('content-hint')=='GUID_partition_scheme')
mp=Path(next(x['mount-point'] for x in e if 'mount-point' in x));expected={}
try:
 for i in range(5000):
  name=f'{i:05d}-'+('catalog-boundary-'*7)+'.bin';data=hashlib.sha256(str(i).encode()).digest()*256
  (mp/name).write_bytes(data);expected[name]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
finally:subprocess.run(['hdiutil','detach',parent],check=True)
(w/'expected.json').write_text(json.dumps({'files':expected},indent=2)+'\n')
with image.open('rb') as src,gzip.open(w/'large.dmg.gz','wb',compresslevel=1) as dst:shutil.copyfileobj(src,dst)
print(w)
