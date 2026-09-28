#!/usr/bin/env python3
"""Create GPL source and native Linux assets from a clean canonical Git checkout."""
from pathlib import Path
import hashlib,json,re,subprocess,tarfile
root=Path(__file__).resolve().parents[1]
assert not subprocess.check_output(['git','status','--porcelain'],cwd=root).strip(),'Commit the reviewed source before packaging'
version=re.search(r'^version = "([^"]+)"',(root/'Cargo.toml').read_text(),re.M).group(1)
commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
files=subprocess.check_output(['git','ls-files','-z'],cwd=root).decode().split('\0');files=[x for x in files if x]
binary=root/'target/release/lapfs';assert binary.is_file()
dist=root/'dist';dist.mkdir(exist_ok=True)
source=dist/f'lapfs-{version}-source.tar.gz'
with tarfile.open(source,'w:gz') as t:
 for n in files:t.add(root/n,arcname='lapfs/'+n,recursive=False)
runtime=dist/f'lapfs-{version}-linux-arm64-gnu.tar.gz'
with tarfile.open(runtime,'w:gz') as t:
 t.add(binary,arcname='lapfs/bin/lapfs')
 for n in files:
  if n in ['README.md','LICENSE','THIRD_PARTY_NOTICES.md','UPSTREAM.md','RELEASE_NOTES.md','scripts/verify-block-device.py','scripts/block_qa_support.py','scripts/safe-eject.py'] or n.startswith(('docs/','licenses/','fixtures/')):
   t.add(root/n,arcname='lapfs/'+n,recursive=False)
 # Preserve all dependency license files with the binary distribution too.
 for n in files:
  if n.startswith(('vendor/','dependencies/')) and Path(n).name.lower().startswith(('license','copying','copyright','notice')):
   t.add(root/n,arcname='lapfs/'+n,recursive=False)
receipt={'version':version,'commit':commit,'canonical_build_host':'DGX Spark / Linux ARM64','binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'assets':{}}
for p in [source,runtime]:receipt['assets'][p.name]=hashlib.sha256(p.read_bytes()).hexdigest()
(dist/'BUILD_RECEIPT.json').write_text(json.dumps(receipt,indent=2)+'\n')
assets=[source,runtime,dist/'BUILD_RECEIPT.json']
(dist/'SHA256SUMS').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n' for p in assets))
with tarfile.open(runtime) as t:assert hashlib.sha256(t.extractfile('lapfs/bin/lapfs').read()).hexdigest()==receipt['binary_sha256']
with tarfile.open(source) as t:
 assert len(t.getmembers())==len(files)
 for n in files:assert hashlib.sha256(t.extractfile('lapfs/'+n).read()).digest()==hashlib.sha256((root/n).read_bytes()).digest(),n
print(json.dumps(receipt,indent=2))
