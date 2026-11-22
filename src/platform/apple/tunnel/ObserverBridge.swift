import Foundation

/// Owns the shared Rust observer inside the packet-tunnel extension process.
/// Native callbacks call the core directly and never reenter the provider queue.
final class ObserverBridge {
    private var handle: UInt64
    let records: [[String: Any]]

    init(payload: Data) throws {
        guard blazer_core_selector_capabilities() & 1 != 0 else {
            throw TunnelError.selectorUnavailable
        }
        let prepared = payload.withUnsafeBytes { bytes in
            blazer_observer_prepare(slice_ref_uint8_t(
                ptr: bytes.bindMemory(to: UInt8.self).baseAddress!, len: bytes.count
            ))
        }
        defer { blazer_observer_free_configuration(prepared.configuration) }
        guard prepared.code == 0, prepared.handle > 0 else { throw TunnelError.observerStart }
        let data = Data(bytes: prepared.configuration.ptr, count: prepared.configuration.len)
        guard let records = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else {
            _ = blazer_observer_stop(prepared.handle)
            throw TunnelError.invalidConfiguration
        }
        self.handle = prepared.handle
        self.records = records
    }

    func releasePorts() throws {
        guard blazer_observer_release_ports(handle) == 0 else { throw TunnelError.observerStart }
    }

    func start(coreGeneration: CLong) throws {
        guard coreGeneration > 0,
              blazer_observer_start(handle, UInt64(coreGeneration),
                                    blazer_core_selector_ready, blazer_core_selector_select) == 0
        else { throw TunnelError.observerStart }
    }

    var running: Bool { handle > 0 && blazer_observer_status(handle) == 1 }

    @discardableResult
    func stop() -> Bool {
        guard handle > 0 else { return true }
        let result = blazer_observer_stop(handle)
        guard result == 0 || result == -2 else { return false }
        handle = 0
        return true
    }

    deinit { stop() }
}
