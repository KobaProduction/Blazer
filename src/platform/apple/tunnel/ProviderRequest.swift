import Foundation
import NetworkExtension

struct ProviderRequest {
    private let endpoint: String
    private let mtu: Int
    private let records: [[String: Any]]
    let observatoryProfile: String?
    let payload: Data

    init?(_ payload: Data) {
        guard payload.count <= 64 * 1024,
              let body = (try? JSONSerialization.jsonObject(with: payload)) as? [String: Any],
              let endpoint = body["endpoint"] as? String,
              !endpoint.isEmpty, endpoint.utf8.count <= 253,
              let mtu = body["mtu"] as? Int, (1280...1500).contains(mtu),
              let records = body["records"] as? [[String: Any]],
              !records.isEmpty, records.count <= 1024,
              Self.validRecords(records, mtu: mtu)
        else { return nil }
        self.endpoint = endpoint
        self.payload = payload
        self.mtu = mtu
        self.records = records
        if let profile = body["observatoryProfile"] {
            guard let profile = profile as? String, profile.utf8.count <= 64 * 1024 else { return nil }
            observatoryProfile = profile
        } else { observatoryProfile = nil }
    }

    private static func validRecords(_ records: [[String: Any]], mtu: Int) -> Bool {
        let tunnels = records.filter { $0["device_name"] != nil }
        guard tunnels.count == 1,
              tunnels[0]["device_name"] as? String == "Blazer",
              tunnels[0]["mtu"] as? Int == mtu else { return false }
        return records.allSatisfy { record in
            guard record["device_fd"] == nil, record["auto_route"] == nil,
                  record["address"] == nil, record["clash_api_address"] == nil else { return false }
            return record["device_name"] != nil || record["client_group"] is String
        }
    }

    func configuration(readinessPort: UInt16, observedRecords: [[String: Any]]? = nil) -> String? {
        // The loopback listener only answers protocol negotiation. It cannot forward traffic.
        let health: [String: Any] = [
            "address": "127.0.0.1:\(readinessPort)",
            "protocol": ["type": "mixed"],
            "rules": "block-all"
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: (observedRecords ?? records) + [health]) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func networkSettings() -> NEPacketTunnelNetworkSettings {
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: endpoint)
        let ipv4 = NEIPv4Settings(addresses: ["198.18.0.1"], subnetMasks: ["255.255.255.0"])
        ipv4.includedRoutes = [.default()]
        settings.ipv4Settings = ipv4
        let ipv6 = NEIPv6Settings(addresses: ["fd9c:5d15:f7c6::1"], networkPrefixLengths: [64])
        ipv6.includedRoutes = [.default()]
        settings.ipv6Settings = ipv6
        let dns = NEDNSSettings(servers: ["198.18.0.2"])
        dns.matchDomains = [""]
        settings.dnsSettings = dns
        settings.mtu = NSNumber(value: mtu)
        return settings
    }
}
