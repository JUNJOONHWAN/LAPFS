#!/usr/bin/env python3
import argparse,gzip,hashlib,json,os,shutil,subprocess,tempfile,time
from pathlib import Path
root=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary',default='target/release/lapfs')
parser.add_argument('--output',default='evidence')
args=parser.parse_args()
binary=Path(args.binary).resolve()
output=Path(args.output).resolve();output.mkdir(parents=True,exist_ok=True)
out=Path(tempfile.mkdtemp(prefix='lapfs-rsync-',dir=output))
image=out/'image.dmg'
with gzip.open(root/'fixtures/block-test.dmg.gz','rb') as src,image.open('wb') as dst: shutil.copyfileobj(src,dst)
mount=out/'mount'; mount.mkdir(exist_ok=True)
source=out/'source'; source.mkdir(exist_ok=True)
setsrc=source/'syncset'; setsrc.mkdir(exist_ok=True)
(setsrc/'payload.txt').write_bytes(b'rsync-v1'*123)
(setsrc/'nested').mkdir(exist_ok=True)
(setsrc/'nested'/'other.bin').write_bytes(bytes(range(256))*4)
large=bytes(range(251))*50000+b'END-unaligned'
(setsrc/'large.bin').write_bytes(large)
(setsrc/'link').unlink(missing_ok=True)
(setsrc/'link').symlink_to('payload.txt')
os.chmod(setsrc/'payload.txt',0o640)
os.utime(setsrc/'payload.txt',ns=(946684800123456789,946684800123456789))
log=(out/'fuse.log').open('wb')
proc=subprocess.Popen([str(binary),'mount-rw',str(image),'20480',str(mount),str(out/'session')],stdout=log,stderr=log)
results=[]
try:
 for _ in range(300):
  if os.path.ismount(mount): break
  if proc.poll() is not None: raise RuntimeError('FUSE exited')
  time.sleep(.1)
 assert os.path.ismount(mount)
 def sync(label,delete=False):
  cmd=['rsync','-a','--itemize-changes']+(['--delete'] if delete else [])+[str(setsrc)+'/',str(mount/'syncset')+'/']
  r=subprocess.run(cmd,capture_output=True,text=True)
  results.append({'label':label,'rc':r.returncode,'stdout':r.stdout,'stderr':r.stderr})
  assert r.returncode==0,(label,r.stderr)
 sync('initial')
 assert (mount/'syncset/link').is_symlink()
 assert os.readlink(mount/'syncset/link')=='payload.txt'
 assert (mount/'syncset/payload.txt').stat().st_mode & 0o777 == 0o640
 assert (mount/'syncset/payload.txt').stat().st_mtime_ns==946684800123456789
 sync('repeat')
 (setsrc/'payload.txt').write_bytes(b'rsync-v2'*150)
 os.utime(setsrc/'payload.txt',ns=(946684800987654321,946684800987654321))
 sync('replace')
 (setsrc/'link').unlink()
 (setsrc/'link').symlink_to('nested/other.bin')
 sync('link-change')
 assert os.readlink(mount/'syncset/link')=='nested/other.bin'
 (mount/'syncset/orphan.txt').write_bytes(b'orphan')
 (mount/'syncset/orphan-link').symlink_to('orphan.txt')
 (mount/'syncset/orphan-dir').mkdir()
 (mount/'syncset/orphan-dir'/'child.txt').write_bytes(b'child')
 sync('delete',True)
 assert not (mount/'syncset/orphan.txt').exists()
 assert not (mount/'syncset/orphan-link').exists()
 assert not (mount/'syncset/orphan-dir').exists()
 assert (mount/'syncset/payload.txt').read_bytes()==(setsrc/'payload.txt').read_bytes()
 result={'status':'passed','binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'runs':results,'payload_sha256':hashlib.sha256((setsrc/'payload.txt').read_bytes()).hexdigest(),'mtime_ns':(mount/'syncset/payload.txt').stat().st_mtime_ns,'mode':oct((mount/'syncset/payload.txt').stat().st_mode & 0o777),'link':os.readlink(mount/'syncset/link'),'nested_sha256':hashlib.sha256((mount/'syncset/nested/other.bin').read_bytes()).hexdigest(),'large_sha256':hashlib.sha256(large).hexdigest(),'large_bytes':len(large)}
 (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
 print(json.dumps(result,indent=2),flush=True)
finally:
 for i in range(20):
  r=subprocess.run(['fusermount3','-u',str(mount)],capture_output=True,text=True)
  if r.returncode==0: break
  time.sleep(.5)
 else: print('UNMOUNT FAILED',r.stderr,flush=True)
 proc.wait(timeout=60)
 log.close()

print('Evidence:',out)
