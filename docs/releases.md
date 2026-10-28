# macOS release procedure

Release binaries belong in GitHub Releases. Keep `dist/`, imported assets, disc
images, credentials, and local build/cache directories out of source commits.
The Mac package contains the app, installation README, project/dependency
licenses, and build provenance; users supply the original game data separately.

## Prepare and verify

1. Set the workspace version in `Cargo.toml` when making a new version. The app
   bundle and ZIP name derive from the game package version. Run `tools/check.sh`
   and `tools/test_macos_launcher.sh`, then review and commit the intended source.
2. Build from the clean commit on an Apple Silicon Mac:

   ```bash
   tools/package_macos.sh --release
   python3 tools/macos_release.py verify \
     dist/github/atomic-bomberman-0.1.0-macos-arm64.zip --release
   ```

   Verification checks the signed bundle, architecture, executable hashes,
   checksum, metadata consistency, install README, relocated launcher, licenses,
   and exclusion of original game data. Packaging does not modify existing user
   data. Inspect `Contents/Resources/BUILD-INFO.json` or the external `.build.json`.
3. Push the commit and wait for Linux and both macOS CI checks. Exercise the
   original-data-dependent native UI/headless tests separately; clean CI needs
   no original game assets. Existing `docs/evidence/macos-*.json` files record
   the local port acceptance snapshot, including its exact binary hashes.

## Tag and distribute

Create an annotated version tag pointing at the verified commit and push it:

```bash
git tag -a v0.1.0 -m "Atomic Bomberman 0.1.0 macOS preview"
git push origin v0.1.0
```

Choose one distribution path:

- For the locally verified build, create a draft GitHub release for the existing
  tag, attach only the versioned ZIP, `.zip.sha256`, and `.build.json` from
  `dist/github/`, and inspect the uploaded hashes before publishing.
- For a CI-built release, run the **macOS release** workflow with that existing
  tag. It validates the version, tests the tagged source, builds from a clean
  checkout, and creates a draft preview release with the same three asset types.
  Review the draft and test its downloaded build before publishing it.

The workflow pins actions to commit IDs. Its build job has read-only repository
access; only the publish job can write release metadata/assets. It verifies the
remote tag still points at the build's source commit and refuses to overwrite
an existing release. Tags and published assets should remain unchanged; fix
releases with a new version and tag.

The initial `v0.1.0` Mac release is a preview, ad-hoc signed, and not notarized.
Developer ID signing/notarization requires a separate signing setup. Leave the
repository visibility unchanged and never attach original game data or the
separate asset-transfer archive to a release.
