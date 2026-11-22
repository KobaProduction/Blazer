#include "manager.h"
#include <TargetConditionals.h>

#if TARGET_OS_OSX
#import <SystemExtensions/SystemExtensions.h>

@interface BlazerExtensionActivation : NSObject<OSSystemExtensionRequestDelegate>
@property(nonatomic, copy) void (^completion)(BOOL);
@end

static BlazerExtensionActivation *activation;

@implementation BlazerExtensionActivation

- (void)finish:(BOOL)active {
    auto reply = self.completion;
    self.completion = nil;
    activation = nil;
    if (reply != nil) reply(active);
}

- (void)request:(OSSystemExtensionRequest *)request didFinishWithResult:(OSSystemExtensionRequestResult)result {
    [self finish:result == OSSystemExtensionRequestCompleted];
}

- (void)request:(OSSystemExtensionRequest *)request didFailWithError:(NSError *)error {
    [self finish:NO];
}

- (void)requestNeedsUserApproval:(OSSystemExtensionRequest *)request {
    // macOS presents the approval UI. The connect request can still be cancelled.
}

- (OSSystemExtensionReplacementAction)request:(OSSystemExtensionRequest *)request
                  actionForReplacingExtension:(OSSystemExtensionProperties *)existing
                                 withExtension:(OSSystemExtensionProperties *)replacement {
    return OSSystemExtensionReplacementActionReplace;
}

@end
#endif

namespace blazer {
void activateExtension(NSString *identifier, void (^completion)(BOOL)) {
#if TARGET_OS_OSX
    if (activation != nil) {
        activation.completion = completion;
        return;
    }
    activation = [BlazerExtensionActivation new];
    activation.completion = completion;
    auto *request = [OSSystemExtensionRequest activationRequestForExtension:identifier
                                                                     queue:dispatch_get_main_queue()];
    request.delegate = activation;
    [[OSSystemExtensionManager sharedManager] submitRequest:request];
#else
    completion(YES);
#endif
}

void cancelExtensionActivation() {
#if TARGET_OS_OSX
    activation.completion = nil;
#endif
}
}
