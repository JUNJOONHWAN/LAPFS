from pathlib import Path
import os,time,json,gzip,shutil,subprocess,hashlib,tempfile,random,argparse
root=Path(__file__).resolve().parents[1];parser=argparse.ArgumentParser();parser.add_argument('--fixture',required=True);parser.add_argument('--binary',required=True);args=parser.parse_args();d=Path(tempfile.mkdtemp(prefix='reuse-',dir=root/'evidence'));mp=d/'mount';mp.mkdir();image=d/'native.dmg';binary=Path(args.binary).resolve()
with gzip.open(args.fixture,'rb') as a,image.open('wb') as b:shutil.copyfileobj(a,b)
log=(d/'mount.log').open('wb');p=subprocess.Popen([str(binary),'mount-rw',str(image),'20480',str(mp),str(d/'session'),'--grouped-writes'],stdout=log,stderr=log);rows=[]
def io():return {k:int(v) for k,v in (line.split(':') for line in Path(f'/proc/{p.pid}/io').read_text().splitlines())}
try:
 for _ in range(300):
  if os.path.ismount(mp):break
  assert p.poll() is None;time.sleep(.05)
 assert os.path.ismount(mp)
 for attempt in range(8):
  data=random.Random(817+attempt).randbytes(1048576);size=256*1048576;h=hashlib.sha256();before=io();start=time.monotonic();fd=os.open(mp/'reuse.bin',os.O_CREAT|os.O_WRONLY|os.O_TRUNC,0o600)
  try:
   for _ in range(256):assert os.write(fd,data)==len(data);h.update(data)
   os.fsync(fd)
  finally:os.close(fd)
  elapsed=time.monotonic()-start;after=io()
  with (mp/'reuse.bin').open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==h.hexdigest()
  row={'pass':attempt,'bytes':size,'seconds':elapsed,'MB_s':size/1e6/elapsed,'write_amplification':(after['write_bytes']-before['write_bytes'])/size,'io_delta':{k:after[k]-before[k] for k in before},'sha256':h.hexdigest()};rows.append(row);print(json.dumps(row),flush=True)
finally:
 if os.path.ismount(mp):subprocess.run(['fusermount3','-u',str(mp)],check=True)
 p.wait(timeout=60)
assert p.returncode==0
(d/'results.json').write_text(json.dumps({'image':str(image),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'rows':rows},indent=2));print(d,flush=True)
