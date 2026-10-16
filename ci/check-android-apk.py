#!/usr/bin/env python3
"""Check the two native bridge libraries for each supported APK ABI."""
import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path
from zipfile import ZipFile

ABIS = ('arm64-v8a', 'armeabi-v7a', 'x86_64')
ELF_ARCHITECTURES = {'arm64-v8a': (2, 183), 'armeabi-v7a': (1, 40), 'x86_64': (2, 62)}
LIBRARIES = ('libtrollm_uniffi.so', 'libjnidispatch.so')


def validate_release(apk, aapt, apksigner, expected_signer):
    if not expected_signer or not re.fullmatch('[a-f0-9]{64}', expected_signer):
        raise ValueError('Release requires an expected SHA256 signing certificate')
    if not aapt or not apksigner:
        raise ValueError('Release requires aapt and apksigner')
    def tool(command, name, env=None):
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=30, env=env)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise ValueError(f'Release {name} validation could not run') from error
        if result.returncode:
            raise ValueError(f'Release {name} validation failed')
        return result.stdout
    badging = tool([str(aapt), 'dump', 'badging', str(apk)], 'manifest')
    package = next((line for line in badging.splitlines() if line.startswith('package: ')), '')
    attributes = dict(re.findall(r"([A-Za-z]+)='([^']*)'", package))
    if (attributes.get('name') != 'com.trollm.app'
            or attributes.get('versionCode') != '2'
            or attributes.get('versionName') != '0.1.0'):
        raise ValueError('Release package/version does not match com.trollm.app 0.1.0 (2)')
    if any(line.strip() == 'application-debuggable' for line in badging.splitlines()):
        raise ValueError('Release APK must not be debuggable')
    certificates = tool([str(apksigner), 'verify', '--verbose', '--print-certs', str(apk)],
                        'signature', dict(os.environ, JAVA_TOOL_OPTIONS='-Xmx256m'))
    signers = re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([a-fA-F0-9]{64})$',
                         certificates, re.MULTILINE)
    if len(signers) != 1 or signers[0].lower() != expected_signer:
        raise ValueError('Release APK signing certificate does not match the project identity')
    return {'applicationId': attributes['name'], 'versionCode': 2, 'versionName': '0.1.0',
            'debuggable': False, 'certificateSha256': signers[0].lower()}


def validate(apk, backend='stub'):
    if backend not in ('stub', 'llama'):
        raise ValueError('Android backend must be stub or llama')
    libraries = LIBRARIES + (('libc++_shared.so',) if backend == 'llama' else ())
    with ZipFile(apk) as archive:
        for abi in ABIS:
            for library in libraries:
                name = f'lib/{abi}/{library}'
                if archive.getinfo(name).file_size == 0:
                    raise ValueError(f'Empty APK native library: {name}')
                # Read only the ELF identification/type/machine fields. All
                # supported Android ABIs use little-endian ELF version 1.
                with archive.open(name) as stream:
                    header = stream.read(20)
                elf_class, machine = ELF_ARCHITECTURES[abi]
                if (len(header) != 20 or header[:4] != b'\x7fELF'
                        or header[4] != elf_class or header[5:7] != b'\x01\x01'
                        or int.from_bytes(header[18:20], 'little') != machine):
                    raise ValueError(f'Invalid ELF architecture for {abi}: {name}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', type=Path)
    parser.add_argument('--build-info', type=Path)
    parser.add_argument('--backend', choices=('stub', 'llama'), default='stub')
    parser.add_argument('--build-type', choices=('debug', 'release'), default='debug')
    parser.add_argument('--aapt', type=Path)
    parser.add_argument('--apksigner', type=Path)
    parser.add_argument('--expected-signer-sha256')
    args = parser.parse_args()
    # Never leave a stale provenance artifact if packaging validation fails.
    if args.build_info:
        args.build_info.unlink(missing_ok=True)
    checksum = args.apk.with_name(args.apk.name + '.sha256')
    checksum.unlink(missing_ok=True)
    if args.build_type == 'release' and args.backend != 'llama':
        raise ValueError('Release APK must use the llama backend')
    validate(args.apk, args.backend)
    release = (validate_release(args.apk, args.aapt, args.apksigner, args.expected_signer_sha256)
               if args.build_type == 'release' else {})
    if args.build_info:
        commit = os.environ['CI_COMMIT_SHA']
        digest = hashlib.sha256()
        with args.apk.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        info = {'schemaVersion': 1, 'sourceCommit': commit, 'backend': args.backend,
                'buildType': args.build_type,
                'apkSha256': digest.hexdigest(), 'minSdk': 33,
                'supportedAbis': list(ABIS),
                'pipelineId': os.environ.get('CI_PIPELINE_ID'),
                'jobId': os.environ.get('CI_JOB_ID'), **release}
        args.build_info.write_text(json.dumps(info, indent=2) + '\n')
        checksum.write_text(f'{digest.hexdigest()}  {args.apk.name}\n')
    print(f'APK ELF packaging valid for {args.backend} on all three supported ABIs')
