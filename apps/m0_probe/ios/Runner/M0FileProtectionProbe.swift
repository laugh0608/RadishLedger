import Darwin
import Foundation

/// DB / WAL / SHM 只读及写方式 open；实际 pwrite 只作用于排他创建的独立合成对照。
final class M0FileProtectionProbe {
  private static let names = ["ledger.db", "ledger.db-wal", "ledger.db-shm"]
  private var directory: Int32 = -1
  private var identities: [String: (dev_t, ino_t)] = [:]
  private var writeControl: M0FileWriteControl?

  init(storage: String, run: String) throws {
    guard run.range(of: "^[0-9a-f]{32}$", options: .regularExpression) != nil else {
      throw NSError(domain: "M0FileProtectionProbe", code: 1)
    }
    directory = open(storage + "/m0c-" + run, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
    guard directory >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    do {
      var info = stat()
      guard fstat(directory, &info) == 0, info.st_uid == geteuid() else {
        throw NSError(domain: "M0FileProtectionProbe", code: 2)
      }
      let owner = openat(directory, "owner", O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
      guard owner >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
      var bytes = [UInt8](repeating: 0, count: 128)
      let regular = fstat(owner, &info) == 0 && info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG)
      let count = regular ? read(owner, &bytes, bytes.count) : -1
      let closed = close(owner)
      let expected = Array(("RadishLedger M0-C synthetic vault v1\n" + run + "\n").utf8)
      guard closed == 0, count == expected.count, Array(bytes.prefix(expected.count)) == expected else {
        throw NSError(domain: "M0FileProtectionProbe", code: 3)
      }
      for name in Self.names {
        guard fstatat(directory, name, &info, AT_SYMLINK_NOFOLLOW) == 0,
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG), info.st_uid == geteuid() else {
          throw NSError(domain: "M0FileProtectionProbe", code: 4)
        }
        identities[name] = (info.st_dev, info.st_ino)
      }
      writeControl = try M0FileWriteControl(directory: directory, path: storage + "/m0c-" + run)
    } catch {
      close(directory)
      directory = -1
      throw error
    }
  }

  deinit { _ = closeResources() }

  func closeResources() -> [String: Int] {
    let controlError = writeControl?.closeDescriptor() ?? 0
    let directoryError = directory >= 0 && close(directory) != 0 ? Int(errno) : 0
    directory = -1
    return ["writeControlCloseErrno": controlError, "directoryCloseErrno": directoryError]
  }

  private func attempt(_ name: String, writeOpen: Bool) -> [String: Any] {
    let fd = openat(directory, name, (writeOpen ? O_WRONLY : O_RDONLY) | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
    guard fd >= 0 else { return ["stage": "open", "result": -1, "errno": Int(errno)] }
    var info = stat()
    let statResult = fstat(fd, &info)
    let statError = statResult == 0 ? 0 : Int(errno)
    var result: [String: Any]
    if statResult != 0 {
      result = ["stage": "fstat", "result": -1, "errno": statError]
    } else if info.st_mode & mode_t(S_IFMT) != mode_t(S_IFREG) ||
                identities[name]?.0 != info.st_dev || identities[name]?.1 != info.st_ino {
      result = ["stage": "identityMismatch", "result": -1, "errno": 0]
    } else if writeOpen {
      result = ["stage": "open", "result": 0, "errno": 0]
    } else {
      var byte: UInt8 = 0
      let count = read(fd, &byte, 1)
      let code = count < 0 ? Int(errno) : 0
      byte = 0
      result = ["stage": "read", "result": count, "errno": code]
    }
    let closed = close(fd)
    result["closeErrno"] = closed == 0 ? 0 : Int(errno)
    return result
  }

  func sample() -> [String: Any] {
    let files = Self.names.map { ["name": $0, "read": attempt($0, writeOpen: false), "writeOpen": attempt($0, writeOpen: true)] }
    return ["files": files, "writeControl": writeControl!.sample()]
  }
}

/// 保留本次合成对照的描述符，使锁屏阶段即使 open 被拒绝，也能实际调用 pwrite。
private final class M0FileWriteControl {
  private var descriptor: Int32 = -1
  private var value: UInt8 = 0x5a
  private var protection = "unavailableOnHost"

  init(directory: Int32, path: String) throws {
    let name = "lock-write-control"
    descriptor = openat(directory, name, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
    guard descriptor >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    do {
      let attributes = try FileManager.default.attributesOfItem(atPath: path + "/" + name)
      let observed = (attributes[.protectionKey] as? FileProtectionType)?.rawValue
        ?? (attributes[.protectionKey] as? String)
      protection = observed ?? "unavailableOnHost"
      #if os(iOS)
      guard protection == FileProtectionType.complete.rawValue else {
        throw NSError(domain: "M0FileWriteControl", code: 1)
      }
      #endif
      var info = stat()
      guard fstat(descriptor, &info) == 0, info.st_uid == geteuid(),
            info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG), info.st_mode & 0o777 == 0o600 else {
        throw NSError(domain: "M0FileWriteControl", code: 2)
      }
    } catch {
      close(descriptor)
      descriptor = -1
      throw error
    }
  }

  deinit { _ = closeDescriptor() }

  func closeDescriptor() -> Int {
    let code = descriptor >= 0 && close(descriptor) != 0 ? Int(errno) : 0
    descriptor = -1
    return code
  }

  func sample() -> [String: Any] {
    value = value == 0x5a ? 0xa5 : 0x5a
    // 仅向此排他创建、无账务内容的对照文件写入一个固定字节；绝不写账库页。
    let written = pwrite(descriptor, &value, 1, 0)
    let code = written < 0 ? Int(errno) : 0
    var result: [String: Any] = [
      "name": "lock-write-control", "fileProtection": protection,
      "retainedDescriptor": true,
      "pwrite": ["result": written, "errno": code],
    ]
    if written == 1 {
      let synced = fsync(descriptor)
      result["fsync"] = ["result": Int(synced), "errno": synced == 0 ? 0 : Int(errno)]
      var returned: UInt8 = 0
      let count = pread(descriptor, &returned, 1, 0)
      result["readback"] = ["result": count, "errno": count < 0 ? Int(errno) : 0,
                            "matched": count == 1 && returned == value]
    }
    return result
  }
}

/// 时间和可用状态由调用方提供，宿主可验证暂停 / 超时语义；不推测 OS 是否暂停过进程。
final class M0FileProtectionObservation {
  static func continuousTime() -> TimeInterval {
    var scale = mach_timebase_info_data_t()
    mach_timebase_info(&scale)
    return Double(mach_continuous_time()) * Double(scale.numer) / Double(scale.denom) / 1_000_000_000
  }

  private let deadline: TimeInterval
  private let available: () -> Bool
  private let sample: () -> [String: Any]
  private var report: [String: Any] = [
    "schema": 1, "hardwareProtectionVerified": false, "acceptance": "not-assessed",
    "writeScope": "ownedControlOnly", "databaseWriteScope": "openOnlyNoBytesWritten", "lockedAttempted": false,
  ]
  private var attempted = false
  private var finished = false

  init(now: TimeInterval, available: @escaping () -> Bool, sample: @escaping () -> [String: Any]) {
    deadline = now + 45
    self.available = available
    self.sample = sample
  }

  private func capture() -> [String: Any] {
    let before = available()
    var observation = sample()
    observation["availableBefore"] = before
    observation["availableAfter"] = available()
    return observation
  }

  private static func successful(_ observation: [String: Any]) -> Bool {
    guard observation["availableBefore"] as? Bool == true,
          observation["availableAfter"] as? Bool == true,
          let files = observation["files"] as? [[String: Any]], files.count == 3 else { return false }
    return files.allSatisfy { file in
      ["read", "writeOpen"].allSatisfy { key in
        guard let io = file[key] as? [String: Any] else { return false }
        return io["stage"] as? String == (key == "read" ? "read" : "open") &&
          io["result"] as? Int == (key == "read" ? 1 : 0) && io["errno"] as? Int == 0 &&
          io["closeErrno"] as? Int == 0
      }
    }
  }

  static func denied(_ observation: [String: Any]) -> Bool {
    guard observation["availableBefore"] as? Bool == false,
          observation["availableAfter"] as? Bool == false,
          let files = observation["files"] as? [[String: Any]], files.count == 3 else { return false }
    return files.allSatisfy { file in
      ["read", "writeOpen"].allSatisfy { key in
        guard let io = file[key] as? [String: Any], let code = io["errno"] as? Int,
              let stage = io["stage"] as? String else { return false }
        return io["result"] as? Int == -1 && (code == Int(EACCES) || code == Int(EPERM)) &&
          (stage == "open" || (key == "read" && stage == "read")) &&
          (io["closeErrno"] as? Int ?? 0) == 0
      }
    }
  }

  static func writeDenied(_ observation: [String: Any]) -> Bool {
    guard observation["availableBefore"] as? Bool == false,
          observation["availableAfter"] as? Bool == false,
          let control = observation["writeControl"] as? [String: Any],
          let io = control["pwrite"] as? [String: Any], let code = io["errno"] as? Int else { return false }
    return io["result"] as? Int == -1 && (code == Int(EACCES) || code == Int(EPERM))
  }

  private static func writeSucceeded(_ observation: [String: Any]) -> Bool {
    guard observation["availableBefore"] as? Bool == true,
          observation["availableAfter"] as? Bool == true,
          let control = observation["writeControl"] as? [String: Any],
          let write = control["pwrite"] as? [String: Any],
          let sync = control["fsync"] as? [String: Any],
          let readback = control["readback"] as? [String: Any] else { return false }
    return write["result"] as? Int == 1 && write["errno"] as? Int == 0 &&
      sync["result"] as? Int == 0 && sync["errno"] as? Int == 0 &&
      readback["result"] as? Int == 1 && readback["errno"] as? Int == 0 && readback["matched"] as? Bool == true
  }

  func start() -> [String: Any]? {
    guard available() else { return finish("baselineUnavailable") }
    let baseline = capture()
    report["baseline"] = baseline
    return Self.successful(baseline) && Self.writeSucceeded(baseline) ? nil : finish("baselineFailed")
  }

  func tick(now: TimeInterval) -> [String: Any]? {
    guard !finished else { return nil }
    // 先检查真实经过时间：恢复执行时已过期，不补造锁屏调用。
    if now >= deadline { return finish(attempted ? "restorationNotObserved" : "lockedIoNotExecuted") }
    if !attempted && !available() {
      let locked = capture()
      report["locked"] = locked
      report["lockedAttempted"] = true
      report["accessDeniedObserved"] = Self.denied(locked)
      report["controlWriteDeniedObserved"] = Self.writeDenied(locked)
      attempted = true
    } else if attempted && available() {
      let restored = capture()
      report["restored"] = restored
      report["accessRestored"] = Self.successful(restored)
      report["controlWriteRestored"] = Self.writeSucceeded(restored)
      return finish("observed")
    }
    return nil
  }

  func cancel() -> [String: Any] { finish("cancelled") }

  private func finish(_ outcome: String) -> [String: Any] {
    finished = true
    report["outcome"] = outcome
    return report
  }
}
