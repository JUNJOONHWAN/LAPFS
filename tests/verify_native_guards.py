"""Native-created hard links, clones, xattrs, symlinks and immutability guards."""
from pathlib import Path
import hashlib,json,os,plistlib,struct,subprocess,tempfile
ROOT=Path(__file__).resolve().parents[1];OUT=Path(tempfile.mkdtemp(prefix='guards-qa-',dir=ROOT/'evidence'));BIN=ROOT/'target/debug/spark-apfs-safe'
def call(args,ok=True):
 r=subprocess.run([str(x) for x in args],capture_output=True)
 if ok and r.returncode:raise RuntimeError(r.stderr.decode()+r.stdout.decode())
 return r
image=OUT/'native.dmg';source=OUT/'source';source.write_bytes(b'replacement')
call(['hdiutil','create','-size','64m','-fs','APFS','-volname','SPARK_GUARDS_QA','-layout','GPTSPUD',image])
raw=image.read_bytes();start=struct.unpack_from('<Q',raw,584)[0]*512;count,stride=struct.unpack_from('<II',raw,592)
offset=next(struct.unpack_from('<Q',raw,start+i*stride+32)[0]*512 for i in range(count) if raw[start+i*stride:start+i*stride+16].hex()=='ef57347c0000aa11aa1100306543ecac')
ent=plistlib.loads(call(['hdiutil','attach','-plist',image]).stdout)['system-entities'];parent=next(e['dev-entry'] for e in ent if e.get('content-hint')=='GUID_partition_scheme')
try:
 m=Path(next(e['mount-point'] for e in ent if 'mount-point' in e))
 for n in ['hard','original','attrs','immutable','plain']:(m/n).write_bytes(b'native source'*1024)
 
 for n in ['hard','original','attrs','immutable','plain']:call(['/usr/bin/xattr','-c',m/n])
 os.link(m/'hard',m/'hard2');call(['/bin/cp','-c',m/'original',m/'clone']);call(['/usr/bin/xattr','-w','com.example.spark-test','preserve',m/'attrs'])
 os.symlink('plain',m/'symlink');call(['/usr/bin/chflags','uchg',m/'immutable'])
 with (m/'sparse').open('wb') as f:f.truncate(8*1024*1024)
 call(['/usr/bin/xattr','-c',m/'sparse'])
 provenance=call(['/usr/bin/xattr','-px','com.apple.provenance',m/'plain']).stdout.strip()
finally:call(['hdiutil','detach',parent])
before=hashlib.sha256(image.read_bytes()).hexdigest();results=[]
for name in ['hard','hard2','original','clone','attrs','immutable','symlink','sparse']:
 batch=OUT/f'{name}.json';batch.write_text(json.dumps([{'op':'put','source':str(source),'path':'/'+name}]))
 r=call([BIN,'prepare',image,offset,batch,OUT/f'j-{name}',16,1024],False)
 assert r.returncode!=0,(name,'unsupported mutation accepted')
 assert hashlib.sha256(image.read_bytes()).hexdigest()==before
 results.append({'kind':name,'rejected':True,'image_unchanged':True,'reason':r.stderr.decode().splitlines()[0]})
# A normal native file must remain writable; otherwise a blanket denial would pass guards.
batch=OUT/'plain.json';batch.write_text(json.dumps([{'op':'put','source':str(source),'path':'/plain'}]));j=OUT/'j-plain'
call([BIN,'prepare',image,offset,batch,j,16,1024]);call([BIN,'apply',j]);call([BIN,'cleanup',j]);assert call([BIN,'read',image,offset,'/plain']).stdout==source.read_bytes()
ent=plistlib.loads(call(['hdiutil','attach','-readonly','-plist',image]).stdout)['system-entities'];parent=next(e['dev-entry'] for e in ent if e.get('content-hint')=='GUID_partition_scheme')
try:
 m=Path(next(e['mount-point'] for e in ent if 'mount-point' in e))
 assert call(['/usr/bin/xattr','-px','com.apple.provenance',m/'plain']).stdout.strip()==provenance
 c=next(e['dev-entry'] for e in ent if e.get('content-hint','').startswith('EF57347C'))
 fsck=call(['/sbin/fsck_apfs','-n',c]).stdout.decode();(OUT/'fsck.log').write_text(fsck)
 assert 'appears to be OK' in fsck and not any(x in fsck.lower() for x in ['error','warning','corrupt','overallocation','is invalid','is not valid'])
 results.append({'kind':'native_plain_with_provenance','write':'passed','provenance_preserved':True,'apple_fsck':'clean'})
finally:call(['hdiutil','detach',parent])
(OUT/'results.json').write_text(json.dumps(results,indent=2));print(json.dumps(results));print('Evidence:',OUT)
