#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' '2db75c40782f5e8ba1fc278a5574bab070adccb2d21ca5a6e5ed840888448046  apps/android/gradle/wrapper/gradle-wrapper.jar' | sha256sum -c -
grep -Fxq 'distributionSha256Sum=f397b287023acdba1e9f6fc5ea72d22dd63669d59ed4a289a29b1a76eee151c6' apps/android/gradle/wrapper/gradle-wrapper.properties
test -x apps/android/gradlew
test -s apps/android/gradlew.bat
bash -n apps/android/gradlew
