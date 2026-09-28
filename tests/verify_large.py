"""4 GiB boundary, bounded spool monitoring and native macOS round-trip QA."""
from pathlib import Path
import hashlib,json,plistlib,struct,subprocess,tempfile,time,os,re
ROOT=Path(__file__).resolve().parents[1]; OUT=Path(os.environ['SPARK_LARGE_RESUME']) if 'SPARK_LARGE_RESUME' in os.environ else Path(tempfile.mkdtemp(prefix='large-qa-',dir=ROOT/'evidence'))
assert OUT.resolve().parent == (ROOT/'evidence').resolve() and OUT.name.startswith('large-qa-')
BIN=ROOT/'target/release/spark-apfs-safe'
def run(args):
    r=subprocess.run([str(x) for x in args],capture_output=True)
    if r.returncode: raise RuntimeError(r.stderr.decode()+r.stdout.decode())
    return r.stdout

def sha(path):
    h=hashlib.sha256()
    with open(path,'rb') as f:
        while b:=f.read(4*1024*1024): h.update(b)
    return h.hexdigest()
image=OUT/'large.dmg'; source=OUT/'source.bin'; job=OUT/'job'
print('Evidence:',OUT,flush=True)
if not image.exists(): run(['hdiutil','create','-size','6g','-fs','APFS','-volname','SPARK_LARGE_QA','-layout','GPTSPUD',image])
with image.open('rb') as f: raw=f.read(1024*1024)
entry=struct.unpack_from('<Q',raw,512+72)[0]*512; count,stride=struct.unpack_from('<II',raw,512+80)
offset=next(struct.unpack_from('<Q',raw,entry+i*stride+32)[0]*512 for i in range(count) if raw[entry+i*stride:entry+i*stride+16].hex()=='ef57347c0000aa11aa1100306543ecac')
if not source.exists():
    with source.open('wb') as f:
        for index in range(1024): f.write(index.to_bytes(8,'little')*(4*1024*1024//8))
        f.write(b'past-4-GiB-boundary!')
expected=sha(source); start=time.monotonic()
args=[BIN,'resume',job] if job.exists() else [BIN,'import',image,offset,source,'/large.bin',job]
p=subprocess.Popen(['/usr/bin/time','-l']+[str(x) for x in args],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
peak=0; last=0
while p.poll() is None:
    try:
        peak=max(peak,sum(x.stat().st_size for x in job.rglob('*') if x.is_file()))
        j=json.loads(json.loads((job/'job.json').read_text())['payload'])
        if time.monotonic()-last>20:
            print(json.dumps({'completed_mib':j['completed']//1048576,'peak_job_bytes':peak}),flush=True);last=time.monotonic()
    except (FileNotFoundError,json.JSONDecodeError): pass
    time.sleep(.1)
out,err=p.communicate()
(OUT/'time.log').write_bytes(err)
rss=re.search(rb'(\d+)\s+maximum resident set size',err)
peak_rss=int(rss.group(1)) if rss else None
if p.returncode: raise RuntimeError(err.decode()+out.decode())
assert json.loads(out)['state']=='Completed'
assert json.loads(run([BIN,'digest',image,offset,'/large.bin']))['sha256']==expected
entities=plistlib.loads(run(['hdiutil','attach','-plist',image]))['system-entities']
parent=next(e['dev-entry'] for e in entities if e.get('content-hint')=='GUID_partition_scheme')
try:
    mount=Path(next(e['mount-point'] for e in entities if 'mount-point' in e))
    assert sha(mount/'large.bin')==expected
    # Let native macOS modify the file, then read it back with our parser.
    with (mount/'large.bin').open('r+b') as f:
        f.seek(2**32+3);f.write(b'MAC-ROUNDTRIP')
    native_expected=sha(mount/'large.bin')
finally: run(['hdiutil','detach',parent])
assert json.loads(run([BIN,'digest',image,offset,'/large.bin']))['sha256']==native_expected
entities=plistlib.loads(run(['hdiutil','attach','-readonly','-plist',image]))['system-entities']
parent=next(e['dev-entry'] for e in entities if e.get('content-hint')=='GUID_partition_scheme')
try:
    container=next(e['dev-entry'] for e in entities if e.get('content-hint','').startswith('EF57347C'))
    log=run(['/sbin/fsck_apfs','-n',container]).decode();(OUT/'fsck.log').write_text(log)
    assert 'appears to be OK' in log and not any(x in log.lower() for x in ['error','warning','corrupt','overallocation','is invalid','is not valid'])
finally: run(['hdiutil','detach',parent])
result={'import_peak_rss_bytes':peak_rss,'file_bytes':source.stat().st_size,'peak_job_bytes_sampled':peak,'elapsed_seconds':time.monotonic()-start,'native_original_digest':expected,'native_modified_digest':native_expected,'apple_fsck':'clean','mac_write_rust_read':'passed','retained_job_bytes':sum(x.stat().st_size for x in job.rglob('*') if x.is_file())}
(OUT/'results.json').write_text(json.dumps(result,indent=2));print(json.dumps(result),flush=True)
