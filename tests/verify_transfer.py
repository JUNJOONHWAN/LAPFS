"""Kill actual Rust processes at protocol boundaries; resume and check with Apple.
Fault-injection is opt-in at compile time and disabled in distributable builds.
"""
from pathlib import Path
import hashlib, json, os, plistlib, shutil, signal, struct, subprocess, tempfile
ROOT = Path(__file__).resolve().parents[1]
OUT = Path(tempfile.mkdtemp(prefix='transfer-qa-', dir=ROOT/'evidence'))
BIN = ROOT/'target/fault/debug/spark-apfs-safe'
def run(args, kill=None):
    env = dict(os.environ)
    env.pop('SPARK_APFS_KILL_AT', None)
    if kill: env['SPARK_APFS_KILL_AT'] = kill
    r = subprocess.run([str(x) for x in args], capture_output=True, env=env)
    if kill:
        assert r.returncode == -signal.SIGKILL, (kill, r.returncode, r.stderr.decode())
    elif r.returncode: raise RuntimeError(r.stderr.decode()+r.stdout.decode())
    return r.stdout
base=OUT/'base.dmg'
run(['hdiutil','create','-size','64m','-fs','APFS','-volname','SPARK_TRANSFER_QA','-layout','GPTSPUD',base])
raw=base.read_bytes(); off=struct.unpack_from('<Q',raw,512+72)[0]*512
count,stride=struct.unpack_from('<II',raw,512+80)
offset=next(struct.unpack_from('<Q',raw,off+i*stride+32)[0]*512 for i in range(count) if raw[off+i*stride:off+i*stride+16].hex()=='ef57347c0000aa11aa1100306543ecac')
source=OUT/'source.bin'
source.write_bytes(b''.join(bytes([i])*1024*1024 for i in range(13))+b'last-unaligned-byte!')
sha=hashlib.sha256(source.read_bytes()).hexdigest()
results=[]
for hook in ['state-Applying','apply-write','state-Committed','cleanup-receipt']:
    image=OUT/f'{hook}.dmg'; shutil.copyfile(base,image); job=OUT/f'job-{hook}'
    run([BIN,'import-plan',image,offset,source,'/large.bin',job])
    run([BIN,'resume',job,1],kill=hook)
    # Repeat failure while recovering an actually interrupted write.
    if hook=='apply-write': run([BIN,'resume',job,1],kill='recover-write')
    paused=json.loads(run([BIN,'resume',job,1]))
    assert paused['state']=='Paused' and paused['bytes'] < source.stat().st_size
    result=json.loads(run([BIN,'resume',job]))
    assert result['state']=='Completed' and result['bytes']==source.stat().st_size
    assert json.loads(run([BIN,'digest',image,offset,'/large.bin']))['sha256']==sha
    assert json.loads(run([BIN,'resume',job]))['state']=='Completed'
    assert not list(job.glob('chunk-*')), 'Unbounded completed chunk journals retained'
    entities=plistlib.loads(run(['hdiutil','attach','-readonly','-plist',image]))['system-entities']
    parent=next(e['dev-entry'] for e in entities if e.get('content-hint')=='GUID_partition_scheme')
    try:
        container=next(e['dev-entry'] for e in entities if e.get('content-hint','').startswith('EF57347C'))
        mount=Path(next(e['mount-point'] for e in entities if 'mount-point' in e))
        fsck=run(['/sbin/fsck_apfs','-n',container]).decode(); (OUT/f'{hook}-fsck.log').write_text(fsck)
        assert 'appears to be OK' in fsck and not any(x in fsck.lower() for x in ['error','warning','corrupt','overallocation','is invalid','is not valid'])
        assert hashlib.sha256((mount/'large.bin').read_bytes()).hexdigest()==sha
        assert not list(mount.glob('.spark-*.partial'))
    finally: run(['hdiutil','detach',parent])
    result.update(kill_point=hook, apple_fsck='clean', native_sha256=sha, job_disk_bytes=sum(p.stat().st_size for p in job.rglob('*') if p.is_file()))
    results.append(result); print(json.dumps(result),flush=True)
(OUT/'results.json').write_text(json.dumps(results,indent=2)); print('Evidence:',OUT)
