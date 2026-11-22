#include "manager.h"

#import <Foundation/Foundation.h>
#import <NetworkExtension/NetworkExtension.h>
#include <TargetConditionals.h>

#include <atomic>

namespace blazer {
namespace {
#if TARGET_OS_OSX
NSString *const providerID = @"com.kobaproduction.blazer.packet-tunnel";
#else
NSString *const providerID = @"com.kobaproduction.blazer.tunnel";
#endif
std::atomic<TunnelState> current{TunnelState::Idle};
std::atomic<std::uint64_t> generation{0};
NETunnelProviderSession *session;
id observer;

bool isCurrent(std::uint64_t request) {
    return generation.load() == request && current.load() == TunnelState::Starting;
}

bool isOffline(NEVPNStatus status) {
    return status == NEVPNStatusDisconnected || status == NEVPNStatusInvalid;
}

void failStart() {
    auto expected = TunnelState::Starting;
    current.compare_exchange_strong(expected, TunnelState::Failed);
}

void forgetSession() {
    if (observer != nil) {
        [[NSNotificationCenter defaultCenter] removeObserver:observer];
        observer = nil;
    }
    session = nil;
}

void updateStatus(bool notification) {
    const auto status = session.status;
    auto previous = current.load();
    if (previous == TunnelState::Stopping) {
        if (session == nil || isOffline(status)) {
            forgetSession();
            current.store(TunnelState::Idle);
        }
        return;
    }
    if (status == NEVPNStatusConnected || status == NEVPNStatusReasserting) {
        if (previous == TunnelState::Starting || previous == TunnelState::Running) {
            current.compare_exchange_strong(previous, TunnelState::Running);
        }
    } else if (isOffline(status) && notification) {
        forgetSession();
        const auto next = previous == TunnelState::Starting ? TunnelState::Failed : TunnelState::Idle;
        current.compare_exchange_strong(previous, next);
    }
}

void connect(NETunnelProviderManager *manager, NSData *configuration, std::uint64_t request) {
    if (!isCurrent(request)) return;
    auto *connection = manager.connection;
    if (![connection isKindOfClass:NETunnelProviderSession.class]) {
        failStart();
        return;
    }

    forgetSession();
    session = (NETunnelProviderSession *)connection;
    observer = [[NSNotificationCenter defaultCenter]
        addObserverForName:NEVPNStatusDidChangeNotification
        object:connection
        queue:NSOperationQueue.mainQueue
        usingBlock:^(NSNotification *event) {
            if (event.object == session) updateStatus(true);
        }];
    NSError *error = nil;
    if (![session startTunnelWithOptions:@{@"packetTunnelConfiguration": configuration} andReturnError:&error]) {
        forgetSession();
        failStart();
        return;
    }
    updateStatus(false);
}

NETunnelProviderManager *matchingManager(NSArray<NETunnelProviderManager *> *managers) {
    for (NETunnelProviderManager *manager in managers) {
        auto *protocol = manager.protocolConfiguration;
        if ([protocol isKindOfClass:NETunnelProviderProtocol.class] &&
            [((NETunnelProviderProtocol *)protocol).providerBundleIdentifier isEqualToString:providerID]) {
            return manager;
        }
    }
    return [NETunnelProviderManager new];
}

void configure(NETunnelProviderManager *manager, NSString *endpoint,
               NSData *configuration, std::uint64_t request, unsigned attempts) {
    if (!isCurrent(request)) return;
    if (!isOffline(manager.connection.status)) {
        // A previous app process may have left this app's tunnel connected.
        if (attempts == 0) [manager.connection stopVPNTunnel];
        if (attempts >= 50) {
            failStart();
            return;
        }
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
            configure(manager, endpoint, configuration, request, attempts + 1);
        });
        return;
    }

    auto *protocol = [NETunnelProviderProtocol new];
    protocol.providerBundleIdentifier = providerID;
    protocol.serverAddress = endpoint;
    manager.protocolConfiguration = protocol;
    manager.localizedDescription = @"Blazer";
    manager.enabled = YES;
    [manager saveToPreferencesWithCompletionHandler:^(NSError *error) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!isCurrent(request)) return;
            if (error != nil) {
                failStart();
                return;
            }
            [manager loadFromPreferencesWithCompletionHandler:^(NSError *loadError) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (!isCurrent(request)) return;
                    if (loadError != nil) failStart();
                    else connect(manager, configuration, request);
                });
            }];
        });
    }];
}

void loadManager(NSString *host, NSData *configuration, std::uint64_t request) {
    [NETunnelProviderManager loadAllFromPreferencesWithCompletionHandler:
        ^(NSArray<NETunnelProviderManager *> *managers, NSError *error) {
            dispatch_async(dispatch_get_main_queue(), ^{
                if (!isCurrent(request)) return;
                if (error != nil) failStart();
                else configure(matchingManager(managers), host, configuration, request, 0);
            });
        }];
}
}

bool start(NSData *configuration) noexcept {
    id envelope = [NSJSONSerialization JSONObjectWithData:configuration options:0 error:nullptr];
    if (![envelope isKindOfClass:NSDictionary.class]) return false;
    NSString *host = ((NSDictionary *)envelope)[@"endpoint"];
    if (![host isKindOfClass:NSString.class] || host.length == 0) return false;
    auto expected = TunnelState::Idle;
    if (!current.compare_exchange_strong(expected, TunnelState::Starting)) return false;
    const auto request = generation.fetch_add(1) + 1;
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!isCurrent(request)) return;
        activateExtension(providerID, ^(BOOL active) {
            if (!isCurrent(request)) return;
            if (active) loadManager(host, configuration, request);
            else failStart();
        });
    });
    return true;
}

void stop() noexcept {
    generation.fetch_add(1);
    current.store(TunnelState::Stopping);
    dispatch_async(dispatch_get_main_queue(), ^{
        cancelExtensionActivation();
        [session stopVPNTunnel];
        updateStatus(false);
    });
}

TunnelState state() noexcept { return current.load(); }
}
