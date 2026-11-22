import Darwin
import Foundation
import Network
import NetworkExtension

final class PhysicalInterface: @unchecked Sendable {
    static let shared = PhysicalInterface()
    private let lock = NSLock()
    private var index: UInt32 = 0

    static func index(on path: NWPath) -> UInt32 {
        guard path.status == .satisfied else { return 0 }
        let physical: [NWInterface.InterfaceType] = [.wifi, .wiredEthernet, .cellular]
        guard let interface = path.availableInterfaces.first(where: {
            physical.contains($0.type) && path.usesInterfaceType($0.type)
        }) else { return 0 }
        return if_nametoindex(interface.name)
    }

    func store(_ value: UInt32) {
        lock.lock()
        index = value
        lock.unlock()
    }

    func bind(_ fd: Int32) -> Bool {
        lock.lock()
        var interfaceIndex = index
        lock.unlock()
        guard interfaceIndex != 0 else { return false }

        var address = sockaddr_storage()
        var size = socklen_t(MemoryLayout.size(ofValue: address))
        let result = withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &size) }
        }
        guard result == 0 else { return false }
        switch Int32(address.ss_family) {
        case AF_INET:
            return setsockopt(fd, IPPROTO_IP, IP_BOUND_IF, &interfaceIndex, socklen_t(MemoryLayout<UInt32>.size)) == 0
        case AF_INET6:
            return setsockopt(fd, IPPROTO_IPV6, IPV6_BOUND_IF, &interfaceIndex, socklen_t(MemoryLayout<UInt32>.size)) == 0
        default:
            return false
        }
    }
}

func protectSocket(_ fd: Int32) -> Bool {
    PhysicalInterface.shared.bind(fd)
}

func writePacket(_ data: UnsafePointer<UInt8>?, _ length: Int, _ context: Int) -> Bool {
    guard let data, (1...Int(UInt16.max)).contains(length),
          let pointer = UnsafeRawPointer(bitPattern: context)
    else { return false }
    let family: Int32
    switch data.pointee >> 4 {
    case 4: family = AF_INET
    case 6: family = AF_INET6
    default: return false
    }
    // The core only lends its buffer until return. Network Extension receives owned bytes.
    let flow = Unmanaged<NEPacketTunnelFlow>.fromOpaque(pointer).takeUnretainedValue()
    return flow.writePackets([Data(bytes: data, count: length)], withProtocols: [NSNumber(value: family)])
}

func releaseFlow(_ context: Int) {
    if let pointer = UnsafeRawPointer(bitPattern: context) {
        Unmanaged<NEPacketTunnelFlow>.fromOpaque(pointer).release()
    }
}

func submitPackets(_ packets: [Data], handle: CLong) {
    // The engine has a bounded queue. It accepts the leading part of the batch when busy.
    let backing = packets.prefix(128).map { $0 as NSData }
    let descriptors = backing.map { packet in
        BlazeCorePacket(data: packet.bytes.assumingMemoryBound(to: UInt8.self), length: packet.length)
    }
    withExtendedLifetime(backing) {
        descriptors.withUnsafeBufferPointer { buffer in
            _ = blazecore_submit_packets(handle, buffer.baseAddress, buffer.count)
        }
    }
}
