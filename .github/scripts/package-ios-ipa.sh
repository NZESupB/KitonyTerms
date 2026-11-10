#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
[ -n "${MOBILE_BUILD_NUMBER:-}" ] && [ -n "${MOBILE_VERSION_LABEL:-}" ]
output="${RUNNER_TEMP:?}/kitonyterms-ios-${GITHUB_RUN_ID:?}-${GITHUB_RUN_ATTEMPT:?}"
(
  cd app
  python3 -B scripts/ios_ipa.py build --build-number "$MOBILE_BUILD_NUMBER" --output "$output"
)
export KITONYTERMS_IOS_RELEASE_OUTPUT="$output"
python3 -B - <<'PY'
import hashlib, json, os, shutil, zipfile, plistlib
from pathlib import Path
root = Path.cwd()
version = json.loads((root/'app/frontend/package.json').read_text(encoding='utf-8'))['version']
label = os.environ['MOBILE_VERSION_LABEL']
if label != 'v'+version: raise ValueError('iOS发布标签不匹配')
source = Path(os.environ['KITONYTERMS_IOS_RELEASE_OUTPUT'])/'kitonyterms-unsigned.ipa'
dist = root/'dist'
dist.mkdir(exist_ok=True)
destination = dist/('kitonyterms-'+label+'-ios-aarch64-unsigned.ipa')
if destination.exists(): raise ValueError('拒绝覆盖IPA')
with zipfile.ZipFile(source) as archive:
    info = plistlib.loads(archive.read('Payload/KitonyTerms.app/Info.plist'))
    if info['CFBundleShortVersionString'] != version or info['CFBundleVersion'] != os.environ['MOBILE_BUILD_NUMBER']: raise ValueError('IPA版本不符')
shutil.copyfile(source, destination)
record = dict(version=version, buildNumber=int(os.environ['MOBILE_BUILD_NUMBER']), commit=os.environ['GITHUB_SHA'], build=int(os.environ['GITHUB_RUN_ID']), attempt=int(os.environ['GITHUB_RUN_ATTEMPT']), signing='unsigned', deviceExecutionVerified=False, files={destination.name: dict(size=destination.stat().st_size, sha256=hashlib.sha256(destination.read_bytes()).hexdigest())})
(dist/'verification-ios.json').write_text(json.dumps(record, indent=2)+'\n', encoding='utf-8')
PY
