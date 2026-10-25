import Darwin
import Flutter
import LocalAuthentication
import Security
import UIKit

/// 仅适配本项目 M0-C 的精确 Keychain 项；不访问宿主钥匙串或其他服务。
final class M0Protection {
  private let service = "com.radishledger.m0c"
  private let channel: FlutterMethodChannel
  private var pending: LAContext?
  private var epoch = 0
  private var observer: NSObjectProtocol?
  private var fileObservation: M0FileProtectionObservation?
  private var fileTimer: Timer?
  private var fileResult: FlutterResult?

  init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(name: "radishledger/m0c/protection", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in self?.handle(call, result: result) }
    observer = NotificationCenter.default.addObserver(
      forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
    ) { [weak self] _ in self?.cancel() }
  }

  deinit {
    if let observer { NotificationCenter.default.removeObserver(observer) }
    pending?.invalidate()
    fileTimer?.invalidate()
    channel.setMethodCallHandler(nil)
  }

  private func cancel() {
    epoch += 1
    pending?.invalidate()
  }

  private func failure(_ code: String, _ status: Int = 0) -> FlutterError {
    FlutterError(code: code, message: "系统保护操作未完成，原账库保留", details: ["platformCode": status])
  }

  private func mapped(_ status: OSStatus) -> FlutterError {
    switch status {
    case errSecItemNotFound: return failure("item_missing", Int(status))
    case errSecUserCanceled: return failure("user_cancelled", Int(status))
    case errSecAuthFailed: return failure("authentication_denied", Int(status))
    case errSecInteractionNotAllowed: return failure("temporarily_unavailable", Int(status))
    case errSecNotAvailable: return failure("unsupported", Int(status))
    case errSecDuplicateItem: return failure("item_exists", Int(status))
    case errSecDecode: return failure("corrupt_protection", Int(status))
    default: return failure("platform_error", Int(status))
    }
  }

  private func capability() -> [String: Any] {
    let context = LAContext()
    var error: NSError?
    let supported = context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error)
    context.invalidate()
    #if targetEnvironment(simulator)
    let environment = "simulator"
    #else
    let environment = "device"
    #endif
    return ["supported": supported, "environment": environment,
            "protection": "whenPasscodeSetThisDeviceOnly+userPresence",
            "hardwareClaim": "not-measured", "platformCode": error?.code ?? 0]
  }

  private func storage() throws -> String {
    let manager = FileManager.default
    var root = try manager.url(for: .applicationSupportDirectory, in: .userDomainMask,
                               appropriateFor: nil, create: true).appendingPathComponent("m0-c", isDirectory: true)
    try manager.createDirectory(at: root, withIntermediateDirectories: true,
                                attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try root.setResourceValues(values)
    return root.path
  }

  private func control(remove: Bool) throws -> String {
    let root = try storage()
    for name in ["control-in", "control-out"] {
      let path = root + "/" + name
      var attributes = stat()
      if lstat(path, &attributes) == 0 {
        guard (attributes.st_mode & mode_t(S_IFMT)) == mode_t(S_IFIFO) else { throw NSError(domain: "M0C", code: 1) }
        if remove && unlink(path) != 0 { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
      } else if errno == ENOENT {
        if !remove && mkfifo(path, 0o600) != 0 { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
      } else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    }
    if remove {
      let ready = root + "/control-ready.json"
      if FileManager.default.fileExists(atPath: ready) { try FileManager.default.removeItem(atPath: ready) }
    }
    return root
  }

  private func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    if call.method == "cancelFileProtectionObservation" {
      if let observation = fileObservation { finishFileObservation(observation.cancel()) }
      result(nil)
      return
    }
    if call.method == "observeFileProtection" {
      guard fileObservation == nil else { result(failure("protection_busy")); return }
      #if targetEnvironment(simulator)
      result(["schema": 1, "outcome": "unsupportedSimulator", "lockedAttempted": false,
              "hardwareProtectionVerified": false, "acceptance": "not-assessed"])
      #else
      guard UIApplication.shared.isProtectedDataAvailable else {
        result(failure("temporarily_unavailable")); return
      }
      do {
        let arguments = call.arguments as? [String: Any]
        let probe = try M0FileProtectionProbe(storage: storage(), run: arguments?["run"] as? String ?? "")
        let observation = M0FileProtectionObservation(
          now: M0FileProtectionObservation.continuousTime(),
          available: { UIApplication.shared.isProtectedDataAvailable }, sample: { probe.sample() })
        fileObservation = observation
        fileResult = result
        if let report = observation.start() { finishFileObservation(report); return }
        fileTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
          guard let self, let report = self.fileObservation?.tick(now: M0FileProtectionObservation.continuousTime()) else { return }
          self.finishFileObservation(report)
        }
      } catch { result(failure("file_boundary_unavailable", (error as NSError).code)) }
      #endif
      return
    }
    if call.method == "fileEnvironment" || call.method == "fileAttributes" {
      do {
        let root = try storage()
        if call.method == "fileEnvironment" { result(try M0FileBoundary.environment(storage: root)) }
        else {
          let arguments = call.arguments as? [String: Any]
          result(try M0FileBoundary.attributes(storage: root, run: arguments?["run"] as? String ?? ""))
        }
      } catch { result(failure("file_boundary_unavailable", (error as NSError).code)) }
      return
    }
    if call.method == "control" || call.method == "removeControl" {
      do { result(try control(remove: call.method == "removeControl")) }
      catch { result(failure("control_unavailable", (error as NSError).code)) }
      return
    }
    if call.method == "capabilities" { result(capability()); return }
    if call.method == "storage" {
      do { result(try storage()) } catch { result(failure("storage_unavailable", (error as NSError).code)) }
      return
    }
    if call.method == "cancel" { cancel(); result(nil); return }
    guard ["create", "read", "delete"].contains(call.method),
          let arguments = call.arguments as? [String: Any],
          let run = arguments["run"] as? String, let generation = arguments["generation"] as? String,
          run.range(of: "^[0-9a-f]{32}$", options: .regularExpression) != nil,
          generation.range(of: "^[0-9a-f]{32}$", options: .regularExpression) != nil else {
      result(failure("invalid_argument")); return
    }
    guard pending == nil else { result(failure("protection_busy")); return }
    var secret = (arguments["secret"] as? FlutterStandardTypedData)?.data ?? Data()
    if call.method == "create" && secret.count != 32 { result(failure("invalid_argument")); return }
    if call.method == "create" && capability()["supported"] as? Bool != true {
      secret.resetBytes(in: 0..<secret.count)
      result(failure("unsupported")); return
    }
    let context = LAContext()
    context.localizedReason = "解锁 RadishLedger 的本次合成实验账库"
    pending = context
    let operationEpoch = epoch
    let base: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
                              kSecAttrService as String: service,
                              kSecAttrAccount as String: "\(run).\(generation)",
                              kSecAttrSynchronizable as String: false]
    DispatchQueue.global(qos: .userInitiated).async {
      var query = base
      var response: Any?
      defer { secret.resetBytes(in: 0..<secret.count) }
      if call.method == "delete" {
        let status = SecItemDelete(query as CFDictionary)
        response = (status == errSecSuccess || status == errSecItemNotFound)
          ? ["deleted": true, "alreadyMissing": status == errSecItemNotFound] : self.mapped(status)
      } else {
        var created = false
        if call.method == "create" {
          var accessError: Unmanaged<CFError>?
          if let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
                                                          .userPresence, &accessError) {
            var add = base
            add[kSecAttrAccessControl as String] = access
            add[kSecValueData as String] = secret
            let status = SecItemAdd(add as CFDictionary, nil)
            created = status == errSecSuccess
            if !created { response = self.mapped(status) }
          } else {
            let code = accessError.map { CFErrorGetCode($0.takeRetainedValue()) } ?? 0
            response = self.failure("unsupported", code)
          }
        }
        if response == nil {
          query[kSecUseAuthenticationContext as String] = context
          query[kSecReturnData as String] = true
          query[kSecMatchLimit as String] = kSecMatchLimitOne
          var returned: CFTypeRef?
          let status = SecItemCopyMatching(query as CFDictionary, &returned)
          if status == errSecSuccess, var bytes = returned as? Data, bytes.count == 32 {
            if call.method == "create" {
              response = bytes == secret ? ["created": true, "protection": "whenPasscodeSetThisDeviceOnly+userPresence"] : self.failure("verification_failed")
            } else {
              response = FlutterStandardTypedData(bytes: bytes)
            }
            // Data / CFData / Flutter codec 可能保留副本；这里只清理本地可控副本。
            bytes.resetBytes(in: 0..<bytes.count)
          } else {
            response = status == errSecSuccess ? self.failure("corrupt_protection") : self.mapped(status)
          }
        }
        if created && response is FlutterError {
          let cleanup = SecItemDelete(base as CFDictionary)
          if cleanup != errSecSuccess && cleanup != errSecItemNotFound { response = self.failure("cleanup_failed", Int(cleanup)) }
        }
      }
      DispatchQueue.main.async {
        self.pending = nil
        context.invalidate()
        if operationEpoch != self.epoch { result(self.failure("interrupted")) }
        else { result(response) }
      }
    }
  }

  private func finishFileObservation(_ report: [String: Any]) {
    fileTimer?.invalidate()
    fileTimer = nil
    fileObservation = nil
    let result = fileResult
    fileResult = nil
    result?(report)
  }
}
