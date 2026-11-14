import importlib.util, pathlib, tempfile, unittest, subprocess, json, hashlib, tarfile
spec=importlib.util.spec_from_file_location('package',pathlib.Path(__file__).with_name('package.py'))
package=importlib.util.module_from_spec(spec);spec.loader.exec_module(package)
class PackageTests(unittest.TestCase):
    def test_reproducible_package_and_compatibility(self):
        with tempfile.TemporaryDirectory() as d:
            root=pathlib.Path(d);binary=root/'webtest'
            binary.write_text('#!/bin/sh\nprintf \'%s\\n\' \'{"target":"test-target","webtest":"0.1.0"}\'\n');binary.chmod(0o755)
            one=package.package(binary,'test-target',root/'one');two=package.package(binary,'test-target',root/'two')
            self.assertEqual(one,two)
            with tarfile.open(root/'one'/one[1]['name']) as tar:
                names=tar.getnames()
                self.assertTrue(any(n.endswith('/compatibility.json') for n in names))
                self.assertTrue(any(n.endswith('/sbom.cdx.json') for n in names))
                self.assertTrue(all('/../' not in n and not n.startswith('/') for n in names))
            with self.assertRaises(ValueError): package.package(binary,'wrong-target',root/'wrong')
    def test_complete_manifest_and_package_manager_projections(self):
        with tempfile.TemporaryDirectory() as d:
            root=pathlib.Path(d)
            targets=['x86_64-apple-darwin','aarch64-apple-darwin','x86_64-unknown-linux-gnu','aarch64-unknown-linux-gnu','x86_64-pc-windows-msvc']
            for target in targets:
                artifacts=[]
                for kind,extension in [('binary','.exe' if target.endswith('msvc') else '.bin'),('archive','.zip' if target.endswith('msvc') else '.tar.gz')]:
                    name=f'webtest-0.1.0-{target}{extension}';data=name.encode();(root/name).write_bytes(data)
                    artifacts.append({'name':name,'kind':kind,'target':target,'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)})
                (root/(target+'.json')).write_text(json.dumps({'schema':1,'version':'0.1.0','compatibility':{'target':target,'plan_format':10},'artifacts':artifacts}))
            subprocess.check_call(['python3',str(pathlib.Path(__file__).with_name('manifest.py')),d])
            subprocess.check_call(['python3',str(pathlib.Path(__file__).with_name('package_managers.py')),d])
            self.assertIn('on_arm',(root/'webtest.rb').read_text())
            self.assertEqual(json.loads((root/'WebTest.WebTest.yaml').read_text())['InstallerType'],'portable')
            (root/'webtest-0.1.0-x86_64-apple-darwin.bin').write_bytes(b'tampered')
            result=subprocess.run(['python3',str(pathlib.Path(__file__).with_name('manifest.py')),d],capture_output=True)
            self.assertNotEqual(result.returncode,0)
if __name__=='__main__': unittest.main()
