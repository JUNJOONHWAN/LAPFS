#!/usr/bin/env python3
"""Disposable FUSE O_SYNC/O_DSYNC ACKs and two-handle append ordering."""
from pathlib import Path
import argparse,subprocess,tempfile,gzip,shutil,os,time,json,hashlib,errno
p=argparse.ArgumentParser();p.add_argument('--binary',required=True);p.add_argument('--output',default='evidence');a=p.parse_args()
root=Path(__file__).resolve().parents[1];out=Path(a.output).resolve();out.mkdir(exist_ok=True);work=Path(tempfile.mkdtemp(prefix='pipeline-fuse-',dir=out));binary=Path(a.binary).resolve();rows=[]
def unmount(mp):
 for _ in range(20):
  result=subprocess.run(['fusermount3','-u',str(mp)],capture_output=True,text=True)
  if result.returncode==0:return
  time.sleep(.25)
 raise RuntimeError(result.stderr)
for label,flag in [('sync',os.O_SYNC),('dsync',os.O_DSYNC)]:
 d=work/label;d.mkdir();mp=d/'mount';mp.mkdir();image=d/'native.dmg'
 with gzip.open(root/'fixtures/block-test.dmg.gz','rb') as src,image.open('wb') as dst:shutil.copyfileobj(src,dst)
 log=(d/'mount.log').open('wb');proc=subprocess.Popen([str(binary),'mount-rw',str(image),'20480',str(mp),str(d/'session'),'--grouped-writes'],stdout=log,stderr=log)
 fd=None
 try:
  for _ in range(200):
   if os.path.ismount(mp):break
   assert proc.poll() is None;time.sleep(.05)
  assert os.path.ismount(mp)
  path=mp/'append.bin';one=os.open(path,os.O_CREAT|os.O_WRONLY|os.O_APPEND,0o600);two=os.open(path,os.O_WRONLY|os.O_APPEND);expected=bytearray()
  try:
   for i in range(37):
    data=bytes([i])*32761;assert os.write(one if i%2 else two,data)==len(data);expected.extend(data)
   os.fsync(one)
  finally:os.close(one);os.close(two)
  assert path.read_bytes()==expected
  dfd=os.open(mp,os.O_RDONLY|os.O_DIRECTORY)
  try:os.fsync(dfd)
  finally:os.close(dfd)
  fd=os.open(mp/'sync-ack.bin',os.O_WRONLY|os.O_CREAT|flag,0o600);payload=bytes(range(251))*1000+b'SYNC-ACK'
  assert os.write(fd,payload)==len(payload)
  # No close or explicit fsync before killing the actual daemon.
  proc.kill();assert proc.wait(timeout=30)!=0
  try:os.close(fd)
  except OSError as e:assert e.errno in [errno.ENOTCONN,errno.EIO]
  fd=None
  unmount(mp)
  rows.append({'name':label,'image':str(image),'expected':{'append.bin':{'bytes':len(expected),'sha256':hashlib.sha256(expected).hexdigest()},'sync-ack.bin':{'bytes':len(payload),'sha256':hashlib.sha256(payload).hexdigest()}},'dgx_recovery_before_native':False})
 finally:
  if fd is not None:os.close(fd)
  if os.path.ismount(mp):unmount(mp)
  if proc.poll() is None:proc.wait(timeout=30)
(work/'results.json').write_text(json.dumps(rows,indent=2));print(work,flush=True)
