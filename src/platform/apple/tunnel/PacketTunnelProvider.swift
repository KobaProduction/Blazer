import Darwin
import Foundation
import Network
import NetworkExtension

@objc(BlazerPacketTunnelProvider)
final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let queue = DispatchQueue(label: "com.kobaproduction.blazer.tunnel")
    private var monitor: NWPathMonitor?
    private var watchdog: DispatchSourceTimer?
    private var generation: UUID?
    private var completion: ((Error?) -> Void)?
    private var handle: CLong = 0
    private var observer: ObserverBridge?

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        queue.async { self.start(options, completionHandler) }
    }

    private func start(_ options: [String: NSObject]?, _ reply: @escaping (Error?) -> Void) {
        guard generation == nil else { return reply(TunnelError.alreadyStarted) }
        if observer != nil || handle > 0 {
            guard stop() else { return reply(TunnelError.observerStopped) }
        }
        guard let payload = options?["packetTunnelConfiguration"] as? Data,
              let request = ProviderRequest(payload)
        else { return reply(TunnelError.invalidConfiguration) }

        let attempt = UUID()
        generation = attempt
        completion = reply
        monitorNetwork(request, attempt)
        queue.asyncAfter(deadline: .now() + 12) { [weak self] in
            guard let self, self.generation == attempt, self.completion != nil else { return }
            self.fail(TunnelError.startTimeout)
        }
    }

    private func monitorNetwork(_ request: ProviderRequest, _ attempt: UUID) {
        let monitor = NWPathMonitor()
        self.monitor = monitor
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self, self.generation == attempt else { return }
            let index = PhysicalInterface.index(on: path)
            PhysicalInterface.shared.store(index)
            if index != 0, self.handle == 0 {
                self.startCore(request, attempt)
            }
        }
        monitor.start(queue: queue)
    }

    private func startCore(_ request: ProviderRequest, _ attempt: UUID) {
        do {
            if request.observatoryProfile != nil {
                observer = try ObserverBridge(payload: request.payload)
            }
        } catch { return fail(error) }
        guard blazecore_init("off") == 0,
              let listener = ReadinessSocket(),
              let configuration = request.configuration(readinessPort: listener.port, observedRecords: observer?.records)
        else { return fail(TunnelError.engineStart) }

        listener.release()
        do { try observer?.releasePorts() } catch { return fail(error) }
        let context = Unmanaged.passRetained(packetFlow).toOpaque()
        handle = configuration.withCString { yaml in
            blazecore_start_packet_flow(
                yaml, protectSocket, writePacket, releaseFlow, Int(bitPattern: context)
            )
        }
        guard handle > 0 else { return fail(TunnelError.engineStart) }
        waitForCore(request, listener.port, attempt)
    }

    private func waitForCore(_ request: ProviderRequest, _ port: UInt16, _ attempt: UUID) {
        guard generation == attempt else { return }
        guard blazecore_is_running() else { return fail(TunnelError.engineStart) }
        if ReadinessSocket.responds(on: port) {
            do { try observer?.start(coreGeneration: handle) } catch { return fail(error) }
            applyNetworkSettings(request, attempt)
        } else {
            queue.asyncAfter(deadline: .now() + .milliseconds(60)) { [weak self] in
                self?.waitForCore(request, port, attempt)
            }
        }
    }

    private func applyNetworkSettings(_ request: ProviderRequest, _ attempt: UUID) {
        setTunnelNetworkSettings(request.networkSettings()) { [weak self] error in
            guard let self else { return }
            self.queue.async {
                guard self.generation == attempt else { return }
                if let error { return self.fail(error) }
                guard blazecore_is_running() else { return self.fail(TunnelError.engineStart) }
                self.readPackets(attempt)
                self.watchCore(attempt)
                self.completeStart(nil)
            }
        }
    }

    private func watchCore(_ attempt: UUID) {
        let watchdog = DispatchSource.makeTimerSource(queue: queue)
        watchdog.schedule(deadline: .now() + 1, repeating: 1, leeway: .milliseconds(250))
        watchdog.setEventHandler { [weak self] in
            guard let self, self.generation == attempt else { return }
            if !blazecore_is_running() { self.fail(TunnelError.engineStopped) }
            else if let observer = self.observer, !observer.running { self.fail(TunnelError.observerStopped) }
        }
        self.watchdog = watchdog
        watchdog.resume()
    }

    private func readPackets(_ attempt: UUID) {
        packetFlow.readPackets { [weak self] packets, _ in
            guard let self else { return }
            self.queue.async {
                guard self.generation == attempt else { return }
                submitPackets(packets, handle: self.handle)
                self.readPackets(attempt)
            }
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        queue.async {
            let stopped = self.stop()
            self.completeStart(stopped ? TunnelError.cancelled : TunnelError.observerStopped)
            if !stopped { self.cancelTunnelWithError(TunnelError.observerStopped) }
            completionHandler()
        }
    }

    private func fail(_ error: Error) {
        let starting = completion != nil
        let failure: Error = stop() ? error : TunnelError.observerStopped
        if starting { completeStart(failure) } else { cancelTunnelWithError(failure) }
    }

    private func completeStart(_ error: Error?) {
        let reply = completion
        completion = nil
        reply?(error)
    }

    @discardableResult
    private func stop() -> Bool {
        generation = nil
        monitor?.cancel()
        monitor = nil
        watchdog?.cancel()
        watchdog = nil
        if let observer {
            guard observer.stop() else { return false }
            self.observer = nil
        }
        PhysicalInterface.shared.store(0)
        if handle > 0 { blazecore_stop(handle) }
        handle = 0
        return true
    }
}

enum TunnelError: Int, Error {
    case invalidConfiguration = 1, alreadyStarted, engineStart, startTimeout, engineStopped, cancelled
    case selectorUnavailable, observerStart, observerStopped
}
