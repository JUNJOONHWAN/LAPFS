#!/usr/bin/env python3
"""Disposable multi-level APFS catalog/extent tree stress. Never a raw device."""
from pathlib import Path
import argparse,subprocess,json,os,time,hashlib,shutil,tempfile
p=argparse.ArgumentParser();p.add_argument('--image',required=True);p.add_argument('--expected',required=True);p.add_argument('--binary',default='target/release/lapfs');p.add_argument('--output',default='evidence');a=p.parse_args()
source=Path(a.image).resolve();assert source.is_file() and source.suffix=='.dmg'
w=Path(tempfile.mkdtemp(prefix='catalog-cow-',dir=Path(a.output).resolve()));image=w/'large.dmg';shutil.copyfile(source,image)
mp=w/'mount';mp.mkdir();log=(w/'fuse.log').open('wb');b=Path(a.binary).resolve()
binary_sha=hashlib.file_digest(b.open('rb'),'sha256').hexdigest()
q=subprocess.Popen([str(b),'mount-rw',str(image),'20480',str(mp),str(w/'session')],stdout=log,stderr=log)
expected=json.loads(Path(a.expected).read_text())['files'];changed={};start=time.monotonic();checks=[]
try:
 for _ in range(300):
  if os.path.ismount(mp):break
  assert q.poll() is None,'mount failed'
  time.sleep(.05)
 assert os.path.ismount(mp)
 # Long names force repeated catalog leaf splits at the growing inode range;
 # hashed directory keys cross existing leaves and change ancestor pivots.
 for i in range(600):
  name=f'new-{i:05d}-'+('split-boundary-'*9)+'.bin';data=hashlib.sha256(str(i).encode()).digest()*129
  (mp/name).write_bytes(data)
  if i%50==0:print('created',i,flush=True)
  changed[name]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
 checks.append('600_long_name_creates_and_writes');print('created 600',flush=True)
 for i,name in enumerate(list(changed)):
  if i%3==0:
   dest=f'renamed-{i:05d}-'+('separator-'*12)+'.bin';os.rename(mp/name,mp/dest);changed[dest]=changed.pop(name)
 checks.append('200_cross_leaf_renames')
 for i,name in enumerate(list(changed)):
  if i%2==0:
   (mp/name).unlink();del changed[name]
 checks.append('300_deletions_with_leaf_removal')
 # Interleaved random writes, overlaps and sequential writes from two handles.
 names=list(changed)[:2];fds=[os.open(mp/n,os.O_RDWR) for n in names];contents=[bytearray((mp/n).read_bytes()) for n in names]
 try:
  for i in range(40):
   slot=i%2;off=(i*71)%3000;data=bytes([i])*113
   assert os.pwrite(fds[slot],data,off)==len(data);contents[slot][off:off+len(data)]=data
  for fd in fds:os.fsync(fd)
 finally:
  for fd in fds:os.close(fd)
 for n,data in zip(names,contents):changed[n]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
 checks.append('interleaved_random_overlapping_writes')
 for index,(name,v) in enumerate({**expected,**changed}.items()):
  if index%500==0:print('verified',index,flush=True)
  assert (mp/name).stat().st_size==v['bytes'],name
  assert hashlib.sha256((mp/name).read_bytes()).hexdigest()==v['sha256'],name
 checks.append(f'all_{len(expected)}_original_files_preserved')
finally:
 if os.path.ismount(mp):subprocess.run(['fusermount3','-u',str(mp)],check=True)
 q.wait(timeout=30)
assert q.returncode==0
result={'status':'passed','checks':checks,'expected':changed,'original_files':len(expected),'seconds':time.monotonic()-start,'image_sha256':hashlib.file_digest(image.open('rb'),'sha256').hexdigest(),'binary_sha256':binary_sha,'physical_usb_writes':0}
(w/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps({k:v for k,v in result.items() if k!='expected'},indent=2));print(w)
