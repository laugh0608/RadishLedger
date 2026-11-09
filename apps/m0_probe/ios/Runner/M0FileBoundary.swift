import Darwin
import Foundation

/// 仅观察本次合成目录的实际属性；Simulator 属性不代表真机硬件保护。
enum M0FileBoundary {
  private static let names: Set<String> = ["owner", "lock", "ledger.db", "ledger.db-wal", "ledger.db-shm", "auxiliary.db", "scanner-control.db"]

  private static func metadata(_ url: URL, directory: Bool) throws -> [String: Any] {
    var info = stat()
    guard lstat(url.path, &info) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    guard info.st_mode & mode_t(S_IFMT) == mode_t(directory ? S_IFDIR : S_IFREG) else {
      throw NSError(domain: "M0FileBoundary", code: 1)
    }
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    let values = try url.resourceValues(forKeys: [.isExcludedFromBackupKey])
    return ["path": url.resolvingSymlinksInPath().path, "mode": Int(info.st_mode & 0o777),
            "uid": Int(info.st_uid), "bytes": Int64(info.st_size),
            "fileProtection": attributes[.protectionKey] ?? NSNull(),
            "ownExcludedFromBackup": values.isExcludedFromBackup.map { $0 as Any } ?? NSNull()]
  }

  static func environment(storage: String) throws -> [String: Any] {
    let manager = FileManager.default
    let root = URL(fileURLWithPath: storage).resolvingSymlinksInPath()
    #if targetEnvironment(simulator)
    let execution = "simulator"
    #else
    let execution = "device"
    #endif
    return ["platform": "ios", "pid": Int(getpid()), "uid": Int(geteuid()),
            "environment": execution, "bundle": Bundle.main.bundleIdentifier ?? "",
            "bundlePath": Bundle.main.bundleURL.path, "home": URL(fileURLWithPath: NSHomeDirectory()).resolvingSymlinksInPath().path,
            "storage": root.path, "storageMetadata": try metadata(root, directory: true),
            "temporary": NSTemporaryDirectory(),
            "cache": try manager.url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: false).path]
  }

  static func attributes(storage: String, run: String) throws -> [String: Any] {
    guard run.range(of: "^[0-9a-f]{32}$", options: .regularExpression) != nil else {
      throw NSError(domain: "M0FileBoundary", code: 2)
    }
    let root = URL(fileURLWithPath: storage).resolvingSymlinksInPath().appendingPathComponent("m0c-" + run, isDirectory: true)
    let rootInfo = try metadata(root, directory: true)
    let files = try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
    guard Set(files.map(\.lastPathComponent)) == names else { throw NSError(domain: "M0FileBoundary", code: 3) }
    return ["root": rootInfo, "files": try files.sorted { $0.lastPathComponent < $1.lastPathComponent }.map { try metadata($0, directory: false) }]
  }
}
