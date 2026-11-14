#!/usr/bin/env python3
"""Run the binary from an archive produced by the current build (not arbitrary input)."""
import argparse,json,pathlib,subprocess,tarfile,tempfile,zipfile
p=argparse.ArgumentParser();p.add_argument('directory',type=pathlib.Path);p.add_argument('target');a=p.parse_args()
m=json.loads((a.directory/(a.target+'.json')).read_text());record=next(x for x in m['artifacts'] if x['kind']=='archive')
with tempfile.TemporaryDirectory() as temp:
    if record['name'].endswith('.zip'):
        with zipfile.ZipFile(a.directory/record['name']) as z:
            for name in z.namelist():
                if pathlib.PurePosixPath(name).is_absolute() or '..' in pathlib.PurePosixPath(name).parts or '\\' in name: raise ValueError('unsafe archive')
            z.extractall(temp)
    else:
        with tarfile.open(a.directory/record['name']) as tar: tar.extractall(temp,filter='data')
    binary=next(pathlib.Path(temp).rglob('webtest.exe' if a.target.endswith('msvc') else 'webtest'))
    version=json.loads(subprocess.check_output([str(binary),'--version','--json']))
    if version!=m['compatibility']: raise ValueError('archive compatibility mismatch')
