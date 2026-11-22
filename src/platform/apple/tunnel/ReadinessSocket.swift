import Darwin

final class ReadinessSocket {
    let port: UInt16
    private var descriptor: Int32

    init?() {
        let socket = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard socket >= 0 else { return nil }
        var address = Self.loopback(0)
        guard Self.withSocketAddress(&address, { bind(socket, $0, $1) }) == 0 else {
            close(socket)
            return nil
        }
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let result = Self.withSocketAddress(&address) { getsockname(socket, $0, &length) }
        guard result == 0 else {
            close(socket)
            return nil
        }
        descriptor = socket
        port = UInt16(bigEndian: address.sin_port)
    }

    func release() {
        guard descriptor >= 0 else { return }
        close(descriptor)
        descriptor = -1
    }

    deinit { release() }

    static func responds(on port: UInt16) -> Bool {
        let socket = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard socket >= 0 else { return false }
        defer { close(socket) }

        var timeout = timeval(tv_sec: 0, tv_usec: 150_000)
        guard setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size)) == 0
        else { return false }
        var noSignal: Int32 = 1
        guard setsockopt(socket, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size)) == 0
        else { return false }

        var address = loopback(port)
        guard withSocketAddress(&address, { connect(socket, $0, $1) }) == 0 else { return false }
        let negotiation: [UInt8] = [5, 1, 0]
        guard negotiation.withUnsafeBytes({ send(socket, $0.baseAddress, $0.count, 0) }) == negotiation.count
        else { return false }
        var response = [UInt8](repeating: 0, count: 2)
        let count = response.withUnsafeMutableBytes { recv(socket, $0.baseAddress, $0.count, MSG_WAITALL) }
        return count == response.count && response == [5, 0]
    }

    private static func loopback(_ port: UInt16) -> sockaddr_in {
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr = in_addr(s_addr: INADDR_LOOPBACK.bigEndian)
        return address
    }

    private static func withSocketAddress<Result>(
        _ address: inout sockaddr_in,
        _ body: (UnsafeMutablePointer<sockaddr>, socklen_t) -> Result
    ) -> Result {
        withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                body($0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
    }
}
