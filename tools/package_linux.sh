#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_dir"
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
    echo 'This package script targets Linux x86-64.' >&2
    exit 1
fi
cargo build --locked --release -p bomberman -p bomberman-assets
cargo build --locked --release -p bomberman --example hardware_check --example audio_probe
package_name=atomic-bomberman-linux-x86_64
package_dir="$project_dir/dist/$package_name"
mkdir -p -- "$package_dir/docs/evidence" "$package_dir/licenses" "$package_dir/tools"
install -m755 target/release/bomberman target/release/bomberman-assets "$package_dir/"
install -m755 target/release/examples/hardware_check "$package_dir/hardware-check"
install -m755 target/release/examples/audio_probe "$package_dir/audio-probe"
install -m755 tools/hardware-check.py "$package_dir/tools/"
install -m644 README.md LICENSE "$package_dir/"
install -m644 docs/behavior-ledger.md docs/asset-formats.md docs/hardware-check.md "$package_dir/docs/"
install -m644 docs/evidence/*.json "$package_dir/docs/evidence/"
install -m644 vendor/quad-snd/NOTICE.md "$package_dir/licenses/quad-snd-NOTICE.md"
install -m644 vendor/quad-snd/LICENSE-APACHE "$package_dir/licenses/quad-snd-LICENSE-APACHE"
cat > "$package_dir/run.sh" <<'EOF'
#!/usr/bin/env sh
set -eu
install_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$install_dir/bomberman" "$@"
EOF
chmod 755 "$package_dir/run.sh"
tar -C "$project_dir/dist" -czf "$project_dir/dist/$package_name.tar.gz" "$package_name"
(
    cd -- "$project_dir/dist"
    sha256sum "$package_name.tar.gz" > "$package_name.tar.gz.sha256"
)
echo "Created dist/$package_name.tar.gz (import original assets separately)."
