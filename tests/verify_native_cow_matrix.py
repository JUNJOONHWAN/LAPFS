#!/usr/bin/env python3
"""Replay exact prepared APFS I/O prefixes; validate without recovery on macOS."""
import argparse,copy,gzip,hashlib,json,os,plistlib,shutil,subprocess,tempfile
from pathlib import Path

def sha(b): return hashlib.sha256(b).hexdigest()
def run(*args):
    r=subprocess.run(list(map(str,args)),capture_output=True)
    if r.returncode: raise RuntimeError(r.stderr.decode(errors='replace'))
    return r.stdout
def seal_read(p):
    x=json.loads(p.read_text());assert sha(x['payload'].encode())==x['sha256']
    return json.loads(x['payload'])
def file_spec(data):return {'kind':'file','sha256':sha(data),'bytes':len(data)}

def generate(a):
    out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
    base=out/'base.dmg'
    with gzip.open(a.fixture,'rb') as f:base.write_bytes(f.read())
    b=a.binary.resolve();provenance={'binary_sha256':sha(b.read_bytes()),'fixture_gzip_sha256':sha(a.fixture.read_bytes())};d='/native-cow-dir';p=d+'/data';ren=d+'/renamed';link=d+'/link'
    data=b'A'*8192;patch=b'Z'*5000;tail=b'T'*4096
    steps=[
      ({'op':'mkdir','path':d},{d:{'kind':'dir'}},None),
      ({'op':'put','path':p},{p:file_spec(data)},data),
      ({'op':'write_at','path':p,'offset':13},{p:file_spec(data[:13]+patch+data[5013:])},patch),
    ]
    data=data[:13]+patch+data[5013:]
    steps += [({'op':'append','path':p,'expected_size':8192,'source_offset':0,'length':4096},{p:file_spec(data+tail)},tail)]
    data+=tail
    steps += [({'op':'write_at','path':p,'offset':len(data)},{p:file_spec(data+b'end')},b'end')]
    data+=b'end'
    steps += [({'op':'truncate','path':p,'size':3000},{p:file_spec(data[:3000])},None)]
    data=data[:3000]+b'\0'*2000
    steps += [({'op':'truncate','path':p,'size':5000},{p:file_spec(data)},None)]
    data=b'new replacement'*701
    steps += [({'op':'put','path':p},{p:file_spec(data)},data),
      ({'op':'set_attrs','path':p,'mode':416,'atime_ns':946684800123456789,'mtime_ns':946684800123456789},{},None),
      ({'op':'rename','path':p,'name':'renamed'},{p:None,ren:file_spec(data)},None),
      ({'op':'symlink','path':link,'target':list(b'renamed')},{link:{'kind':'symlink','target':'renamed'}},None),
      ({'op':'remove','path':link},{link:None},None),
      ({'op':'remove','path':ren},{ren:None},None),
      ({'op':'rmdir','path':d},{d:None},None),
      ({'op':'put','path':'/native-cow-empty'},{'/native-cow-empty':file_spec(b'')},b''),
      ({'op':'remove','path':'/native-cow-empty'},{'/native-cow-empty':None},None)]
    state={};rows=[];scope=[d,p,ren,link,'/native-cow-empty']
    for index,(action,changes,content) in enumerate(steps,1):
        before=copy.deepcopy(state);state.update(changes);after=copy.deepcopy(state)
        work=out/f'work-{index}';work.mkdir()
        if content is not None:
            source=work/'payload';source.write_bytes(content);action['source']=str(source)
        plan=work/'action.json';plan.write_text(json.dumps([action]));journal=work/'journal'
        run(b,'prepare',base,20480,plan,journal,64,0)
        manifest=seal_read(journal/'manifest.json');redo=(journal/'redo.bin').read_bytes()
        image=bytearray(base.read_bytes())
        def save(label,raw,step,kind):
            name=f'{index:02d}-{label}.dmg.gz'
            with gzip.open(out/name,'wb',compresslevel=1) as z:z.write(raw)
            rows.append({'image':name,'action':action['op'],'step':step,'kind':kind,'before':before,'after':after,'image_sha256':sha(raw)})
        for step,op in enumerate(manifest['ops'],1):
            if isinstance(op,dict):
                w=op['Write'];blob=w['blob'];raw=redo[blob['offset']:blob['offset']+blob['len']];assert sha(raw)==blob['sha256']
                at=w['target']
                if raw[32:36]==b'NXSB':
                    for cut in range(512,len(raw),512):
                        torn=image[:];torn[at:at+cut]=raw[:cut];save(f'{step:03d}-torn-{cut}',torn,step,'torn-checkpoint')
                image[at:at+len(raw)]=raw
            save(f'{step:03d}',image,step,'completed-prefix')
        run(b,'apply',journal)
        assert sha(base.read_bytes())==sha(image),'trace replay differs from actual apply'
        shutil.rmtree(work)
        (out/'manifest.json').write_text(json.dumps({'scope':scope,'rows':rows,'provenance':provenance},indent=2))
        print(index,action['op'],len(manifest['ops']),flush=True)
    with gzip.open(out/'final.dmg.gz','wb',compresslevel=1) as z:z.write(base.read_bytes())
    base.unlink();print('images',len(rows),flush=True)

def check(a):
    out=a.output.resolve();m=json.loads((out/'manifest.json').read_text());expected=json.loads(a.expected.read_text())['files'];results=[]
    def match(mp,state):
        for name in m['scope']:
            spec=state.get(name);p=mp/name.lstrip('/')
            if spec is None:
                if os.path.lexists(p):return False
            elif spec['kind']=='dir':
                if not p.is_dir() or p.is_symlink():return False
            elif spec['kind']=='symlink':
                if not p.is_symlink() or os.readlink(p)!=spec['target']:return False
            elif not p.is_file() or p.is_symlink() or sha(p.read_bytes())!=spec['sha256']:return False
        return True
    for item in m['rows']:
        row={'image':item['image'],'action':item['action'],'kind':item['kind']}
        with tempfile.TemporaryDirectory(prefix='lapfs-matrix-') as td:
            image=Path(td)/'case.dmg'
            with gzip.open(out/item['image'],'rb') as z:image.write_bytes(z.read())
            if 'image_sha256' in item:assert sha(image.read_bytes())==item['image_sha256']
            r=subprocess.run(['hdiutil','attach','-readonly','-nobrowse','-plist',str(image)],capture_output=True)
            row['attach']=r.returncode
            if not r.returncode:
                entities=plistlib.loads(r.stdout)['system-entities'];parent=next(e['dev-entry'] for e in entities if e.get('content-hint')=='GUID_partition_scheme')
                try:
                    dev=next(e['dev-entry'] for e in entities if e.get('content-hint','').startswith('EF57347C'))
                    fsck=subprocess.run(['/sbin/fsck_apfs','-n',dev],capture_output=True,text=True);log=fsck.stdout+fsck.stderr
                    row['fsck_clean']=fsck.returncode==0 and 'appears to be OK' in log and not any(s in log.lower() for s in ['error:','warning:','overallocation','underallocation'])
                    mp=next((Path(e['mount-point']) for e in entities if e.get('mount-point')),None)
                    row['atomic']=bool(mp) and (match(mp,item['before']) or match(mp,item['after']))
                    row['original_hashes']=bool(mp) and all(sha((mp/n).read_bytes())==v['sha256'] for n,v in expected.items())
                    if not row['fsck_clean']:(out/(item['image']+'.fsck.log')).write_text(log)
                finally:run('hdiutil','detach',parent)
            row['passed']=all(row.get(k) for k in ['fsck_clean','atomic','original_hashes']);results.append(row)
            if not row['passed']:print('FAIL',json.dumps(row),flush=True)
            if len(results)%25==0:print('checked',len(results),'failed',sum(not r['passed'] for r in results),flush=True)
        (out/'mac-results.json').write_text(json.dumps(results,indent=2))
    print('TOTAL',len(results),'FAILED',sum(not r['passed'] for r in results),flush=True)
    raise SystemExit(0 if all(r['passed'] for r in results) else 1)

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('mode',choices=['generate','check']);p.add_argument('--output',type=Path,required=True)
    p.add_argument('--binary',type=Path);p.add_argument('--fixture',type=Path);p.add_argument('--expected',type=Path);a=p.parse_args()
    generate(a) if a.mode=='generate' else check(a)
