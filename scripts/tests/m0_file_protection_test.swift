import Darwin
import Foundation

@main
enum FileProtectionTests {
  static func main() throws {
    let run = String(repeating: "a", count: 32)
    guard let temporary = ProcessInfo.processInfo.environment["M0_FILE_PROTECTION_TEST_ROOT"] else {
      fatalError("run through scripts/check-ios-file-protection.sh")
    }
    let storage = URL(fileURLWithPath: temporary).appendingPathComponent("m0-file-protection-" + UUID().uuidString)
    let root = storage.appendingPathComponent("m0c-" + run)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try! FileManager.default.removeItem(at: storage) }
    let owner = root.appendingPathComponent("owner")
    try Data(("RadishLedger M0-C synthetic vault v1\n" + run + "\n").utf8).write(to: owner)
    let names = ["ledger.db", "ledger.db-wal", "ledger.db-shm"]
    let payload = Data("synthetic-file-protection-control".utf8)
    for name in names { try payload.write(to: root.appendingPathComponent(name)) }
    let probe = try M0FileProtectionProbe(storage: storage.path, run: run)
    var available = true
    var calls = 0
    let actual = M0FileProtectionObservation(now: 0, available: { available }, sample: {
      calls += 1
      return probe.sample()
    })
    assert(actual.start() == nil)
    available = false
    assert(actual.tick(now: 1) == nil)
    available = true
    let observed = actual.tick(now: 2)!
    assert(observed["outcome"] as? String == "observed")
    assert(observed["accessDeniedObserved"] as? Bool == false) // 宿主可读不能宣称保护拒绝。
    assert(observed["accessRestored"] as? Bool == true && calls == 3)
    assert(observed["controlWriteDeniedObserved"] as? Bool == false)
    assert(observed["controlWriteRestored"] as? Bool == true)
    let control = root.appendingPathComponent("lock-write-control")
    let firstControl = try Data(contentsOf: control)
    assert(firstControl == Data([0xa5])) // 三次真实 pwrite，固定字节交替；不是 open 成功替身。
    for name in names {
      let preserved = try Data(contentsOf: root.appendingPathComponent(name))
      assert(preserved == payload)
    }

    calls = 0
    let expired = M0FileProtectionObservation(now: 0, available: { available }, sample: { calls += 1; return probe.sample() })
    assert(expired.start() == nil)
    available = false
    let timedOut = expired.tick(now: 46)!
    assert(timedOut["outcome"] as? String == "lockedIoNotExecuted" && calls == 1)
    assert(timedOut["lockedAttempted"] as? Bool == false && expired.tick(now: 47) == nil)
    let unavailable = M0FileProtectionObservation(now: 0, available: { false }, sample: { fatalError("must not sample") })
    assert(unavailable.start()?["outcome"] as? String == "baselineUnavailable")

    func denial(_ code: Int32, stage: String = "open") -> [[String: Any]] {
      names.map { ["name": $0,
                   "read": ["stage": stage, "result": -1, "errno": Int(code)],
                   "writeOpen": ["stage": "open", "result": -1, "errno": Int(code)]] }
    }
    func locked(_ files: [[String: Any]], after: Bool = false) -> [String: Any] {
      ["availableBefore": false, "availableAfter": after, "files": files]
    }
    func deniedSample(_ code: Int32) -> [String: Any] {
      ["files": denial(code), "writeControl": ["pwrite": ["result": -1, "errno": Int(code)]]]
    }
    assert(M0FileProtectionObservation.denied(locked(denial(EACCES))))
    assert(M0FileProtectionObservation.denied(locked(denial(EPERM, stage: "read"))))
    for code in [ENOENT, EIO, ELOOP, ESTALE] {
      assert(!M0FileProtectionObservation.denied(locked(denial(code))))
    }
    assert(!M0FileProtectionObservation.denied(locked(denial(EACCES), after: true)))
    assert(!M0FileProtectionObservation.denied(locked(denial(EACCES, stage: "fstat"))))
    var closeFailure = denial(EACCES)
    closeFailure[0]["read"] = ["stage": "read", "result": -1, "errno": Int(EACCES), "closeErrno": Int(EIO)]
    assert(!M0FileProtectionObservation.denied(locked(closeFailure)))

    available = true
    let recovery = M0FileProtectionObservation(now: 0, available: { available }, sample: {
      available ? probe.sample() : deniedSample(EACCES)
    })
    assert(recovery.start() == nil)
    available = false
    assert(recovery.tick(now: 3) == nil)
    available = true
    let recovered = recovery.tick(now: 4)!
    assert(recovered["accessDeniedObserved"] as? Bool == true)
    assert(recovered["accessRestored"] as? Bool == true)
    assert(recovered["controlWriteDeniedObserved"] as? Bool == true)
    assert(recovered["controlWriteRestored"] as? Bool == true)
    assert(recovered["hardwareProtectionVerified"] as? Bool == false)
    let incomplete = M0FileProtectionObservation(now: 0, available: { available }, sample: {
      available ? probe.sample() : deniedSample(EPERM)
    })
    assert(incomplete.start() == nil)
    available = false
    assert(incomplete.tick(now: 1) == nil)
    assert(incomplete.tick(now: 45)?["outcome"] as? String == "restorationNotObserved")
    available = true
    let cancelled = M0FileProtectionObservation(now: 0, available: { available }, sample: { probe.sample() })
    assert(cancelled.start() == nil)
    assert(cancelled.cancel()["outcome"] as? String == "cancelled")
    assert(cancelled.tick(now: 1) == nil)

    // 写方式 open 被拒绝但没有 pwrite 结果，不能记为实际 write 拒绝。
    assert(!M0FileProtectionObservation.writeDenied(locked(denial(EACCES))))
    for code in [ENOENT, EIO, EBADF] {
      var observation = deniedSample(code)
      observation["availableBefore"] = false
      observation["availableAfter"] = false
      assert(!M0FileProtectionObservation.writeDenied(observation))
    }
    var unstable = deniedSample(EACCES)
    unstable["availableBefore"] = false
    unstable["availableAfter"] = true
    assert(!M0FileProtectionObservation.writeDenied(unstable))
    let badWrite = M0FileProtectionObservation(now: 0, available: { true }, sample: {
      var sample = probe.sample()
      sample["writeControl"] = ["pwrite": ["result": 0, "errno": 0]] // 短写不能作基线。
      return sample
    })
    assert(badWrite.start()?["outcome"] as? String == "baselineFailed")
    let badSync = M0FileProtectionObservation(now: 0, available: { true }, sample: {
      var sample = probe.sample()
      sample["writeControl"] = ["pwrite": ["result": 1, "errno": 0],
                                "fsync": ["result": -1, "errno": Int(EIO)],
                                "readback": ["result": 1, "errno": 0, "matched": true]]
      return sample
    })
    assert(badSync.start()?["outcome"] as? String == "baselineFailed")
    let beforeCollision = try Data(contentsOf: control)
    do {
      _ = try M0FileProtectionProbe(storage: storage.path, run: run)
      fatalError("existing control must not be reused or overwritten")
    } catch { assert((error as NSError).code == Int(EEXIST)) }
    let afterCollision = try Data(contentsOf: control)
    assert(beforeCollision == afterCollision)
    let savedControl = root.appendingPathComponent("retained-control")
    let foreign = storage.appendingPathComponent("unrelated")
    try payload.write(to: foreign)
    try FileManager.default.moveItem(at: control, to: savedControl)
    try FileManager.default.createSymbolicLink(at: control, withDestinationURL: foreign)
    do {
      _ = try M0FileProtectionProbe(storage: storage.path, run: run)
      fatalError("control symlink must not be followed")
    } catch { assert((error as NSError).code == Int(EEXIST)) }
    let untouched = try Data(contentsOf: foreign)
    assert(untouched == payload)
    try FileManager.default.removeItem(at: control)
    try FileManager.default.moveItem(at: savedControl, to: control)

    let db = root.appendingPathComponent("ledger.db")
    let saved = root.appendingPathComponent("retained.db")
    try FileManager.default.moveItem(at: db, to: saved)
    try payload.write(to: db)
    let replaced = (probe.sample()["files"] as! [[String: Any]])[0]["read"] as! [String: Any]
    assert(replaced["stage"] as? String == "identityMismatch")
    try FileManager.default.removeItem(at: db)
    try FileManager.default.createSymbolicLink(at: db, withDestinationURL: saved)
    let linked = (probe.sample()["files"] as! [[String: Any]])[0]["writeOpen"] as! [String: Any]
    assert(linked["result"] as? Int == -1)
    let preserved = try Data(contentsOf: saved)
    assert(preserved == payload)
    do {
      _ = try M0FileProtectionProbe(storage: storage.path, run: run)
      fatalError("symlink must reject")
    } catch { }
    try FileManager.default.removeItem(at: db)
    try FileManager.default.moveItem(at: saved, to: db)
    try Data("foreign-owner".utf8).write(to: owner)
    do {
      _ = try M0FileProtectionProbe(storage: storage.path, run: run)
      fatalError("foreign owner must reject")
    } catch { }
    let serialized = try JSONSerialization.data(withJSONObject: observed)
    assert(!String(decoding: serialized, as: UTF8.self).contains("synthetic-file-protection-control"))
    assert(probe.closeResources().values.allSatisfy { $0 == 0 })
    let otherDescriptor = open(owner.path, O_RDONLY | O_CLOEXEC)
    assert(otherDescriptor >= 0)
    assert(probe.closeResources().values.allSatisfy { $0 == 0 })
    assert(fcntl(otherDescriptor, F_GETFD) != -1)
    assert(close(otherDescriptor) == 0)
    let closedControl = probe.sample()["writeControl"] as! [String: Any]
    assert((closedControl["pwrite"] as! [String: Any])["errno"] as? Int == Int(EBADF))
    print("iOS file observation host checks passed: real control pwrite / fsync / readback; database preservation / identity; simulated denial / timeout / cancellation. No device protection claim.")
  }
}
