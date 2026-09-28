#!/usr/bin/env python3
"""Disposable real Linux FUSE workload; retains output for independent Apple QA."""
from pathlib import Path
import argparse,tempfile,json
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary',default='target/release/lapfs')
parser.add_argument('--output',default='evidence')
args=parser.parse_args();root=Path(__file__).resolve().parents[1]
out=Path(args.output).resolve();out.mkdir(parents=True,exist_ok=True)

from pathlib import Path
import subprocess,json,hashlib,os,gzip,time,errno,shutil
r=Path(tempfile.mkdtemp(prefix='lapfs-fuse-',dir=out));b=Path(args.binary).resolve();image=r/'native.dmg';shutil.copyfile(root/'fixtures/block-test.dmg.gz',r/'fixture.gz')
with gzip.open(r/'fixture.gz','rb') as src,image.open('xb') as dst:shutil.copyfileobj(src,dst)
mp=r/'mount';mp.mkdir();log=(r/'fuse.log').open('wb')
p=subprocess.Popen([str(b),'mount-rw',str(image),'20480',str(mp),str(r/'session')],stdout=log,stderr=log)
checks=[];expected={}
def digest(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def expect_error(fn,errors):
 try:fn()
 except OSError as e:assert e.errno in errors,e
 else:raise AssertionError('Operation unexpectedly succeeded')
try:
 for _ in range(200):
  if os.path.ismount(mp):break
  assert p.poll() is None,'Mount failed'
  time.sleep(.05)
 assert os.path.ismount(mp)
 checks.append('real_rw_fuse_mount')
 payload=bytes(range(251))*50000+b'END-unaligned'
 start=time.monotonic()
 fd=os.open(mp/'buffered.bin',os.O_CREAT|os.O_RDWR,0o600)
 try:
  for off in range(0,len(payload),131071):
   chunk=payload[off:off+131071];assert os.write(fd,chunk)==len(chunk)
  assert os.fstat(fd).st_size==len(payload)
  other=os.open(mp/'buffered.bin',os.O_RDWR)
  try:
   assert os.pread(other,97,17)==payload[17:114]
   assert os.pwrite(other,b'RANDOM-OVERWRITE',4091)==16
   payload=payload[:4091]+b'RANDOM-OVERWRITE'+payload[4107:]
   assert os.pread(fd,32,4080)==payload[4080:4112]
   expect_error(lambda:os.unlink(mp/'buffered.bin'),[errno.EBUSY])
  finally:os.close(other)
  os.fsync(fd)
 finally:os.close(fd)
 elapsed=time.monotonic()-start
 assert digest(mp/'buffered.bin')==hashlib.sha256(payload).hexdigest()
 expected['buffered.bin']={'bytes':len(payload),'sha256':hashlib.sha256(payload).hexdigest()}
 checks+=['large_copy_bounded_queue','read_your_writes_across_handles','unaligned_random_overwrite','fsync_and_close','open_unlink_rejected']
 (mp/'자료 rw').mkdir();(mp/'자료 rw/한글 space.txt').write_bytes(b'hello buffered world')
 with (mp/'자료 rw/한글 space.txt').open('r+b') as f:f.truncate(7)
 with (mp/'자료 rw/한글 space.txt').open('ab') as f:f.write(b' plus')
 assert (mp/'자료 rw/한글 space.txt').read_bytes()==b'hello b plus'
 expected['자료 rw/한글 space.txt']={'bytes':12,'sha256':hashlib.sha256(b'hello b plus').hexdigest()}
 (mp/'old.txt').write_bytes(b'old');(mp/'temp.txt').write_bytes(b'new atomically replaced')
 os.replace(mp/'temp.txt',mp/'old.txt');assert (mp/'old.txt').read_bytes()==b'new atomically replaced'
 expected['old.txt']={'bytes':23,'sha256':hashlib.sha256(b'new atomically replaced').hexdigest()}
 (mp/'remove.txt').write_bytes(b'delete');(mp/'remove.txt').unlink();assert not (mp/'remove.txt').exists()
 checks+=['mkdir_unicode','truncate_and_unaligned_append','atomic_replace','unlink']
 (mp/'temporary-empty').mkdir()
 (mp/'temporary-empty/nested').mkdir()
 expect_error(lambda:os.rmdir(mp/'temporary-empty'),[errno.ENOTEMPTY])
 os.rmdir(mp/'temporary-empty/nested')
 os.rmdir(mp/'temporary-empty')
 assert not (mp/'temporary-empty').exists()
 checks+=['rmdir_nonempty_guard','rmdir_nested_empty']
 copydata=b'ordinary cp command'*10000;(r/'copy-input.bin').write_bytes(copydata)
 subprocess.run(['cp',str(r/'copy-input.bin'),str(mp/'copy-command.bin')],check=True)
 assert (mp/'copy-command.bin').read_bytes()==copydata
 expected['copy-command.bin']={'bytes':len(copydata),'sha256':hashlib.sha256(copydata).hexdigest()}
 checks.append('ordinary_cp_command')
 fd=os.open(mp/'buffered.bin',os.O_WRONLY)
 try:expect_error(lambda:os.pwrite(fd,b'hole',len(payload)+1000),[errno.EOPNOTSUPP])
 finally:os.close(fd)
 expect_error(lambda:os.chmod(mp/'old.txt',0o644),[errno.EOPNOTSUPP])
 assert digest(mp/'buffered.bin')==expected['buffered.bin']['sha256'];checks+=['unsupported_sparse_and_chmod_explicit_error','mount_still_usable_after_rejection']
 # Existing original files, links and large sparse ranges remain readable.
 for name,v in json.loads((root/'fixtures/expected.json').read_text())['files'].items():assert digest(mp/name)==v['sha256'],name
 assert os.readlink(mp/'link')=='payload.bin'
 with (mp/'sparse.bin').open('rb') as f:f.seek(4*1024**3);assert f.read(11)==b'\0'*7+b'LAST'
 checks+=['original_104_files_preserved','symlink_read','sparse_read_above_4GiB']
 assert not subprocess.run([str(b),'inspect',str(image),'20480'],capture_output=True).returncode==0
 checks.append('exclusive_raw_reader_denied')
finally:
 if os.path.ismount(mp):subprocess.run(['fusermount3','-u',str(mp)],check=True)
 p.wait(timeout=30)
assert p.returncode==0, 'Unmount failed'
# Kill the actual FUSE daemon after write(2) returned, before close/fsync.
crashlog=(r/'crash-fuse.log').open('wb')
q=subprocess.Popen([str(b),'mount-rw',str(image),'20480',str(mp),str(r/'crash-session')],stdout=crashlog,stderr=crashlog)
for _ in range(200):
 if os.path.ismount(mp):break
 assert q.poll() is None
 time.sleep(.05)
assert os.path.ismount(mp)
fd=os.open(mp/'crash-ack.bin',os.O_CREAT|os.O_RDWR,0o600)
crash_data=bytes(range(251))*2000+b'ACK END'
assert os.write(fd,crash_data)==len(crash_data)
assert os.pread(fd,37,7)==crash_data[7:44]
q.kill();q.wait(timeout=10)
try:os.close(fd)
except OSError:pass
subprocess.run(['fusermount3','-u',str(mp)],check=True)
assert subprocess.run([str(b),'inspect',str(image),'20480'],capture_output=True).returncode!=0
recovery=subprocess.run([str(b),'mount-recover',str(r/'crash-session')],capture_output=True,text=True)
(r/'crash-recovery.log').write_text(recovery.stdout+recovery.stderr);assert recovery.returncode==0,recovery.stderr
actual=json.loads(subprocess.check_output([str(b),'digest',str(image),'20480','/crash-ack.bin']))
assert actual['sha256']==hashlib.sha256(crash_data).hexdigest()
expected['crash-ack.bin']={'bytes':len(crash_data),'sha256':actual['sha256']}
checks+=['actual_fuse_sigkill_after_ack','pending_session_blocks_raw_read','recovery_preserves_acknowledged_writes']
envelope=json.loads((r/'session/session.json').read_text());session=json.loads(envelope['payload']);assert session['closed'] and not session['queue']
spool=sum(x.stat().st_size for x in (r/'session').rglob('*') if x.is_file())
logdir=Path.home()/'.local/state/lapfs'
events=[]
for logfile in logdir.glob('errors.jsonl*'):
 for line in logfile.read_text().splitlines():
  event=json.loads(line)
  if event['pid'] in [p.pid,q.pid]:events.append(event)
assert any(e['operation']=='write' and e['errno']==errno.EOPNOTSUPP for e in events),events
assert any(e['operation']=='setattr' and e['errno']==errno.EOPNOTSUPP for e in events),events
assert any(e['operation']=='unlink' and e['errno']==errno.EBUSY for e in events),events
assert not any(e['operation']=='lookup' and e['errno']==errno.ENOENT for e in events),events
(r/'error-events.json').write_text(json.dumps(events,indent=2))
checks+=['separate_operation_errno_logs','expected_lookup_misses_not_logged']
result={'status':'passed' ,'checks':checks,'expected':expected,'copy_bytes':len(payload),'copy_seconds':elapsed,'spool_bytes_after_close':spool,'committed_batches':session['sequence'],'binary_sha256':digest(b),'physical_usb_writes':0,'image_sha256':digest(image)}
(r/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))

print('Evidence:',r)
