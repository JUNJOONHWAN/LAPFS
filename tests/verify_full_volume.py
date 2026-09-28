"""Real APFS no-space, resume-no-space and original-preservation checks."""
from pathlib import Path
import hashlib,json,plistlib,struct,subprocess,tempfile
ROOT=Path(__file__).resolve().parents[1];OUT=Path(tempfile.mkdtemp(prefix='full-qa-',dir=ROOT/'evidence'));BIN=ROOT/'target/debug/spark-apfs-safe'
def call(args, ok=True):
    r=subprocess.run([str(x) for x in args],capture_output=True)
    if ok and r.returncode:raise RuntimeError(r.stderr.decode()+r.stdout.decode())
    return r
image=OUT/'full.dmg';source=OUT/'source';job=OUT/'job'
call(['hdiutil','create','-size','64m','-fs','APFS','-volname','SPARK_FULL_QA','-layout','GPTSPUD',image])
raw=image.read_bytes();start=struct.unpack_from('<Q',raw,512+72)[0]*512;count,stride=struct.unpack_from('<II',raw,512+80)
offset=next(struct.unpack_from('<Q',raw,start+i*stride+32)[0]*512 for i in range(count) if raw[start+i*stride:start+i*stride+16].hex()=='ef57347c0000aa11aa1100306543ecac')
with source.open('wb') as f:f.truncate(64*1024*1024)
r=call([BIN,'import',image,offset,source,'/too-large.bin',job],False);assert r.returncode!=0 and b'no free blocks' in r.stderr
(OUT/'failure.log').write_bytes(r.stderr)
before=hashlib.sha256(image.read_bytes()).hexdigest()
r=call([BIN,'resume',job],False);assert r.returncode!=0 and b'no free blocks' in r.stderr
assert hashlib.sha256(image.read_bytes()).hexdigest()==before
j=json.loads(json.loads((job/'job.json').read_text())['payload']);assert j['completed']<source.stat().st_size
ent=plistlib.loads(call(['hdiutil','attach','-readonly','-plist',image]).stdout)['system-entities'];parent=next(e['dev-entry'] for e in ent if e.get('content-hint')=='GUID_partition_scheme')
try:
 c=next(e['dev-entry'] for e in ent if e.get('content-hint','').startswith('EF57347C'));mount=Path(next(e['mount-point'] for e in ent if 'mount-point' in e))
 fsck=call(['/sbin/fsck_apfs','-n',c]).stdout.decode();(OUT/'fsck.log').write_text(fsck)
 assert 'appears to be OK' in fsck and not any(x in fsck.lower() for x in ['error','warning','corrupt','overallocation','is invalid','is not valid'])
 assert not (mount/'too-large.bin').exists()
 partial=mount/j['temporary'].lstrip('/');assert partial.stat().st_size==j['completed']
 with partial.open('rb') as f:
  while b:=f.read(1048576):assert not any(b)
finally:call(['hdiutil','detach',parent])
result={'completed_before_full':j['completed'],'failed_retry_image_unchanged':True,'unpublished_destination':True,'partial_content_verified':True,'apple_fsck':'clean'}
(OUT/'results.json').write_text(json.dumps(result,indent=2));print(json.dumps(result));print('Evidence:',OUT)
