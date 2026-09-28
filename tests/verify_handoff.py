#!/usr/bin/env python3
"""Disposable APFS image: clean FUSE handoff and dead-daemon recovery."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=root/'target/release/lapfs')
parser.add_argument('--output', type=Path, default=root/'evidence')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
work = Path(tempfile.mkdtemp(prefix='lapfs-handoff-',dir=args.output))
base = work/'base.dmg'
with gzip.open(root/'fixtures/block-test.dmg.gz','rb') as source, base.open('wb') as dest:
    shutil.copyfileobj(source,dest)
exe = args.binary.resolve()
helper = root/'scripts/safe-eject.py'
expected = b'LAPFS mac linux handoff fixture' * 113
results = {}

def wait_mount(mp, present):
    for _ in range(100):
        mounted = subprocess.run(['findmnt','-n','--mountpoint',str(mp)],capture_output=True).returncode == 0
        if mounted == present:
            return
        time.sleep(.1)
    raise AssertionError('FUSE mount did not reach expected state')

for case in ['clean','killed']:
    path = work/case
    path.mkdir()
    image = path/'native.dmg'
    shutil.copy2(base,image)
    mp = path/'mnt'
    mp.mkdir()
    session = path/'session'
    with (path/'mount.log').open('wb') as log:
        proc = subprocess.Popen([str(exe),'mount-rw',str(image),'20480',str(mp),str(session)],stdout=log,stderr=log)
        try:
            wait_mount(mp,True)
            rejected = subprocess.run([str(exe),'handoff-ready',str(image),'20480',str(session)],capture_output=True)
            assert rejected.returncode != 0, 'Active mount was accepted'
            fd = os.open(mp/'handoff.bin',os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
            try:
                assert os.write(fd,expected) == len(expected)
                if case == 'clean':
                    os.fsync(fd)
            finally:
                if case == 'killed':
                    proc.send_signal(signal.SIGKILL)
                    proc.wait(timeout=15)
                try: os.close(fd)
                except OSError: pass
            receipt = subprocess.run(['python3',str(helper),'--binary',str(exe),str(image),'20480',str(mp),str(session)],capture_output=True,text=True)
            assert receipt.returncode == 0, receipt.stderr
            obj = json.loads(receipt.stdout)
            assert obj['status'] == 'ready_to_disconnect' and obj['session']['closed'] is True
            if case == 'clean':
                assert proc.wait(timeout=15) == 0
            wait_mount(mp,False)
            digest = subprocess.run([str(exe),'digest',str(image),'20480','/handoff.bin'],capture_output=True,text=True)
            assert digest.returncode == 0, digest.stderr
            assert json.loads(digest.stdout)['sha256'] == hashlib.sha256(expected).hexdigest()
            results[case] = {'status':'passed','image':str(image),'image_sha256':hashlib.sha256(image.read_bytes()).hexdigest(),'file_sha256':hashlib.sha256(expected).hexdigest()}
        finally:
            if proc.poll() is None:
                proc.terminate()
                try: proc.wait(timeout=5)
                except subprocess.TimeoutExpired: proc.kill()
(work/'result.json').write_text(json.dumps(results,indent=2))
print(json.dumps({'status':'passed','output':str(work),'cases':list(results)}))
