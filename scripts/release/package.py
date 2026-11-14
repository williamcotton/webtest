#!/usr/bin/env python3
"""Build deterministic archives and raw-install records from a tested native binary."""
import argparse, gzip, hashlib, io, json, os, pathlib, subprocess, tarfile, zipfile
ROOT = pathlib.Path(__file__).resolve().parents[2]
def run(*args):
    return subprocess.check_output(args, cwd=ROOT)
def package(binary, target, output):
    compatibility = json.loads(run(str(binary.resolve()), '--version', '--json'))
    if compatibility['target'] != target:
        raise ValueError('binary target does not match requested artifact target')
    version = compatibility['webtest']
    output.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--format-version', '1'))
    notices = '\n'.join(sorted(f"{p['name']} {p['version']}: {p.get('license')} {p.get('repository')}" for p in metadata['packages'])) + '\n'
    sbom = {'bomFormat':'CycloneDX','specVersion':'1.5','version':1,'components':[{'type':'library','name':p['name'],'version':p['version'],'purl':f"pkg:cargo/{p['name']}@{p['version']}"} for p in sorted(metadata['packages'],key=lambda p:(p['name'],p['version']))]}
    contents = {
        'webtest.exe' if target.endswith('msvc') else 'webtest': binary.read_bytes(),
        'LICENSE': (ROOT/'LICENSE').read_bytes(),
        'README.md': (ROOT/'release/README.md').read_bytes(),
        'THIRD_PARTY_NOTICES': notices.encode(),
        'sbom.cdx.json': (json.dumps(sbom,sort_keys=True,indent=2)+'\n').encode(),
        'compatibility.json': (json.dumps(compatibility,sort_keys=True,indent=2)+'\n').encode(),
        'Cargo.lock': (ROOT/'Cargo.lock').read_bytes(),
        'toolchain.txt': run('rustc','-vV'),
        'chrome-metadata.rs': (ROOT/'crates/browser-manager/src/lib.rs').read_bytes(),
    }
    stem = f'webtest-{version}-{target}'
    raw_name = stem + ('.exe' if target.endswith('msvc') else '.bin')
    (output/raw_name).write_bytes(binary.read_bytes())
    archive = output/(stem + ('.zip' if target.endswith('msvc') else '.tar.gz'))
    if target.endswith('msvc'):
        with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as z:
            for name,data in sorted(contents.items()):
                info=zipfile.ZipInfo(stem+'/'+name,date_time=(1980,1,1,0,0,0));info.external_attr=(0o100755 if name.startswith('webtest') else 0o100644)<<16
                z.writestr(info,data,compress_type=zipfile.ZIP_DEFLATED,compresslevel=9)
    else:
        with archive.open('wb') as out, gzip.GzipFile(fileobj=out,mode='wb',mtime=0,filename='') as gz, tarfile.open(fileobj=gz,mode='w') as tar:
            for name,data in sorted(contents.items()):
                info=tarfile.TarInfo(stem+'/'+name);info.size=len(data);info.mode=0o755 if name=='webtest' else 0o644;info.mtime=0
                tar.addfile(info,io.BytesIO(data))
    records=[]
    for file,kind in [(output/raw_name,'binary'),(archive,'archive')]:
        data=file.read_bytes();records.append({'name':file.name,'target':target,'kind':kind,'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)})
    (output/(target+'.json')).write_text(json.dumps({'schema':1,'version':version,'compatibility':compatibility,'artifacts':records},sort_keys=True,indent=2)+'\n')
    (output/(target+'.sbom.cdx.json')).write_bytes(contents['sbom.cdx.json'])
    return records
if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=pathlib.Path,required=True);parser.add_argument('--target',required=True);parser.add_argument('--output',type=pathlib.Path,default=ROOT/'release/dist')
    args=parser.parse_args();package(args.binary,args.target,args.output)
