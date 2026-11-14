#!/usr/bin/env python3
"""Generate reviewable package-manager definitions from an already verified manifest."""
import json,pathlib,re,sys
root=pathlib.Path(sys.argv[1]);m=json.loads((root/'manifest.json').read_text());v=m['version']
if not re.fullmatch(r'\d+\.\d+\.\d+',v) or m['schema']!=1: raise ValueError('unsupported release')
base=f'https://github.com/williamcotton/webtest/releases/download/v{v}/'
def artifact(target,kind):
    items=[a for a in m['artifacts'] if a['target']==target and a['kind']==kind]
    if len(items)!=1: raise ValueError('ambiguous release')
    a=items[0]
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]+',a['name']) or not re.fullmatch(r'[a-f0-9]{64}',a['sha256']): raise ValueError('unsafe metadata')
    return a
formula=['class Webtest < Formula','  desc "Statically analyzable web system tests"','  homepage "https://github.com/williamcotton/webtest"',f'  version "{v}"','  license "MIT"']
for os_name,suffix in [('macos','apple-darwin'),('linux','unknown-linux-gnu')]:
    formula.append(f'  on_{os_name} do')
    for arch,target in [('intel','x86_64'),('arm','aarch64')]:
        a=artifact(f'{target}-{suffix}','archive')
        formula.extend([f'    on_{arch} do',f'      url "{base}{a["name"]}"',f'      sha256 "{a["sha256"]}"','    end'])
    formula.append('  end')
formula.extend(['  def install','    bin.install "webtest"','  end','  test do','    assert_match version.to_s, shell_output("#{bin}/webtest --version")','  end','end'])
(root/'webtest.rb').write_text('\n'.join(formula)+'\n')
a=artifact('x86_64-pc-windows-msvc','binary')
winget={'PackageIdentifier':'WebTest.WebTest','PackageVersion':v,'PackageLocale':'en-US','Publisher':'WebTest','PackageName':'WebTest','License':'MIT','ShortDescription':'Statically analyzable web system tests','PackageUrl':'https://github.com/williamcotton/webtest','InstallerType':'portable','Commands':['webtest'],'Installers':[{'Architecture':'x64','InstallerUrl':base+a['name'],'InstallerSha256':a['sha256'].upper()}],'ManifestType':'singleton','ManifestVersion':'1.9.0'}
# JSON is valid YAML; package repositories may reformat these reviewed projections.
(root/'WebTest.WebTest.yaml').write_text(json.dumps(winget,indent=2)+'\n')
