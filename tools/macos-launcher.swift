import AppKit
import Darwin
import Foundation
import UniformTypeIdentifiers

enum LauncherError: LocalizedError {
    case message(String)
    case cancelled
    var errorDescription: String? {
        switch self {
        case .message(let text): return text
        case .cancelled: return "Import cancelled."
        }
    }
}

struct Options {
    var gameArguments: [String] = []
    var source: URL?
    var importOnly = false
    var customDirectory = false
    var directory: URL

    init(_ arguments: [String]) throws {
        let home = ProcessInfo.processInfo.environment["HOME"]
            ?? FileManager.default.homeDirectoryForCurrentUser.path
        directory = URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library/Application Support/Atomic Bomberman", isDirectory: true)
        var index = 0
        while index < arguments.count {
            let argument = arguments[index]
            switch argument {
            case "--import-source", "--data-directory":
                index += 1
                guard index < arguments.count else {
                    throw LauncherError.message("\(argument) requires a path.")
                }
                let url = URL(fileURLWithPath: arguments[index]).standardizedFileURL
                if argument == "--import-source" { source = url }
                else { directory = url; customDirectory = true }
            case "--import-only": importOnly = true
            default:
                // Old Finder versions pass a process serial number.
                if !argument.hasPrefix("-psn_") { gameArguments.append(argument) }
            }
            index += 1
        }
        if importOnly && source == nil {
            throw LauncherError.message("--import-only requires --import-source <archive, image or directory>.")
        }
        if source != nil && gameArguments.contains("--assets") {
            throw LauncherError.message("Choose either --import-source or --assets.")
        }
    }
}

/// Import jobs run in a private sibling directory, so failed or cancelled work
/// never modifies the active cache. This also contains the Rust import staging
/// and the temporarily extracted disc image.
final class AssetStore {
    let directory: URL
    let helper: URL
    let cache: URL
    init(directory: URL, helper: URL) {
        self.directory = directory
        self.helper = helper
        cache = directory.appendingPathComponent("assets", isDirectory: true)
    }

    func isValid() -> Bool {
        let process = Process()
        process.executableURL = helper
        process.arguments = ["validate", "--assets", cache.path]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do { try process.run(); process.waitUntilExit(); return process.terminationStatus == 0 }
        catch { return false }
    }
}

final class ImportJob {
    private let store: AssetStore
    private let lock = NSLock()
    private var cancelled = false
    private var process: Process?
    init(store: AssetStore) { self.store = store }

    func cancel() {
        lock.lock()
        cancelled = true
        if let process = process, process.isRunning { process.terminate() }
        lock.unlock()
    }

    private func checkCancellation() throws {
        lock.lock(); defer { lock.unlock() }
        if cancelled { throw LauncherError.cancelled }
    }

    private func run(_ arguments: [String], log: FileHandle) throws {
        let child = Process()
        child.executableURL = store.helper
        child.arguments = arguments
        child.standardOutput = log
        child.standardError = log
        lock.lock()
        if cancelled { lock.unlock(); throw LauncherError.cancelled }
        do { try child.run() } catch { lock.unlock(); throw error }
        process = child
        lock.unlock()
        child.waitUntilExit()
        lock.lock(); process = nil; lock.unlock()
        try checkCancellation()
        guard child.terminationReason == .exit && child.terminationStatus == 0 else {
            throw LauncherError.message("The game data could not be imported or validated.")
        }
    }

    func execute(source: URL) throws -> URL {
        let manager = FileManager.default
        try manager.createDirectory(at: store.directory, withIntermediateDirectories: true)
        let job = store.directory.appendingPathComponent(".import-\(UUID().uuidString)", isDirectory: true)
        try manager.createDirectory(at: job, withIntermediateDirectories: false)
        defer { try? manager.removeItem(at: job) }
        let replacement = job.appendingPathComponent("assets", isDirectory: true)
        let logURL = job.appendingPathComponent("import.log")
        guard manager.createFile(atPath: logURL.path, contents: nil) else {
            throw LauncherError.message("Could not create the import log.")
        }
        let log = try FileHandle(forWritingTo: logURL)
        defer { try? log.close() }
        do {
            try run(["import", "--source", source.path, "--output", replacement.path], log: log)
            try run(["validate", "--assets", replacement.path], log: log)
            try checkCancellation()
            let existing = try? manager.attributesOfItem(atPath: store.cache.path)
            if let existing = existing {
                guard existing[.type] as? FileAttributeType == .typeDirectory else {
                    throw LauncherError.message("The saved asset path must be a real directory.")
                }
                // Both paths are on the same volume. Swap atomically so a crash
                // cannot strand the previous cache between two rename calls.
                let result = replacement.path.withCString { incoming in
                    store.cache.path.withCString { active in
                        renameatx_np(AT_FDCWD, incoming, AT_FDCWD, active, UInt32(RENAME_SWAP))
                    }
                }
                guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            } else {
                try manager.moveItem(at: replacement, to: store.cache)
            }
            return store.cache
        } catch LauncherError.cancelled {
            throw LauncherError.cancelled
        } catch {
            try? log.synchronize()
            let details = (try? String(contentsOf: logURL, encoding: .utf8)) ?? ""
            throw LauncherError.message("\(error.localizedDescription)\n\n\(details.suffix(6000))")
        }
    }
}

final class ImportWindow: NSObject, NSWindowDelegate, NSApplicationDelegate {
    private let store: AssetStore
    private let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 270),
        styleMask: [.titled, .closable], backing: .buffered, defer: false)
    private let status = NSTextField(wrappingLabelWithString: "")
    private let progress = NSProgressIndicator()
    private let choose = NSButton(title: "Choose Game Data…", target: nil, action: nil)
    private let cancel = NSButton(title: "Quit", target: nil, action: nil)
    private var job: ImportJob?
    private var quitAfterCancel = false
    private(set) var result: URL?

    init(store: AssetStore, repair: Bool) {
        self.store = store
        super.init()
        window.title = "Atomic Bomberman"
        window.delegate = self
        window.isReleasedWhenClosed = false
        let title = NSTextField(labelWithString: repair ? "Repair Game Data" : "Welcome to Atomic Bomberman")
        title.font = .boldSystemFont(ofSize: 23)
        let explanation = NSTextField(wrappingLabelWithString:
            "Choose your Atomic Bomberman .7z archive, .img/.iso disc image, or extracted game folder. Your original files stay unchanged. You only need to do this once.")
        explanation.font = .systemFont(ofSize: 14)
        status.stringValue = repair ? "The saved game data could not be validated." : "Game data is required to play."
        status.textColor = .secondaryLabelColor
        progress.style = .bar
        progress.isIndeterminate = true
        progress.isHidden = true
        choose.target = self; choose.action = #selector(selectSource)
        choose.keyEquivalent = "\r"
        cancel.target = self; cancel.action = #selector(cancelImport)
        cancel.keyEquivalent = "\u{1b}"
        let buttons = NSStackView(views: [cancel, choose])
        buttons.orientation = .horizontal
        buttons.alignment = .centerY
        buttons.spacing = 12
        let stack = NSStackView(views: [title, explanation, status, progress, buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 18
        stack.translatesAutoresizingMaskIntoConstraints = false
        window.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: window.contentView!.leadingAnchor, constant: 28),
            stack.trailingAnchor.constraint(equalTo: window.contentView!.trailingAnchor, constant: -28),
            stack.topAnchor.constraint(equalTo: window.contentView!.topAnchor, constant: 26),
            progress.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
    }

    func show() -> URL? {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        app.delegate = self
        let menu = NSMenu()
        let item = NSMenuItem()
        let submenu = NSMenu()
        let quit = NSMenuItem(title: "Quit Atomic Bomberman", action: #selector(quitApp), keyEquivalent: "q")
        quit.target = self
        submenu.addItem(quit); item.submenu = submenu; menu.addItem(item)
        app.mainMenu = menu
        window.center(); window.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)
        app.run()
        window.orderOut(nil)
        return result
    }

    private func finish(_ cache: URL?) {
        result = cache
        NSApplication.shared.stop(nil)
        if let event = NSEvent.otherEvent(with: .applicationDefined, location: .zero,
            modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
            subtype: 0, data1: 0, data2: 0) {
            NSApplication.shared.postEvent(event, atStart: false)
        }
    }

    @objc private func selectSource() {
        let panel = NSOpenPanel()
        panel.title = "Choose Atomic Bomberman Game Data"
        panel.prompt = "Import"
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = false
        panel.allowedContentTypes = ["7z", "img", "iso"].compactMap { UTType(filenameExtension: $0) } + [.folder]
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let source = panel.url else { return }
            self.beginImport(source)
        }
    }

    private func beginImport(_ source: URL) {
        let task = ImportJob(store: store)
        job = task
        choose.isEnabled = false
        cancel.title = "Cancel"
        progress.isHidden = false; progress.startAnimation(nil)
        status.stringValue = "Importing game data. This can take several minutes…"
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try task.execute(source: source) }
            DispatchQueue.main.async {
                self.job = nil
                self.progress.stopAnimation(nil); self.progress.isHidden = true
                self.choose.isEnabled = true; self.cancel.isEnabled = true; self.cancel.title = "Quit"
                if self.quitAfterCancel { self.finish(nil); return }
                switch outcome {
                case .success(let cache): self.finish(cache)
                case .failure(LauncherError.cancelled): self.status.stringValue = "Import cancelled. Choose game data to try again."
                case .failure(let error):
                    self.status.stringValue = "Import failed. Choose another source to try again."
                    let alert = NSAlert()
                    alert.messageText = "Could Not Import Game Data"
                    alert.informativeText = error.localizedDescription
                    alert.beginSheetModal(for: self.window)
                }
            }
        }
    }

    @objc private func cancelImport() {
        if let job = job {
            status.stringValue = "Cancelling import…"
            cancel.isEnabled = false; job.cancel()
        } else { finish(nil) }
    }
    @objc private func quitApp() {
        if job != nil { quitAfterCancel = true; cancelImport() }
        else { finish(nil) }
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool { quitApp(); return false }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        quitApp(); return .terminateCancel
    }
}

func replaceProcess(_ executable: URL, _ arguments: [String]) throws -> Never {
    let strings = ([executable.path] + arguments).map { strdup($0) }
    defer { strings.forEach { free($0) } }
    var pointers = strings + [nil]
    pointers.withUnsafeMutableBufferPointer { buffer in
        _ = executable.path.withCString { execv($0, buffer.baseAddress!) }
    }
    throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
}

func launch() throws {
    var options = try Options(Array(CommandLine.arguments.dropFirst()))
    let executable = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
    let executables = executable.deletingLastPathComponent()
    let game = executables.appendingPathComponent("bomberman")
    if options.gameArguments.contains("--help") || options.gameArguments.contains("-h") {
        print("macOS launcher: [--import-source <path>] [--import-only] [--data-directory <directory>] [game options]\n")
        fflush(stdout)
        try replaceProcess(game, options.gameArguments)
    }
    if options.customDirectory && !options.gameArguments.contains("--settings")
        && ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"] == nil {
        options.gameArguments += ["--settings", options.directory.appendingPathComponent("settings.json").path]
    }
    if options.gameArguments.contains("--assets") || (options.gameArguments.contains("--headless") && options.source == nil) {
        try replaceProcess(game, options.gameArguments)
    }
    let store = AssetStore(directory: options.directory, helper: executables.appendingPathComponent("bomberman-assets"))
    let cache: URL
    if let source = options.source {
        cache = try ImportJob(store: store).execute(source: source)
    } else if store.isValid() {
        cache = store.cache
    } else {
        let controller = ImportWindow(store: store, repair: FileManager.default.fileExists(atPath: store.cache.path))
        guard let imported = controller.show() else { return }
        cache = imported
    }
    if options.importOnly { print("Imported and validated: \(cache.path)"); return }
    options.gameArguments += ["--assets", cache.path]
    // Finder does not provide a terminal. Keep game startup/runtime diagnostics
    // next to the managed data, outside the immutable signed application.
    if options.source == nil && !options.gameArguments.contains("--headless") {
        let logPath = options.directory.appendingPathComponent("game.log").path
        let descriptor = open(logPath, O_WRONLY | O_CREAT | O_TRUNC, S_IRUSR | S_IWUSR)
        if descriptor >= 0 { dup2(descriptor, STDOUT_FILENO); dup2(descriptor, STDERR_FILENO); close(descriptor) }
    }
    try replaceProcess(game, options.gameArguments)
}

@main
enum LauncherMain {
    static func main() {
        do {
            #if LAUNCHER_TESTS
            try launcherTests()
            #else
            try launch()
            #endif
        } catch {
            fputs("Atomic Bomberman launcher: \(error.localizedDescription)\n", stderr)
            exit(1)
        }
    }
}
