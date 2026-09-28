#!/usr/bin/env python3
"""Root-only QA on a NEW loop device backed by a disposable APFS fixture.
Never accepts a physical device argument. Creates only task-owned loop/state paths.
Retains a modified image and logs for independent Apple fsck; never certifies USB power loss.
"""
from pathlib import Path
import gzip,hashlib,json,os,shutil,subprocess,sys,tempfile,time
from block_qa_support import find_apfs_partition
BASE=Path(__file__).resolve().parents[1]
BIN=BASE/'bin/lapfs'
FIXTURE=BASE/'fixtures/block-test.dmg.gz'
assert os.geteuid()==0,'Run sudo python3 scripts/verify-block-device.py'
assert FIXTURE.is_file() and not FIXTURE.is_symlink() and FIXTURE.stat().st_size<=256*1024*1024
OUT=Path(tempfile.mkdtemp(prefix='lapfs-block-qa-',dir='/var/tmp'))
image=OUT/'disposable.dmg'
with gzip.open(FIXTURE,'rb') as src,image.open('xb') as dst:
 total=0
 while True:
  chunk=src.read(1024*1024)
  if not chunk:break
  total+=len(chunk);assert total<=256*1024*1024,'Fixture exceeds QA capacity bound'
  dst.write(chunk)
loop=None;alias=None;partalias=None;registry=None
results=[]
def run(*args,fail=False):
 p=subprocess.run([str(x) for x in args],capture_output=True)
 if fail:assert p.returncode!=0,(args,p.stdout);return p
 if p.returncode:raise RuntimeError(str(args)+'\n'+(p.stdout+p.stderr).decode(errors='replace'))
 return p.stdout
def cmd(*args,**kwargs):return run(BIN,*args,**kwargs)
def digest(p):
 h=hashlib.sha256()
 with Path(p).open('rb') as f:
  for b in iter(lambda:f.read(4*1024*1024),b''):h.update(b)
 return h.hexdigest()
try:
 loop=run('losetup','--find','--show','--partscan',image).decode().strip()
 assert loop.startswith('/dev/loop') and loop[9:].isdigit()
 run('udevadm','settle')
 document=json.loads(run('lsblk','--json','--tree','-o','PATH,TYPE,FSTYPE,PARTUUID',loop))
 (OUT/'lsblk.json').write_text(json.dumps(document,indent=2))
 selected=find_apfs_partition(document,loop)
 part=Path(selected['path'])
 pu=selected['partuuid'];assert pu and all(c in '0123456789abcdefABCDEF-' for c in pu)
 # Only this uniquely named test device gains a persistent-looking test alias.
 alias=Path('/dev/disk/by-id')/(OUT.name+'-part1');assert not alias.exists();alias.symlink_to(part)
 pa=Path('/dev/disk/by-partuuid')/pu
 if not pa.exists():pa.parent.mkdir(exist_ok=True);pa.symlink_to(part);partalias=pa
 info=json.loads(cmd('inspect',part,0));assert info['source_kind']=='linux-block-device'
 assert info['volumes'][0]['name']=='LAPFS_BETA_QA';results.append('raw_block_ioctl_inspect')
 before=digest(image);files=json.loads(cmd('ls',part,0,'/'));assert files['entries'];cmd('digest',part,0,'/payload.bin')
 cmd('export',part,0,'/payload.bin',OUT/'export.bin');assert digest(image)==before;results.append('raw_block_read_export_no_changes')
 enroll=json.loads(cmd('device-enroll',alias,info['container_uuid']));target=Path(enroll['target']);registry=target.parent
 assert registry.parent==Path('/var/lib/lapfs/devices') and registry.name==info['container_uuid']
 source=OUT/'source.bin';source.write_bytes(bytes(range(251))*60000+b'BLOCK TEST END');sha=digest(source)
 job=registry/'test-import'
 cmd('import-plan',target,0,source,'/block-test.bin',job)
 assert json.loads(cmd('resume',job,1))['state']=='Paused'
 assert json.loads(cmd('resume',job))['state']=='Completed'
 assert json.loads(cmd('digest',part,0,'/block-test.bin'))['sha256']==sha;results.append('raw_block_write_resume_readback')
 # Staged ownership must block a raw reader and a second transaction.
 batch=OUT/'batch.json';batch.write_text(json.dumps([{'op':'mkdir','path':'/block-test-dir'}]))
 journal=registry/'test-journal';cmd('prepare',target,0,batch,journal)
 cmd('inspect',part,0,fail=True);results.append('pending_raw_read_denied')
 cmd('recover',journal);cmd('cleanup',journal);cmd('inspect',part,0);results.append('prepared_cancel_releases_owner')
 # Wrong device UUID, whole-device enrollment, and volatile/wrong spool are rejected.
 cmd('device-enroll',alias,'00000000-0000-0000-0000-000000000000',fail=True)
 cmd('import-plan',target,0,source,'/forbidden.bin',OUT/'outside-job',fail=True)
 assert not (OUT/'outside-job').exists();results.append('uuid_and_external_spool_denied')
 # Exercise the registered kernel block device through the new RW mount.
 # The mount is owned by the invoking user; only this synthetic OUT is traversable.
 os.chmod(OUT,0o711)
 mp=OUT/'rw-mount';mp.mkdir()
 mountlog=(OUT/'rw-mount.log').open('wb')
 mounted=subprocess.Popen([str(BIN),'mount-rw',str(target),'0',str(mp),str(registry/'rw-session')],stdout=mountlog,stderr=mountlog)
 try:
  for _ in range(200):
   if os.path.ismount(mp):break
   assert mounted.poll() is None,'RW mount failed; see rw-mount.log'
   time.sleep(.05)
  assert os.path.ismount(mp),'RW mount timed out'
  import pwd
  username=pwd.getpwuid(int(os.environ.get('SUDO_UID','0'))).pw_name
  script="import pathlib,os,hashlib; p=pathlib.Path("+repr(str(mp/'rw-block.bin'))+"); data=bytes(range(251))*20000+b'RW BLOCK END'; f=p.open('wb'); f.write(data); f.flush(); os.fsync(f.fileno()); f.close(); assert hashlib.sha256(p.read_bytes()).digest()==hashlib.sha256(data).digest()"
  run('runuser','-u',username,'--','python3','-c',script)
  results.append('registered_block_buffered_rw_fuse_as_invoking_user')
 finally:
  if os.path.ismount(mp):run('fusermount3','-u',mp)
  mounted.wait(timeout=30)
 assert mounted.returncode==0,'RW unmount did not finish cleanly'
 rw_hash=json.loads(cmd('digest',part,0,'/rw-block.bin'))['sha256']
 assert rw_hash==hashlib.sha256(bytes(range(251))*20000+b'RW BLOCK END').hexdigest()
 results.append('registered_block_rw_unmount_native_readback')
 # Remove registry only when all task-created transactions are terminal.
 assert not list(registry.glob('.*spark-apfs-owner.json'))
 shutil.rmtree(registry);registry=None
 result={'status':'passed','checks':results,'source_bytes':source.stat().st_size,'import_sha256':sha,'loop_device':loop,'physical_usb_writes':0,'image':str(image),'binary_sha256':digest(BIN),'rw_mount_sha256':rw_hash,'apple_fsck':'pending-independent-Mac-check','output':str(OUT)}
 (OUT/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))
except Exception as e:
 import traceback
 (OUT/'traceback.log').write_text(traceback.format_exc())
 (OUT/'failure.json').write_text(json.dumps({'error':str(e),'completed_checks':results,'recovery_registry':str(registry) if registry else None},indent=2));raise
finally:
 pending=registry is not None and any(registry.glob('.*spark-apfs-owner.json'))
 if not pending:
  if alias and alias.is_symlink():alias.unlink()
  if partalias and partalias.is_symlink():partalias.unlink()
  if loop:run('losetup','--detach',loop)
 else:print('Pending recovery retained: loop, alias, registry',loop,alias,registry)
 # Root-generated read-back artifacts may be retrieved by the invoking user.
 uid=int(os.environ.get('SUDO_UID','0'));gid=int(os.environ.get('SUDO_GID','0'))
 for p in [OUT,*OUT.rglob('*')]:os.chown(p,uid,gid)
 print('QA evidence:',OUT)
