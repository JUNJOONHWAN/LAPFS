import os,sys,time,json,gzip,shutil,subprocess,hashlib,tempfile,argparse
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--old',required=True);p.add_argument('--binary',default='target/release/lapfs');p.add_argument('--output',default='evidence');args=p.parse_args()
output=Path(args.output).resolve();output.mkdir(parents=True,exist_ok=True)
work=Path(tempfile.mkdtemp(prefix='throughput-',dir=output))
old=Path(args.old).resolve()
new=Path(args.binary).resolve()
shutil.copy2(new,work/'new-lapfs');new=work/'new-lapfs'
rows=[]
for run,(label,binary) in enumerate([('old',old),('new',new),('new',new),('old',old)]):
 d=work/f'{run}-{label}';d.mkdir(); mp=d/'mount';mp.mkdir();image=d/'native.dmg'
 with gzip.open(root/'fixtures/block-test.dmg.gz','rb') as a,image.open('wb') as b:shutil.copyfileobj(a,b)
 log=(d/'mount.log').open('wb');p=subprocess.Popen([str(binary),'mount-rw',str(image),'20480',str(mp),str(d/'session')],stdout=log,stderr=log)
 try:
  for i in range(300):
   if os.path.ismount(mp):break
   assert p.poll() is None,(d/'mount.log').read_text();time.sleep(.05)
  assert os.path.ismount(mp)
  for chunk in [10240,1048576]:
   data=bytes(range(256))*(chunk//256); size=16*1048576;dest=mp/f'bench-{chunk}.bin';h=hashlib.sha256(); t=time.monotonic();fd=os.open(dest,os.O_WRONLY|os.O_CREAT,0o600)
   for off in range(0,size,chunk):
    b=data[:min(chunk,size-off)];assert os.write(fd,b)==len(b);h.update(b)
   os.fsync(fd);os.close(fd);elapsed=time.monotonic()-t
   assert hashlib.sha256(dest.read_bytes()).hexdigest()==h.hexdigest()
   row={'run':run,'label':label,'chunk':chunk,'bytes':size,'seconds':elapsed,'MiB_s':16/elapsed,'sha256':h.hexdigest(),'image':str(image),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()};rows.append(row);print(json.dumps(row),flush=True)
 finally:
  if os.path.ismount(mp):subprocess.run(['fusermount3','-u',str(mp)],check=True)
  p.wait(timeout=60)
 assert p.returncode==0
 (work/'results.json').write_text(json.dumps(rows,indent=2))
print('EVIDENCE='+str(work),flush=True)
