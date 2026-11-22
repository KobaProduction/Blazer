import Dispatch
import Foundation
import NetworkExtension

@main
struct SystemExtension {
    static func main() {
        autoreleasepool {
            NEProvider.startSystemExtensionMode()
        }
        dispatchMain()
    }
}
