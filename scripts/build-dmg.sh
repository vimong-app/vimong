#!/usr/bin/env bash
# `npm run tauri build`의 dmg 생성 단계는 이 프로젝트 경로(/Volumes/DATA 볼륨)에서
# hdiutil이 "No space left on device"로 항상 실패한다(로컬 hdiutil 버그, 실제 용량과 무관 —
# srcfolder가 /Volumes/DATA에 있으면 재현되고 내장 디스크에 있으면 정상 동작함).
# Finder 창 꾸미기 AppleScript도 Automation 권한이 없어 실패한다.
#
# 먼저 평소대로 전체 빌드를 시도한다. .app 번들링은 성공하고, dmg 생성이
# hdiutil 단계에서 실패하지만 그 실패 이전에 bundle_dmg.sh와 지원 리소스가
# target/release/bundle/ 아래에 만들어져 남는다. 그걸 재사용해서,
# srcfolder만 내장 디스크로 복사하고 --sandbox-safe(Finder 꾸미기 생략)로
# 직접 다시 실행해 우회한다.
set -uo pipefail
cd "$(dirname "$0")/.."

npm run tauri build || true

BUNDLE_DIR="src-tauri/target/release/bundle"
SCRIPT="$BUNDLE_DIR/dmg/bundle_dmg.sh"
if [[ ! -f "$SCRIPT" ]]; then
  echo "error: $SCRIPT 가 없습니다. 'npm run tauri build'가 .app 빌드 단계에서부터 실패한 것 같습니다." >&2
  exit 1
fi

PRODUCT=$(sed -n 's/.*"productName": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1)
# 버전은 package.json이 단일 소스다 — tauri.conf.json의 "version"은 "../package.json"
# 경로 문자열만 가리키고 있어서(Tauri가 빌드 시점에 그 파일을 읽어 반영), 여기서도
# package.json을 직접 읽어야 한다.
VERSION=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' package.json | head -1)
ARCH=$(uname -m | sed 's/arm64/aarch64/; s/x86_64/x64/')
DMG_NAME="${PRODUCT}_${VERSION}_${ARCH}.dmg"

TMP=$(mktemp -d "/tmp/${PRODUCT}-dmg.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/macos"
cp -R "$BUNDLE_DIR/macos/${PRODUCT}.app" "$TMP/macos/"

rm -f "$BUNDLE_DIR/dmg/$DMG_NAME"
set -e
bash "$SCRIPT" \
  --volname "$PRODUCT" \
  --icon-size 128 --icon "${PRODUCT}.app" 180 170 \
  --app-drop-link 480 170 \
  --window-size 660 400 \
  --hide-extension "${PRODUCT}.app" \
  --sandbox-safe \
  "$PWD/$BUNDLE_DIR/dmg/$DMG_NAME" \
  "$TMP/macos"

echo "dmg 생성 완료: $BUNDLE_DIR/dmg/$DMG_NAME"

# 이 dmg는 `npm run tauri build`의 표준 흐름 밖에서 만들어지므로, .app이
# tauri.conf.json의 signingIdentity로 이미 서명·공증됐더라도 그 stapling이
# dmg 자체에는 붙지 않는다. 배포용이면 dmg도 별도로 공증해야 한다.
#
# Apple ID 로그인 비밀번호 대신 앱 암호(appleid.apple.com에서 발급)를 아래처럼
# 키체인에 한 번만 등록해두면(비밀번호가 코드/파일에 남지 않는다), 이후로는
# 프로필 이름만 참조한다:
#   xcrun notarytool store-credentials "vimong-notary" \
#     --apple-id "<Apple ID>" --team-id "<Team ID>" --password "<앱 암호>"
NOTARY_PROFILE="${APPLE_NOTARY_PROFILE:-vimong-notary}"
DMG_PATH="$PWD/$BUNDLE_DIR/dmg/$DMG_NAME"

set +e
NOTARY_OUT=$(xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait 2>&1)
NOTARY_STATUS=$?
set -e

if [[ $NOTARY_STATUS -eq 0 ]]; then
  echo "$NOTARY_OUT"
  xcrun stapler staple "$DMG_PATH"
  echo "공증 완료: $DMG_PATH"
elif echo "$NOTARY_OUT" | grep -qi "keychain"; then
  echo "키체인 프로필 '$NOTARY_PROFILE' 없음 — 공증 생략 (로컬 테스트용 dmg)"
else
  echo "$NOTARY_OUT" >&2
  echo "공증 실패 (dmg는 이미 생성됨, 공증만 실패): $DMG_PATH" >&2
fi
