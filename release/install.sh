#!/bin/sh
set -eu
version=${1:?usage: install.sh VERSION [PREFIX] (requires curl, cosign, python3)}
prefix=${2:-"$HOME/.local/bin"}
case "$version" in *[!0-9.]*|''|.*|*.) echo 'Expected exact x.y.z version' >&2; exit 2;; esac
case "$(uname -s)/$(uname -m)" in
  Darwin/x86_64) target=x86_64-apple-darwin;; Darwin/arm64) target=aarch64-apple-darwin;;
  Linux/x86_64) target=x86_64-unknown-linux-gnu;; Linux/aarch64) target=aarch64-unknown-linux-gnu;;
  *) echo 'Unsupported platform; build from source.' >&2; exit 2;;
esac
for tool in curl cosign python3; do command -v "$tool" >/dev/null; done
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
base="https://github.com/williamcotton/webtest/releases/download/v$version"
curl --proto '=https' --tlsv1.2 -fLsS --max-time 120 --max-filesize 1048576 "$base/manifest.json" -o "$tmp/manifest.json"
curl --proto '=https' --tlsv1.2 -fLsS --max-time 120 --max-filesize 1048576 "$base/manifest.sigstore.json" -o "$tmp/manifest.sigstore.json"
cosign verify-blob --bundle "$tmp/manifest.sigstore.json" --certificate-oidc-issuer https://token.actions.githubusercontent.com --certificate-identity "https://github.com/williamcotton/webtest/.github/workflows/release.yml@refs/tags/v$version" "$tmp/manifest.json" >/dev/null
python3 - "$tmp" "$version" "$target" "$base" <<'PY'
import hashlib,json,pathlib,re,sys,urllib.request
root=pathlib.Path(sys.argv[1]);version,target,base=sys.argv[2:]
if not re.fullmatch(r'\d+\.\d+\.\d+',version): raise ValueError('exact x.y.z version required')
m=json.loads((root/'manifest.json').read_bytes())
if m['schema']!=1 or m['version']!=version: raise ValueError('unsupported manifest')
artifacts=[a for a in m['artifacts'] if a['kind']=='binary' and a['target']==target]
if len(artifacts)!=1: raise ValueError('missing or ambiguous platform binary')
a=artifacts[0]
if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._-]{0,180}',a['name']) or not 0<a['bytes']<=268435456: raise ValueError('unsafe artifact')
with urllib.request.urlopen(base+'/'+a['name'],timeout=120) as response:
    if not response.url.startswith('https://'): raise ValueError('insecure redirect')
    data=response.read(a['bytes']+1)
if len(data)!=a['bytes'] or hashlib.sha256(data).hexdigest()!=a['sha256']: raise ValueError('checksum mismatch')
(root/'webtest').write_bytes(data)
PY
mkdir -p "$prefix"
staged=$(mktemp "$prefix/.webtest-install.XXXXXXXX")
trap 'rm -rf "$tmp"; rm -f "$staged"' EXIT HUP INT TERM
cp "$tmp/webtest" "$staged"
chmod 755 "$staged"
mv -f "$staged" "$prefix/webtest"
printf 'Installed WebTest %s at %s/webtest\n' "$version" "$prefix"
