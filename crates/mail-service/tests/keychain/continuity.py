#!/usr/bin/env python3
"""Run explicitly on macOS. Uses only disposable keychains and synthetic signing keys.
No certificate trust changes. Restores the user's original search list in finally.
"""
import pathlib, subprocess, tempfile, shlex, hashlib, sys

def run(*args, ok=True):
    result = subprocess.run(args, capture_output=True, text=True, timeout=45)
    if ok and result.returncode:
        raise RuntimeError(f'{args[0]} failed: {result.stderr.strip()} {result.stdout.strip()}')
    return result

if sys.platform != 'darwin':
    raise SystemExit('macOS required')
original = shlex.split(run('/usr/bin/security','list-keychains','-d','user').stdout)
source = pathlib.Path(__file__).with_suffix('.c')
with tempfile.TemporaryDirectory(prefix='email-keychain-acceptance-') as directory:
    root = pathlib.Path(directory)
    signing = str(root/'signing.keychain-db')
    store = str(root/'credentials.keychain-db')
    adhoc_store = str(root/'adhoc.keychain-db')
    partition_store = str(root/'partitions.keychain-db')
    try:
        run('/usr/bin/security','create-keychain','-p','synthetic-signing-password',signing)
        run('/usr/bin/security','unlock-keychain','-p','synthetic-signing-password',signing)
        config = root/'certificate.cnf'
        config.write_text('[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=Email Disposable Continuity Test\n[ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n')
        run('/usr/bin/openssl','req','-new','-newkey','rsa:2048','-nodes','-x509','-days','1','-config',str(config),'-keyout',str(root/'key.pem'),'-out',str(root/'cert.pem'))
        run('/usr/bin/openssl','pkcs12','-export','-inkey',str(root/'key.pem'),'-in',str(root/'cert.pem'),'-out',str(root/'identity.p12'),'-passout','pass:synthetic-import-password')
        run('/usr/bin/security','import',str(root/'identity.p12'),'-k',signing,'-P','synthetic-import-password','-T','/usr/bin/codesign')
        for version, identifier in [('A','dev.email.synthetic.continuity'),('B','dev.email.synthetic.continuity'),('C','dev.email.synthetic.other')]:
            binary = str(root/version)
            run('/usr/bin/clang','-Wno-deprecated-declarations',f'-DBUILD_LABEL="{version}"',str(source),'-framework','Security','-framework','CoreFoundation','-o',binary)
            run('/usr/bin/codesign','--force','--sign','Email Disposable Continuity Test','--keychain',signing,'--identifier',identifier,binary)
            run('/usr/bin/codesign','--verify','--strict',binary)
        a,b,c = (str(root/v) for v in 'ABC')
        ra = run('/usr/bin/codesign','-dr','-',a)
        dra = ra.stdout + ra.stderr
        rb = run('/usr/bin/codesign','-dr','-',b)
        drb = rb.stdout + rb.stderr
        # codesign emits a path line plus DR; compare only the requirement.
        dra = next(line for line in dra.splitlines() if line.startswith('designated =>'))
        drb = next(line for line in drb.splitlines() if line.startswith('designated =>'))
        assert dra == drb
        def cdhash(binary):
            result=run('/usr/bin/codesign','-d','--verbose=4',binary)
            return next(line.split('=',1)[1] for line in (result.stdout+result.stderr).splitlines() if line.startswith('CDHash='))
        hash_a,hash_b=cdhash(a),cdhash(b)
        assert hash_a != hash_b
        print(f'A CDHash={hash_a}, B CDHash={hash_b}')
        assert hashlib.sha256(pathlib.Path(a).read_bytes()).digest() != hashlib.sha256(pathlib.Path(b).read_bytes()).digest()
        print('A/B different bytes, identical certificate-bound designated requirement:',dra)
        for binary, command, expected in [(a,'create',True),(a,'read-old',True),(b,'read-old',True),(b,'migrate',True),(b,'read',True),(b,'read',True),(c,'read',False),(b,'lock',True),(b,'read',False),(b,'unlock',True),(b,'read',True)]:
            result=run(binary,command,store,ok=False)
            print(result.stdout.strip())
            assert (result.returncode==0)==expected,(command,result.stdout,result.stderr)
            if not expected:
                assert int(result.stdout.rsplit('status=',1)[1]) in (-25293,-25308,-25315),result.stdout
        # Historical-owner migration fixture. The separate test keychain does
        # not reproduce login-Keychain XARA enforcement for equal DRs. Use a
        # foreign original owner to enforce a denied historical read, then test
        # destination ownership separately. Only synthetic items are changed.
        run(c,'create-v3',partition_store)
        for account in ('mail','calendar','booking'):
            run('/usr/bin/security','set-generic-password-partition-list',
                '-a',account,'-s','email.synthetic.v3','-S','cdhash:'+cdhash(c),
                '-k','synthetic-keychain-password',partition_store)
        for binary, command, expected in [
            (c,'read-v3',True), (b,'read-v3',False),
            (b,'adopt-v4',True), (b,'read-v4',True), (b,'read-v4',True),
            (c,'read-v3',True), (b,'read-v3',False), (c,'read-v4',False),
        ]:
            result=run(binary,command,partition_store,ok=False)
            print(result.stdout.strip())
            assert (result.returncode==0)==expected,(command,result.stdout,result.stderr)
        print('PASS: historical owner rejection; newly owned copy reads silently in fresh processes; original owner access retained')
        for version in ('E','F'):
            binary = str(root/version)
            run('/usr/bin/clang','-Wno-deprecated-declarations',f'-DBUILD_LABEL="{version}"',str(source),'-framework','Security','-framework','CoreFoundation','-o',binary)
            run('/usr/bin/codesign','--force','--sign','-','--identifier','dev.email.synthetic.adhoc',binary)
        for version,command,expected in [('E','create',True),('E','read-old',True),('F','read-old',False)]:
            result=run(str(root/version),command,adhoc_store,ok=False)
            print(result.stdout.strip())
            assert (result.returncode==0)==expected,(command,result.stdout,result.stderr)
            if not expected:
                assert int(result.stdout.rsplit('status=',1)[1]) in (-25293,-25308,-25315),result.stdout
        print('PASS: A-to-B continuity, migration, fresh-process reads, foreign identity rejection, locked-store rejection, unlock recovery and ad-hoc upgrade rejection')
    finally:
        # Delete only the two disposable keychains, then restore even if creation,
        # signing or an assertion failed. No default-keychain changes are made.
        for keychain in (store, adhoc_store, partition_store, signing):
            if pathlib.Path(keychain).exists():
                run('/usr/bin/security','delete-keychain',keychain,ok=False)
        run('/usr/bin/security','list-keychains','-d','user','-s',*original)
        assert shlex.split(run('/usr/bin/security','list-keychains','-d','user').stdout)==original
