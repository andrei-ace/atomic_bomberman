// Development-only Cocoa events sent to this process's own game view. This
// does not request Accessibility access or send input to other applications.
#import <Cocoa/Cocoa.h>

static NSTimer *timer;
static NSWindow *window;
static NSMutableArray *steps, *checks;
static NSString *probePath, *settingsPath, *reportPath;
static NSDictionary *initialPositions;
static NSUInteger indexInSteps, ticks, delay;
static BOOL finished;
static BOOL launcherMode;

static NSButton *FindButton(NSView *view, NSString *title) {
    if ([view isKindOfClass:NSButton.class] && [((NSButton *)view).title isEqual:title]) return (NSButton *)view;
    for (NSView *child in view.subviews) {
        NSButton *found = FindButton(child, title);
        if (found) return found;
    }
    return nil;
}

static NSDictionary *ReadJSON(NSString *path) {
    NSData *data = [NSData dataWithContentsOfFile:path];
    return data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : nil;
}
static NSDictionary *State(void) { return ReadJSON(probePath); }
static void Finish(BOOL passed, NSString *error) {
    if (finished) return;
    finished = YES;
    [timer invalidate];
    NSDictionary *report = @{
        @"passed": @(passed), @"error": error ?: [NSNull null], @"checks": checks ?: @[],
        @"backend": launcherMode ? @"Native AppKit first-launch window and NSOpenPanel" : @"Cocoa NSEvents delivered to the production game view in its own process",
        @"backing_scale_factor": @(window.backingScaleFactor),
        @"window_style_mask": @(window.styleMask),
        @"window_collection_behavior": @(window.collectionBehavior),
        @"application_active": @(NSApp.active),
        @"window_frame": NSStringFromRect(window.frame),
        @"screen_frame": NSStringFromRect(window.screen.frame),
        @"window_visible": @(window.visible),
        @"physical_keyboard_verified": @NO, @"physical_controller_verified": @NO,
        @"limitations": @[@"Synthetic Cocoa events verify software input and window behavior; physical devices are recorded separately."]
    };
    [[NSJSONSerialization dataWithJSONObject:report options:NSJSONWritingPrettyPrinted error:NULL]
        writeToFile:reportPath atomically:YES];
    [NSApp terminate:nil];
}
static BOOL Check(BOOL passed, NSString *name) {
    if (!passed) { Finish(NO, name); return NO; }
    [checks addObject:name];
    return YES;
}
static void Key(unsigned short code, BOOL down) {
    NSEvent *event = [NSEvent keyEventWithType:down ? NSEventTypeKeyDown : NSEventTypeKeyUp
        location:NSZeroPoint modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
        windowNumber:window.windowNumber context:nil characters:@" " charactersIgnoringModifiers:@" "
        isARepeat:NO keyCode:code];
    if (down) [window.contentView keyDown:event];
    else [window.contentView keyUp:event];
}
static void Tap(unsigned short code) { Key(code, YES); Key(code, NO); }
static void Modifiers(NSUInteger flags) {
    NSEvent *event = [NSEvent keyEventWithType:NSEventTypeFlagsChanged location:NSZeroPoint
        modifierFlags:flags timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber
        context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:56];
    [window.contentView flagsChanged:event];
}
static void Capture(void) {
    [[NSFileManager defaultManager] removeItemAtPath:probePath error:NULL];
    Tap(111); // F12: existing debug-only game state probe.
}
static void Click(double x, double y) {
    NSSize size = window.contentView.bounds.size;
    double scale = MIN(size.width / 640., size.height / 480.);
    NSPoint point = NSMakePoint((size.width - 640 * scale) / 2 + x * scale,
        size.height - ((size.height - 480 * scale) / 2 + y * scale));
    for (NSNumber *kind in @[@(NSEventTypeMouseMoved), @(NSEventTypeLeftMouseDown), @(NSEventTypeLeftMouseUp)]) {
        NSEvent *event = [NSEvent mouseEventWithType:kind.unsignedIntegerValue location:point modifierFlags:0
            timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
            eventNumber:0 clickCount:1 pressure:1];
        if (kind.unsignedIntegerValue == NSEventTypeMouseMoved) [window.contentView mouseMoved:event];
        else if (kind.unsignedIntegerValue == NSEventTypeLeftMouseDown) [window.contentView mouseDown:event];
        else [window.contentView mouseUp:event];
    }
}
static void Add(void (^block)(void)) { [steps addObject:[block copy]]; }
static NSDictionary *Positions(void) {
    NSArray *players = State()[@"simulation"][@"players"];
    return players.count >= 2 ? @{@"first": players[0][@"position"], @"second": players[1][@"position"]} : @{};
}
static void BuildSteps(void) {
    steps = [NSMutableArray array]; checks = [NSMutableArray array];
    Add(^{ [NSApp activateIgnoringOtherApps:YES]; [window makeKeyAndOrderFront:nil]; [window setContentSize:NSMakeSize(1280, 720)]; Capture(); });
    Add(^{ Check(fabs(window.contentView.bounds.size.width - 1280) < 2, @"resizable native window"); });
    Add(^{ Click(480, 260); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Controls"], @"letterboxed mouse coordinates open Controls"); });
    Add(^{ Click(300, 236); }); // Keyboard 1 action binding.
    Add(^{ Tap(6); }); // Z; do not probe while a binding capture is pending.
    Add(^{ Check([ReadJSON(settingsPath)[@"keyboard"][0][@"keys"][5] isEqual:@"Z"], @"keyboard remapping persists"); });
    Add(^{ Tap(53); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Main"], @"Escape returns to menu"); });
    Add(^{ Tap(103); delay = 15; }); // F11.
    Add(^{ Check((window.styleMask & NSWindowStyleMaskFullScreen)
        && [ReadJSON(settingsPath)[@"fullscreen"] boolValue], @"F11 enters fullscreen and saves preference"); });
    Add(^{ Tap(103); delay = 15; });
    Add(^{ Check(!(window.styleMask & NSWindowStyleMaskFullScreen)
        && ![ReadJSON(settingsPath)[@"fullscreen"] boolValue], @"F11 returns to window"); });
    Add(^{ Modifiers(0x100008 | 0x040001); Tap(3); }); // Command-Control-F.
    Add(^{ Modifiers(0); delay = 6; });
    Add(^{ Check((window.styleMask & NSWindowStyleMaskFullScreen)
        && [ReadJSON(settingsPath)[@"fullscreen"] boolValue], @"Command-Control-F enters fullscreen"); });
    Add(^{ Modifiers(0x100008 | 0x040001); Tap(3); });
    Add(^{ Modifiers(0); delay = 6; });
    Add(^{ Check(!(window.styleMask & NSWindowStyleMaskFullScreen)
        && ![ReadJSON(settingsPath)[@"fullscreen"] boolValue], @"Command-Control-F returns to window"); });
    Add(^{ Tap(36); delay = 18; }); // Start battle, allow native countdown.
    Add(^{ Capture(); });
    Add(^{
        if (!Check([State()[@"screen"] isEqual:@"Battle"]
            && [State()[@"simulation"][@"phase"] isEqual:@"Playing"], @"two-human battle starts")) return;
        initialPositions = Positions();
        Key(124, YES); Key(0, YES); // Right arrow and A, independent layouts.
    });
    Add(^{ Capture(); });
    Add(^{
        NSDictionary *positions = Positions();
        Key(124, NO); Key(0, NO);
        Check(![positions[@"first"] isEqual:initialPositions[@"first"]]
            && ![positions[@"second"] isEqual:initialPositions[@"second"]], @"simultaneous two-player movement");
    });
    Add(^{ Key(49, YES); Modifiers(0x020002); }); // Space and left Shift bombs.
    Add(^{ Capture(); });
    Add(^{
        Key(49, NO); Modifiers(0);
        NSMutableSet *owners = [NSMutableSet set];
        for (NSDictionary *bomb in State()[@"simulation"][@"bombs"]) [owners addObject:bomb[@"owner"]];
        Check([owners containsObject:@0] && [owners containsObject:@1], @"simultaneous bombs from both keyboard layouts");
    });
    Add(^{ Tap(35); Capture(); }); // P.
    Add(^{
        if (!Check([State()[@"paused"] boolValue], @"pause key pauses battle")) return;
        initialPositions = State()[@"simulation"];
        delay = 2;
    });
    Add(^{ Capture(); });
    Add(^{ Check([State()[@"simulation"] isEqual:initialPositions], @"pause freezes simulation state"); });
    Add(^{ Tap(122); Capture(); }); // F1.
    Add(^{ Check([State()[@"screen"] isEqual:@"Help"], @"help opens during battle"); });
    Add(^{ Tap(53); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Battle"] && [State()[@"paused"] boolValue], @"help preserves paused battle"); });
    Add(^{ Tap(35); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Battle"] && ![State()[@"paused"] boolValue], @"pause key resumes battle"); });
    Add(^{ Finish(YES, nil); });
}

__attribute__((constructor)) static void InstallDriver(void) {
    const char *report = getenv("ATOMIC_BOMBERMAN_MAC_UI_REPORT");
    if (!report) return;
    reportPath = [NSString stringWithUTF8String:report];
    launcherMode = getenv("ATOMIC_BOMBERMAN_MAC_LAUNCHER_UI") != NULL;
    if (!launcherMode) {
        probePath = [NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_STATE_PROBE")];
        settingsPath = [NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_MAC_UI_SETTINGS")];
    }
    checks = [NSMutableArray array];
    dispatch_async(dispatch_get_main_queue(), ^{
        timer = [NSTimer scheduledTimerWithTimeInterval:.25 repeats:YES block:^(NSTimer *unused) {
            if (++ticks > 240) { Finish(NO, @"native UI test timed out"); return; }
            if (!window) {
                for (NSWindow *candidate in NSApp.windows) {
                    if ([candidate.title hasPrefix:@"Atomic Bomberman"]) { window = candidate; break; }
                }
                if (!window) return;
            }
            if (launcherMode) {
                if (indexInSteps == 0) {
                    NSButton *choose = FindButton(window.contentView, @"Choose Game Data…");
                    if (!Check(choose && window.visible, @"first-launch import window appears")) return;
                    [choose performClick:nil];
                    ++indexInSteps;
                } else if (indexInSteps == 1) {
                    NSOpenPanel *panel = (NSOpenPanel *)window.sheets.firstObject;
                    if (!panel) return;
                    if (!Check([panel isKindOfClass:NSOpenPanel.class] && panel.canChooseFiles && panel.canChooseDirectories,
                        @"native source chooser accepts images, archives, and game directories")) return;
                    [panel cancel:nil];
                    ++indexInSteps;
                } else if (!window.sheets.count) {
                    Check(FindButton(window.contentView, @"Choose Game Data…").enabled, @"source chooser cancellation returns to import window");
                    Finish(YES, nil);
                }
                return;
            }
            if (!steps) {
                if (![State()[@"screen"] isEqual:@"Main"]) { Capture(); return; }
                BuildSteps();
            }
            if (delay) { --delay; return; }
            if (!finished && indexInSteps < steps.count) ((void (^)(void))steps[indexInSteps++])();
        }];
    });
}
