#pragma once

#import <Foundation/Foundation.h>
#include <cstdint>
#include <cstdint>

namespace blazer {
enum class TunnelState : std::uint8_t { Idle, Starting, Running, Failed, Stopping };

void activateExtension(NSString *identifier, void (^completion)(BOOL));
void cancelExtensionActivation();
bool start(NSData *configuration) noexcept;
void stop() noexcept;
TunnelState state() noexcept;
}
