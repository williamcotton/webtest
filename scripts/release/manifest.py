#!/usr/bin/env python3
"""Combine every required target only after verifying its archive and raw binary."""
import hashlib, json, pathlib, sys
root=pathlib.Path(sys.argv[1]);targets=['x86_64-apple-darwin','aarch64-apple-darwin','x86_64-unknown-linux-gnu','aarch64-unknown-linux-gnu','x86_64-pc-windows-msvc']
manifest=None
for target in targets:
    part=json.loads((root/(target+'.json')).read_text())
    if manifest is None: manifest={**part,'artifacts':[]}
    for key in ['schema','version']:
        if part[key]!=manifest[key]: raise ValueError('release version skew')
    for key,value in part['compatibility'].items():
        if key not in ['target'] and value!=manifest['compatibility'][key]: raise ValueError('compatibility skew: '+key)
    for artifact in part['artifacts']:
        name=artifact['name']
        if pathlib.PurePath(name).name!=name or '/' in name or '\\' in name: raise ValueError('unsafe artifact name')
        data=(root/name).read_bytes()
        if len(data)!=artifact['bytes'] or hashlib.sha256(data).hexdigest()!=artifact['sha256']: raise ValueError('checksum mismatch')
        manifest['artifacts'].append(artifact)
manifest['compatibility'].pop('target',None)
for file in sorted(root.iterdir()):
    if file.name.endswith(('.tgz','.vsix','.sbom.cdx.json','.rb','.yaml')):
        data=file.read_bytes()
        manifest['artifacts'].append({'name':file.name,'kind':'package','target':'any','bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()})
(root/'manifest.json').write_text(json.dumps(manifest,sort_keys=True,indent=2)+'\n')
(root/'SHA256SUMS').write_text(''.join(f"{a['sha256']}  {a['name']}\n" for a in manifest['artifacts']))
