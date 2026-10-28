import Foundation

func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
    if try !condition() { throw LauncherError.message("Test failed: \(message)") }
}

func launcherTests() throws {
    let manager = FileManager.default
    let root = manager.temporaryDirectory.resolvingSymlinksInPath()
        .appendingPathComponent("atomic-launcher-tests-\(UUID().uuidString)", isDirectory: true)
    try manager.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? manager.removeItem(at: root) }
    let helper = root.appendingPathComponent("fixture importer")
    // Model the real helper's process protocol. Real disc decoding is covered
    // by the Rust suite and the end-to-end supplied-disc acceptance run.
    let fixture = """
    #!/bin/sh
    set -eu
    if [ "$1" = validate ]; then test -f "$3/payload"; exit; fi
    mkdir -p "$5"
    printf '%s' "$3" > "$5/payload"
    case "$3" in
      */fail) exit 42 ;;
      */slow) exec /bin/sleep 30 ;;
    esac
    """
    try fixture.write(to: helper, atomically: true, encoding: .utf8)
    try manager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: helper.path)
    let store = AssetStore(directory: root.appendingPathComponent("data with spaces"), helper: helper)
    func source(_ name: String) -> URL { root.appendingPathComponent(name) }
    func payload() throws -> String { try String(contentsOf: store.cache.appendingPathComponent("payload"), encoding: .utf8) }
    func clean() throws {
        let names = try manager.contentsOfDirectory(atPath: store.directory.path)
        try require(!names.contains { $0.hasPrefix(".import-") }, "temporary import jobs were cleaned")
    }

    try require(!store.isValid(), "first launch has no cache")
    _ = try ImportJob(store: store).execute(source: source("first"))
    try require(store.isValid(), "first import published a valid cache")
    try require(try payload() == source("first").path, "first import contents")
    _ = try ImportJob(store: store).execute(source: source("second"))
    try require(try payload() == source("second").path, "atomic replacement replaced old data")
    do {
        _ = try ImportJob(store: store).execute(source: source("fail"))
        throw LauncherError.message("failed import unexpectedly succeeded")
    } catch LauncherError.message(let text) {
        try require(text.contains("could not be imported"), "import failure diagnostics")
    }
    try require(try payload() == source("second").path, "failed import preserved active cache")
    try clean()

    let task = ImportJob(store: store)
    let finished = DispatchSemaphore(value: 0)
    let resultLock = NSLock()
    var cancelledAsExpected = false
    DispatchQueue.global().async {
        do { _ = try task.execute(source: source("slow")) }
        catch LauncherError.cancelled { resultLock.lock(); cancelledAsExpected = true; resultLock.unlock() }
        catch { fputs("Unexpected cancellation error: \(error)\n", stderr) }
        finished.signal()
    }
    let deadline = Date().addingTimeInterval(5)
    var running = false
    while Date() < deadline {
        let folders = (try? manager.contentsOfDirectory(at: store.directory, includingPropertiesForKeys: nil)) ?? []
        running = folders.contains { $0.lastPathComponent.hasPrefix(".import-")
            && manager.fileExists(atPath: $0.appendingPathComponent("assets/payload").path) }
        if running { break }
        Thread.sleep(forTimeInterval: 0.01)
    }
    try require(running, "slow import was actually running before cancellation")
    task.cancel()
    try require(finished.wait(timeout: .now() + 5) == .success, "cancel stopped the helper promptly")
    resultLock.lock(); let wasCancelled = cancelledAsExpected; resultLock.unlock()
    try require(wasCancelled, "cancelled import reported cancellation")
    try require(try payload() == source("second").path, "cancel preserved active cache")
    try clean()

    try manager.removeItem(at: store.cache.appendingPathComponent("payload"))
    try require(!store.isValid(), "corrupt cache detected")
    _ = try ImportJob(store: store).execute(source: source("repair"))
    try require(store.isValid(), "corrupt cache repaired after validation")
    try clean()

    let outside = root.appendingPathComponent("outside", isDirectory: true)
    try manager.moveItem(at: store.cache, to: outside)
    try manager.createSymbolicLink(at: store.cache, withDestinationURL: outside)
    do {
        _ = try ImportJob(store: store).execute(source: source("third"))
        throw LauncherError.message("symlink cache unexpectedly replaced")
    } catch LauncherError.message(let text) {
        try require(text.contains("real directory"), "symlink cache refused")
    }
    try require(try String(contentsOf: outside.appendingPathComponent("payload"), encoding: .utf8) == source("repair").path,
        "symlink target remained untouched")
    try clean()

    let options = try Options(["--data-directory", root.path, "--assets", "/cache with spaces",
        "--settings", "/settings.json", "--windowed"])
    try require(options.gameArguments == ["--assets", "/cache with spaces", "--settings", "/settings.json", "--windowed"],
        "explicit game arguments preserved")
    print("Launcher acceptance passed: first import, atomic replacement, failure preservation, cancellation, corruption recovery, symlink refusal, argument forwarding.")
}
