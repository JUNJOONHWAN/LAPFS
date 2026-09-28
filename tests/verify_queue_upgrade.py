"""Disposable v1 durable queue -> v2 reader compatibility, no physical device I/O."""
import argparse,gzip,json,os,shutil,signal,subprocess,tempfile,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--old-durable-flag',action='store_true');p.add_argument('--old',required=True);p.add_argument('--binary',required=True);p.add_argument('--output',default='evidence');a=p.parse_args()
root=Path(__file__).resolve().parents[1];out=Path(a.output).resolve();out.mkdir(exist_ok=True,parents=True);work=Path(tempfile.mkdtemp(prefix='queue-upgrade-',dir=out));image=work/'native.dmg';mp=work/'mount';mp.mkdir();session=work/'session'
with gzip.open(root/'fixtures/block-test.dmg.gz','rb') as f,image.open('wb') as g:shutil.copyfileobj(f,g)
def unmount():
 for i in range(50):
  r=subprocess.run(['fusermount3','-u',str(mp)],capture_output=True,text=True)
  if r.returncode==0:return
  if 'busy' not in r.stderr:raise RuntimeError(r.stderr)
  time.sleep(.1)
 raise RuntimeError(r.stderr)
def start(binary,sd,flags=[]):
 log=(work/('mount-'+str(time.time_ns())+'.log')).open('wb');q=subprocess.Popen([binary,'mount-rw',str(image),'20480',str(mp),str(sd)]+flags,stdout=log,stderr=log)
 for i in range(200):
  if os.path.ismount(mp):return q
  assert q.poll() is None;time.sleep(.05)
 raise AssertionError('mount timeout')
q=start(a.old,session,['--durable-writes'] if a.old_durable_flag else []);fd=os.open(mp/'legacy.bin',os.O_WRONLY|os.O_CREAT,0o600);os.write(fd,b'legacy-durable-ack');q.kill();q.wait()
try:os.close(fd)
except OSError as e:
 assert e.errno==107
# Dead daemon only, on this disposable image. Never a production handoff.
unmount()
r=subprocess.run([a.binary,'mount-rw',str(image),'20480',str(mp),str(session),'--grouped-writes'],capture_output=True,text=True);assert r.returncode!=0 and 'policy differs' in r.stderr
q=start(a.binary,session);assert (mp/'legacy.bin').read_bytes()==b'legacy-durable-ack';unmount();assert q.wait(timeout=30)==0
j=json.loads(json.loads((session/'session.json').read_text())['payload']);assert j['write_policy']=='durable' and j['closed']
fresh=work/'grouped';q=start(a.binary,fresh);assert (mp/'legacy.bin').read_bytes()==b'legacy-durable-ack';unmount();assert q.wait(timeout=30)==0
j=json.loads(json.loads((fresh/'session.json').read_text())['payload']);assert j['write_policy']=='grouped'
result={'status':'passed','checks':['legacy_ack_recovered','explicit_policy_mismatch_refused','legacy_policy_preserved','new_mount_defaults_grouped'],'physical_usb_writes':0};(work/'result.json').write_text(json.dumps(result,indent=2));print(work);print(json.dumps(result))
