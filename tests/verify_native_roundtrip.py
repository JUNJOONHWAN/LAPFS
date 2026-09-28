#!/usr/bin/env python3
"""Disposable cross-host APFS native roundtrip. Never targets a block device."""
import gzip,hashlib,json,os,plistlib,subprocess,time
from pathlib import Path
import argparse,shlex
parser=argparse.ArgumentParser(description="Disposable Mac native write / Linux LAPFS write / Mac fsck roundtrip")
parser.add_argument('--fixture',type=Path,required=True,help='gzip-compressed APFS GPT image, partition offset 20480')
parser.add_argument('--expected',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--ssh-host',required=True)
parser.add_argument('--remote-output',required=True,help='new disposable directory on Linux')
parser.add_argument('--linux-binary',required=True)
a=parser.parse_args()
out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
img=out/'roundtrip.dmg'
with gzip.open(a.fixture,'rb') as f:img.write_bytes(f.read())
expected=json.loads(a.expected.read_text())['files']
remote=a.remote_output;binary=a.linux_binary
# Restrict command operands to absolute safe paths; the supplied host is an SSH
# destination and never interpreted as remote shell text.
import re
assert all(re.fullmatch(r'/[A-Za-z0-9_./-]+',x) for x in [remote,binary])
assert not a.ssh_host.startswith('-')
def run(*args):return subprocess.check_output(list(map(str,args)),stderr=subprocess.STDOUT)
def mount(ro=False):
    args=['hdiutil','attach','-nobrowse','-plist']
    if ro:args+=['-readonly']
    e=plistlib.loads(run(*args,img))['system-entities']
    return next(x['dev-entry'] for x in e if x.get('content-hint')=='GUID_partition_scheme'),next(x['dev-entry'] for x in e if x.get('content-hint','').startswith('EF57347C')),Path(next(x['mount-point'] for x in e if 'mount-point' in x))
def verify(m):
    for p,e in expected.items():assert hashlib.sha256((m/p).read_bytes()).hexdigest()==e['sha256'],p
run('ssh',a.ssh_host,f'mkdir {remote}')
rows=[]
for cycle in range(1,4):
    parent,dev,m=mount()
    try:
        verify(m)
        if cycle>1:assert (m/'roundtrip-data').read_bytes()==b'L'*4096+previous[4096:]
        payload=(f'Mac cycle {cycle}'.encode()*1000)
        p=m/'roundtrip-data';p.write_bytes(payload)
        os.chmod(p,0o640);os.utime(p,ns=(946684800123456789,946684800123456789))
        (m/'mac-temporary').write_bytes(b'temporary');(m/'mac-temporary').unlink()
        with p.open('rb') as f:os.fsync(f.fileno())
        previous=payload
    finally:run('hdiutil','detach',parent)
    gz=out/'upload.gz'
    with gzip.open(gz,'wb',compresslevel=1) as f:f.write(img.read_bytes())
    run('scp',gz,f'{a.ssh_host}:{remote}/upload.gz')
    command=f'''cd {remote} && gzip -dc upload.gz > image.dmg && python3 - <<'REMOTE'
import json,subprocess
from pathlib import Path
b='{binary}';p=Path.cwd();(p/'patch').write_bytes(b'L'*4096)
(p/'batch.json').write_text(json.dumps([{{'op':'write_at','path':'/roundtrip-data','offset':0,'source':str(p/'patch')}}]))
subprocess.run([b,'prepare',str(p/'image.dmg'),'20480',str(p/'batch.json'),str(p/'journal-{cycle}'),'64','1024'],check=True)
subprocess.run([b,'apply',str(p/'journal-{cycle}')],check=True)
REMOTE
'''
    (out/f'linux-{cycle}.log').write_bytes(run('ssh',a.ssh_host,command))
    with (out/'download.gz').open('wb') as f:subprocess.run(['ssh',a.ssh_host,f'gzip -1 -c {remote}/image.dmg'],stdout=f,check=True)
    with gzip.open(out/'download.gz','rb') as f:img.write_bytes(f.read())
    parent,dev,m=mount(True)
    try:
        log=run('/sbin/fsck_apfs','-n',dev).decode();(out/f'fsck-{cycle}.log').write_text(log)
        if 'is greater than current time' in log:
            (out/f'fsck-{cycle}-clock-ahead.log').write_text(log)
            time.sleep(5)
            log=run('/sbin/fsck_apfs','-n',dev).decode();(out/f'fsck-{cycle}.log').write_text(log)
        assert 'appears to be OK' in log and not any(x in log.lower() for x in ['error','warning','corrupt','overallocation','invalid'])
        verify(m);assert (m/'roundtrip-data').read_bytes()==b'L'*4096+payload[4096:]
        assert (m/'roundtrip-data').stat().st_mode&0o777==0o640
        rows.append({'cycle':cycle,'fsck':'clean','original_files':len(expected),'linux_patch':'match','mode':'0640'})
    finally:run('hdiutil','detach',parent)
    print(rows[-1],flush=True)
    (out/'result.json').write_text(json.dumps(rows,indent=2))
