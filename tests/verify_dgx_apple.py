"""Independent Apple validation of a recovered DGX QA image."""
from pathlib import Path
import hashlib,json,plistlib,subprocess,sys
ROOT=Path(__file__).resolve().parents[1]
p=Path(sys.argv[1]).resolve();assert p.name=='results' and p.parent.parent==ROOT/'evidence' and p.parent.name.startswith('dgx-qa-')
r=json.loads((p/'result.json').read_text())
def run(args):
 p=subprocess.run([str(x) for x in args],capture_output=True)
 if p.returncode:raise RuntimeError((p.stdout+p.stderr).decode())
 return p.stdout
entities=plistlib.loads(run(['hdiutil','attach','-readonly','-plist',p/'dgx-written.dmg']))['system-entities']
parent=next(e['dev-entry'] for e in entities if e.get('content-hint')=='GUID_partition_scheme')
try:
 container=next(e['dev-entry'] for e in entities if e.get('content-hint','').startswith('EF57347C'))
 mount=Path(next(e['mount-point'] for e in entities if 'mount-point' in e))
 log=run(['/sbin/fsck_apfs','-n',container]).decode();(p/'apple_fsck.log').write_text(log)
 assert 'appears to be OK' in log and not any(x in log.lower() for x in ['error','warning','corrupt','overallocation','is invalid','is not valid'])
 assert hashlib.sha256((mount/'dgx-result.bin').read_bytes()).hexdigest()==r['file_sha256']
 r.update(apple_fsck='clean',native_mac_read='sha256 matched');(p/'result.json').write_text(json.dumps(r,indent=2));print(json.dumps(r))
finally:run(['hdiutil','detach',parent])
