#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_dir"
release_build=false
if [[ $# -gt 0 ]]; then
    if [[ $# -ne 1 || "$1" != --release ]]; then
        echo 'Usage: tools/package_macos.sh [--release]' >&2
        exit 2
    fi
    release_build=true
    if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
        echo 'Release builds require a clean Git working tree.' >&2
        exit 1
    fi
fi
if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
    echo 'This package script builds macOS Apple Silicon applications.' >&2
    exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=11.0
mkdir -p -- build/swift-cache dist
version=$(python3 tools/macos_release.py version)
package_name="atomic-bomberman-$version-macos-arm64"
cargo build --locked --release -p bomberman -p bomberman-assets
cargo build --locked --release -p bomberman --example hardware_check --example audio_probe
xcrun swiftc -swift-version 5 -parse-as-library -O -target arm64-apple-macosx11.0 \
    -module-cache-path "$project_dir/build/swift-cache" \
    tools/macos-launcher.swift -o build/AtomicBomberman
xcrun swift -swift-version 5 -module-cache-path "$project_dir/build/swift-cache" \
    tools/macos-icon.swift build/AtomicBomberman.iconset
iconutil -c icns build/AtomicBomberman.iconset -o build/AtomicBomberman.icns

stage=$(mktemp -d "$project_dir/dist/.macos-package.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/payload"
app="$stage/payload/Atomic Bomberman.app"
mkdir -p -- "$app/Contents/MacOS" "$app/Contents/Resources/docs" "$app/Contents/Resources/licenses"
install -m755 build/AtomicBomberman target/release/bomberman target/release/bomberman-assets "$app/Contents/MacOS/"
install -m755 target/release/examples/hardware_check "$app/Contents/MacOS/hardware-check"
install -m755 target/release/examples/audio_probe "$app/Contents/MacOS/audio-probe"
install -m644 build/AtomicBomberman.icns LICENSE "$app/Contents/Resources/"
install -m644 docs/INSTALL-macos.md docs/macos-handoff.md docs/asset-formats.md docs/behavior-ledger.md "$app/Contents/Resources/docs/"
install -m644 docs/INSTALL-macos.md "$stage/payload/README.txt"
install -m644 vendor/quad-snd/NOTICE.md vendor/quad-snd/LICENSE-APACHE "$app/Contents/Resources/licenses/"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.andrei.atomic-bomberman</string>
<key>CFBundleName</key><string>Atomic Bomberman</string>
<key>CFBundleDisplayName</key><string>Atomic Bomberman</string>
<key>CFBundleExecutable</key><string>AtomicBomberman</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleIconFile</key><string>AtomicBomberman.icns</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>LSApplicationCategoryType</key><string>public.app-category.games</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
# The CLI tools use system frameworks only. Reject accidental local dylibs.
for executable in "$app/Contents/MacOS/"*; do
    otool -L "$executable" | tail -n +2 | awk '{print $1}' | while IFS= read -r dependency; do
        case "$dependency" in
            /System/Library/*|/usr/lib/*|@rpath/libswift*.dylib) ;;
            *) echo "Unexpected non-system dependency: $dependency" >&2; exit 1 ;;
        esac
    done
    codesign --force --sign - "$executable"
done
if [[ "$release_build" == true ]]; then
    python3 tools/macos_release.py metadata "$app" --release
else
    python3 tools/macos_release.py metadata "$app"
fi
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
python3 tools/macos_release.py manifest "$app" "$stage/payload/BUILD-INFO.json"
"$app/Contents/MacOS/AtomicBomberman" --help > /dev/null
ditto -c -k --norsrc --noextattr --noqtn "$stage/payload" "$stage/$package_name.zip"
install -m644 "$stage/payload/BUILD-INFO.json" "$stage/$package_name.build.json"
(
    cd -- "$stage"
    shasum -a 256 "$package_name.zip" > "$package_name.zip.sha256"
)
if [[ "$release_build" == true ]]; then
    python3 tools/macos_release.py verify "$stage/$package_name.zip" --release
else
    python3 tools/macos_release.py verify "$stage/$package_name.zip"
fi
# Only generated artifacts with these fixed names are replaced.
rm -rf -- "$project_dir/dist/Atomic Bomberman.app"
mv -- "$app" "$project_dir/dist/Atomic Bomberman.app"
mkdir -p "$project_dir/dist/github"
mv -- "$stage/$package_name.zip" "$stage/$package_name.zip.sha256" \
    "$stage/$package_name.build.json" "$project_dir/dist/github/"
echo "Created dist/github/$package_name.zip and dist/Atomic Bomberman.app"
