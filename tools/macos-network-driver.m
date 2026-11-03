// Verification-only events delivered to this process's production game view.
// Commands use private files; no Accessibility, global events, or clipboard.
#import <Cocoa/Cocoa.h>

static NSTimer *timer;
static NSWindow *window;
static NSString *commandPath, *ackPath, *probePath, *role;
static NSDictionary *pending;
static NSUInteger lastID;
static NSTimeInterval pendingSince;
static NSTimeInterval lastProbe;
static NSUInteger folderPickerCount;

static NSDictionary *ReadJSON(NSString *path) {
    NSData *data = [NSData dataWithContentsOfFile:path];
    return data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : nil;
}

static void Acknowledge(NSUInteger identifier, NSString *error) {
    NSDictionary *value = @{
        @"id": @(identifier), @"ok": @(error == nil), @"error": error ?: [NSNull null],
        @"pid": @(NSProcessInfo.processInfo.processIdentifier), @"role": role,
        @"window_visible": @(window.visible),
        @"window_frame": NSStringFromRect(window.frame),
        @"content_size": NSStringFromSize(window.contentView.bounds.size),
        @"screen_frame": NSStringFromRect(window.screen.frame),
        @"application_active": @(NSApp.active),
        @"folder_picker_count": @(folderPickerCount),
        @"view_class": NSStringFromClass(window.contentView.class),
        @"backing_scale_factor": @(window.backingScaleFactor),
        @"backend": @"Cocoa NSEvents confined to this process's production game view"
    };
    [[NSJSONSerialization dataWithJSONObject:value options:NSJSONWritingPrettyPrinted error:NULL]
        writeToFile:ackPath atomically:YES];
}

static void Key(unsigned short code, BOOL down, NSString *text) {
    NSEvent *event = [NSEvent keyEventWithType:down ? NSEventTypeKeyDown : NSEventTypeKeyUp
        location:NSZeroPoint modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
        windowNumber:window.windowNumber context:nil characters:text ?: @""
        charactersIgnoringModifiers:text ?: @"" isARepeat:NO keyCode:code];
    if (down) [window.contentView keyDown:event];
    else [window.contentView keyUp:event];
}
static void Tap(unsigned short code) { Key(code, YES, nil); Key(code, NO, nil); }

static void Type(NSString *value) {
    for (NSUInteger i = 0; i < value.length; ++i) {
        NSString *character = [value substringWithRange:NSMakeRange(i, 1)];
        Key(0, YES, character);
        Key(0, NO, character);
    }
}

static void CancelFolderPicker(void) {
    [NSApp activateIgnoringOtherApps:YES];
    [window makeKeyAndOrderFront:nil];
    __block NSTimer *chooser;
    NSTimeInterval expires = NSProcessInfo.processInfo.systemUptime + 10;
    chooser = [NSTimer timerWithTimeInterval:.05 repeats:YES block:^(NSTimer *tick) {
        NSWindow *modal = window.attachedSheet;
        if ([modal isKindOfClass:NSOpenPanel.class]) {
            folderPickerCount++;
            [NSApp endSheet:modal returnCode:NSModalResponseCancel];
            [tick invalidate];
            chooser = nil;
        } else if (NSProcessInfo.processInfo.systemUptime >= expires) {
            [tick invalidate];
            chooser = nil;
        }
    }];
    [NSRunLoop.mainRunLoop addTimer:chooser forMode:NSRunLoopCommonModes];
    Tap(36);
}

static void Click(double x, double y) {
    NSSize size = window.contentView.bounds.size;
    double scale = MIN(size.width / 640., size.height / 480.);
    NSPoint point = NSMakePoint((size.width - 640 * scale) / 2 + x * scale,
        size.height - ((size.height - 480 * scale) / 2 + y * scale));
    for (NSNumber *kind in @[@(NSEventTypeMouseMoved), @(NSEventTypeLeftMouseDown), @(NSEventTypeLeftMouseUp)]) {
        NSEvent *event = [NSEvent mouseEventWithType:kind.unsignedIntegerValue location:point
            modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
            windowNumber:window.windowNumber context:nil eventNumber:0 clickCount:1 pressure:1];
        if (kind.unsignedIntegerValue == NSEventTypeMouseMoved) [window.contentView mouseMoved:event];
        else if (kind.unsignedIntegerValue == NSEventTypeLeftMouseDown) [window.contentView mouseDown:event];
        else [window.contentView mouseUp:event];
    }
}

__attribute__((constructor)) static void InstallDriver(void) {
    const char *command = getenv("ATOMIC_BOMBERMAN_MAC_NETWORK_COMMAND");
    const char *ack = getenv("ATOMIC_BOMBERMAN_MAC_NETWORK_ACK");
    const char *probe = getenv("ATOMIC_BOMBERMAN_STATE_PROBE");
    if (!command || !ack || !probe) return;
    commandPath = [NSString stringWithUTF8String:command];
    ackPath = [NSString stringWithUTF8String:ack];
    probePath = [NSString stringWithUTF8String:probe];
    // A mod switch execs the game in place. Ignore the command that caused
    // that restart while preserving monotonically increasing harness IDs.
    lastID = [ReadJSON(commandPath)[@"id"] unsignedIntegerValue];
    role = [NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_MAC_NETWORK_ROLE") ?: "game"];
    dispatch_async(dispatch_get_main_queue(), ^{
        timer = [NSTimer scheduledTimerWithTimeInterval:.04 repeats:YES block:^(NSTimer *unused) {
            if (!window) {
                for (NSWindow *candidate in NSApp.windows) {
                    if ([candidate.title hasPrefix:@"Atomic Bomberman"]) { window = candidate; break; }
                }
                if (!window) return;
                window.title=[NSString stringWithFormat:@"Atomic Bomberman - automated check (%@)",role];
                [window setContentSize:NSMakeSize(640, 480)];
                NSRect screen = window.screen.visibleFrame;
                CGFloat x = [role isEqual:@"host"] ? NSMinX(screen) + 15 : NSMaxX(screen) - window.frame.size.width - 15;
                [window setFrameOrigin:NSMakePoint(x, NSMaxY(screen) - window.frame.size.height - 25)];
                [window orderFront:nil];
                Acknowledge(0, nil);
            }
            NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
            if (pending) {
                BOOL capture = [pending[@"op"] isEqual:@"capture"];
                BOOL ready = now - pendingSince >= .12;
                if (capture) {
                    ready = ReadJSON(probePath) != nil;
                    // The window exists before initial media loading finishes.
                    // A probe key consumed during that async loading frame is
                    // retried once the production update loop is available.
                    if (!ready && now - lastProbe >= .25) {
                        Tap(111);
                        lastProbe = now;
                    }
                    NSString *png = pending[@"png"];
                    if (png.length) ready = ready && [[NSFileManager defaultManager] fileExistsAtPath:png];
                }
                if (ready || now - pendingSince > 30) {
                    lastID = [pending[@"id"] unsignedIntegerValue];
                    Acknowledge(lastID, ready ? nil : @"State/screenshot capture timed out");
                    pending = nil;
                }
                return;
            }
            NSDictionary *command = ReadJSON(commandPath);
            NSUInteger identifier = [command[@"id"] unsignedIntegerValue];
            if (!command || identifier <= lastID) return;
            NSString *operation = command[@"op"];
            if ([operation isEqual:@"quit"]) {
                lastID = identifier;
                Acknowledge(identifier, nil);
                [timer invalidate];
                [NSApp terminate:nil];
                return;
            }
            if ([operation isEqual:@"capture"]) {
                [[NSFileManager defaultManager] removeItemAtPath:probePath error:NULL];
                NSString *png = command[@"png"];
                if (png.length) {
                    [[NSFileManager defaultManager] removeItemAtPath:png error:NULL];
                    setenv("ATOMIC_BOMBERMAN_SCREENSHOT_PROBE", png.UTF8String, 1);
                } else unsetenv("ATOMIC_BOMBERMAN_SCREENSHOT_PROBE");
                Tap(111); // Existing opt-in F12 state probe, after the game draws.
                lastProbe = now;
            } else if ([operation isEqual:@"show"]) {
                [NSApp unhide:nil];
                [window deminiaturize:nil];
                [window orderFront:nil];
            } else if ([operation isEqual:@"tap"]) Tap([command[@"code"] unsignedShortValue]);
            else if ([operation isEqual:@"cancel_folder_picker"]) CancelFolderPicker();
            else if ([operation isEqual:@"key"]) Key([command[@"code"] unsignedShortValue], [command[@"down"] boolValue], nil);
            else if ([operation isEqual:@"type"]) Type(command[@"text"]);
            else if ([operation isEqual:@"click"]) Click([command[@"x"] doubleValue], [command[@"y"] doubleValue]);
            else {
                lastID = identifier;
                Acknowledge(identifier, @"Unknown harness command");
                return;
            }
            pending = command;
            pendingSince = now;
        }];
    });
}
