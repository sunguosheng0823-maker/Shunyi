// RustDesk scrap expects this host-provided macOS backing-scale hook.
// Adapted from rustdesk/src/platform/macos.mm at 851d2df88cc8ef7a8368f74e8b2e7254861ee00a.
// SPDX-License-Identifier: AGPL-3.0-only
#import <AppKit/AppKit.h>
#include <stdint.h>

extern "C" float BackingScaleFactor(uint32_t display) {
    @autoreleasepool {
        for (NSScreen *screen in [NSScreen screens]) {
            NSNumber *number = screen.deviceDescription[@"NSScreenNumber"];
            if (number.unsignedIntValue == display) return (float)screen.backingScaleFactor;
        }
        return 1.0f;
    }
}
