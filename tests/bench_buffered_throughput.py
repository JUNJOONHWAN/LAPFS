import os,sys,time,json,gzip,shutil,subprocess,hashlib,tempfile,argparse,random
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--random-seed',type=int);p.add_argument('--fixture',default=str(root/'fixtures/block-test.dmg.gz'));p.add_argument('--offset',type=int,default=20480);p.add_argument('--size-mib',type=int,default=16);p.add_argument('--old',required=True);p.add_argument('--binary',default='target/release/lapfs');p.add_argument('--output',default='evidence');args=p.parse_args()
output=Path(args.output).resolve();output.mkdir(parents=True,exist_ok=True)
work=Path(tempfile.mkdtemp(prefix='throughput-',dir=output))
old=Path(args.old).resolve()
new=Path(args.binary).resolve()
shutil.copy2(new,work/'new-lapfs');new=work/'new-lapfs'
rows=[]
def proc_io(pid):
 return {k:int(v) for k,v in (line.split(":") for line in Path(f"/proc/{pid}/io").read_text().splitlines())}
for run,(label,binary) in enumerate([('old',old),('new',new),('new',new),('old',old)]):
 d=work/f'{run}-{label}';d.mkdir(); mp=d/'mount';mp.mkdir();image=d/'native.dmg'
 with gzip.open(args.fixture,'rb') as a,image.open('wb') as b:shutil.copyfileobj(a,b)
 log=(d/'mount.log').open('wb');p=subprocess.Popen([str(binary),'mount-rw',str(image),str(args.offset),str(mp),str(d/'session')],stdout=log,stderr=log)
 try:
  for i in range(300):
   if os.path.ismount(mp):break
   assert p.poll() is None,(d/'mount.log').read_text();time.sleep(.05)
  assert os.path.ismount(mp)
  for chunk in [10240,1048576]:
   data=random.Random(args.random_seed).randbytes(chunk) if args.random_seed is not None else bytes(range(256))*(chunk//256); size=args.size_mib*1048576;dest=mp/f'bench-{chunk}.bin';h=hashlib.sha256(); before=proc_io(p.pid); t=time.monotonic();fd=os.open(dest,os.O_WRONLY|os.O_CREAT,0o600)
   try:
    for off in range(0,size,chunk):
     b=data[:min(chunk,size-off)];assert os.write(fd,b)==len(b);h.update(b)
    os.fsync(fd)
   finally:os.close(fd)
   elapsed=time.monotonic()-t; after=proc_io(p.pid)
   read_started=time.monotonic(); read_hash=hashlib.sha256(); read_bytes=0
   with dest.open('rb',buffering=0) as f:
    while block:=f.read(1048576):read_hash.update(block);read_bytes+=len(block)
   read_seconds=time.monotonic()-read_started
   assert read_bytes==size and read_hash.hexdigest()==h.hexdigest()
   row={'read_seconds':read_seconds,'read_MB_s':read_bytes/1e6/read_seconds,'random_seed':args.random_seed,'write_amplification':(after['write_bytes']-before['write_bytes'])/size,'daemon_io_delta':{k:after[k]-before[k] for k in before},'run':run,'label':label,'chunk':chunk,'bytes':size,'seconds':elapsed,'MiB_s':args.size_mib/elapsed,'sha256':h.hexdigest(),'image':str(image),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()};rows.append(row);print(json.dumps(row),flush=True)
 finally:
  if os.path.ismount(mp):subprocess.run(['fusermount3','-u',str(mp)],check=True)
  p.wait(timeout=60)
 assert p.returncode==0
 (work/'results.json').write_text(json.dumps(rows,indent=2))
print('EVIDENCE='+str(work),flush=True)
