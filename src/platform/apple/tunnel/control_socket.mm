#include "manager.h"

#include <cerrno>
#include <cstdlib>
#include <string>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace {
constexpr NSUInteger maxConfigurationBytes = 64 * 1024;
constexpr std::uint8_t invalid = 255;

bool ownProcess(int connection) {
    pid_t peer{};
    socklen_t size = sizeof(peer);
    return getsockopt(connection, SOL_LOCAL, LOCAL_PEERPID, &peer, &size) == 0 &&
        peer == getpid();
}

ssize_t readRetrying(int connection, void *buffer, size_t size) {
    ssize_t count;
    do {
        count = read(connection, buffer, size);
    } while (count < 0 && errno == EINTR);
    return count;
}

NSData *readConfiguration(int connection) {
    NSMutableData *configuration = [NSMutableData data];
    std::uint8_t buffer[4096];
    for (;;) {
        const auto count = readRetrying(connection, buffer, sizeof(buffer));
        if (count == 0) return configuration.length == 0 ? nil : configuration;
        if (count < 0 || configuration.length + static_cast<NSUInteger>(count) > maxConfigurationBytes) return nil;
        [configuration appendBytes:buffer length:static_cast<NSUInteger>(count)];
    }
}

std::uint8_t handleRequest(int connection) {
    std::uint8_t command;
    if (readRetrying(connection, &command, 1) != 1) return invalid;
    switch (command) {
    case '+': {
        NSData *configuration = readConfiguration(connection);
        if (configuration == nil || !blazer::start(configuration)) return invalid;
        return static_cast<std::uint8_t>(blazer::TunnelState::Starting);
    }
    case '-':
        blazer::stop();
        return static_cast<std::uint8_t>(blazer::TunnelState::Stopping);
    case '?':
        return static_cast<std::uint8_t>(blazer::state());
    default:
        return invalid;
    }
}

void serve(int connection) {
    if (!ownProcess(connection)) return;
    const int enabled = 1;
    const timeval timeout{1, 0};
    if (setsockopt(connection, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof(enabled)) != 0 ||
        setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) return;
    const auto reply = handleRequest(connection);
    write(connection, &reply, sizeof(reply));
}

void listenForControls() {
    NSString *filename = [NSString stringWithFormat:@"bz-ne.%d", getpid()];
    NSString *path = [NSTemporaryDirectory() stringByAppendingPathComponent:filename];
    const char *name = path.fileSystemRepresentation;
    sockaddr_un address{};
    if (name == nullptr || strlcpy(address.sun_path, name, sizeof(address.sun_path)) >= sizeof(address.sun_path)) return;
    address.sun_family = AF_UNIX;
    address.sun_len = SUN_LEN(&address);
    const int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listener < 0) return;
    struct stat existing{};
    if (lstat(name, &existing) == 0 &&
        (!S_ISSOCK(existing.st_mode) || existing.st_uid != geteuid() || unlink(name) != 0)) {
        close(listener);
        return;
    }
    if (bind(listener, reinterpret_cast<sockaddr *>(&address), address.sun_len) != 0 ||
        chmod(name, S_IRUSR | S_IWUSR) != 0 || listen(listener, 4) != 0) {
        close(listener);
        return;
    }

    dispatch_queue_t queue = dispatch_queue_create("com.kobaproduction.blazer.tunnel-control", DISPATCH_QUEUE_SERIAL);
    dispatch_source_t source = dispatch_source_create(DISPATCH_SOURCE_TYPE_READ, listener, 0, queue);
    dispatch_source_set_event_handler(source, ^{
        @autoreleasepool {
            const int connection = accept(listener, nullptr, nullptr);
            if (connection < 0) return;
            serve(connection);
            close(connection);
        }
    });
    // The app owns this listener for the lifetime of its process.
    static dispatch_source_t controlSource;
    static std::string socketPath;
    socketPath = name;
    atexit([] { unlink(socketPath.c_str()); });
    controlSource = source;
    dispatch_resume(controlSource);
}

__attribute__((constructor)) void startControlSocket() {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool { listenForControls(); }
    });
}
}
