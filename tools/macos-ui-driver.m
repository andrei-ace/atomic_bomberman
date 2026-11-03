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
static BOOL reloadMode;
static NSString *splashMode;
static NSString *graphicsMode;
static NSMutableArray *splashTimings;
static NSTimeInterval splashObservedAt, skipRequestedAt;
static NSInteger splashObservedStage = -1;

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
        @"fullscreen_checks_included": @(getenv("ATOMIC_BOMBERMAN_MAC_UI_SKIP_FULLSCREEN") == NULL),
        @"splash_timings": splashTimings ?: @[],
        @"graphics_case": graphicsMode ?: [NSNull null],
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
    NSString *characters=code==49?@" ":@"";
    NSEvent *event = [NSEvent keyEventWithType:down ? NSEventTypeKeyDown : NSEventTypeKeyUp
        location:NSZeroPoint modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime
        windowNumber:window.windowNumber context:nil characters:characters charactersIgnoringModifiers:characters
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
    unsetenv("ATOMIC_BOMBERMAN_SCREENSHOT_PROBE");
    [[NSFileManager defaultManager] removeItemAtPath:probePath error:NULL];
    // Allow a native splash's brief clearing frame to present before capture.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,50000000),dispatch_get_main_queue(),^{Tap(111);});
}
static void CaptureNamed(NSString *name) {
    Capture();
    NSString *path = [[reportPath stringByDeletingLastPathComponent] stringByAppendingPathComponent:[name stringByAppendingString:@".png"]];
    setenv("ATOMIC_BOMBERMAN_SCREENSHOT_PROBE", path.UTF8String, 1);
}
static void Type(NSString *value) {
    for (NSUInteger i=0; i<value.length; ++i) {
        NSString *character = [value substringWithRange:NSMakeRange(i,1)];
        NSEvent *event = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:0
            timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
            characters:character charactersIgnoringModifiers:character isARepeat:NO keyCode:0];
        [window.contentView keyDown:event];
        Key(0,NO);
    }
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
static void BuildGraphicsSteps(void) {
    steps=[NSMutableArray array];checks=[NSMutableArray array];
    Add(^{[NSApp activateIgnoringOtherApps:YES];[window makeKeyAndOrderFront:nil];CaptureNamed(@"main-cursor");});
    Add(^{Check([State()[@"main_cursor"][@"sequence"] isEqual:@"bomb trigger green"] && State()[@"main_cursor"][@"variant"]==NSNull.null,@"original striped trigger-bomb menu cursor without recoloring");});
    if([graphicsMode isEqual:@"enabled"]) {
        Add(^{
            NSDictionary *graphics=State()[@"graphics"];
            if([graphics[@"job"][@"preparing"] boolValue]){Capture();--indexInSteps;return;}
            if(!Check([graphics[@"hd_active"] boolValue] && [graphics[@"texture_bytes"] unsignedLongLongValue]>0 && [graphics[@"target_width"] intValue]>640,@"validated HD cache loads at physical viewport resolution"))return;
            CaptureNamed(@"hd-main");
        });
    }else{
        Add(^{
            if([State()[@"graphics"][@"job"][@"preparing"] boolValue]){Capture();--indexInSteps;return;}
            Check(![State()[@"graphics"][@"hd_active"] boolValue],@"Classic remains playable when HD is unavailable or unselected");
        });
    }
    Add(^{Tap(125);});Add(^{Tap(125);});Add(^{Tap(125);});
    Add(^{Tap(36);Capture();});
    Add(^{if(!Check([State()[@"screen"] isEqual:@"Settings"],@"Options opens normally"))return;Tap(126);});
    Add(^{Tap(126);});
    Add(^{Tap(36);CaptureNamed(@"graphics-options");});
    Add(^{Check([State()[@"screen"] isEqual:@"Graphics"],@"Graphics submenu opens from Options");});
    if([graphicsMode isEqual:@"cancel"]) {
        Add(^{Tap(36);Capture();});
        Add(^{
            NSDictionary *job=State()[@"graphics"][@"job"];
            if(![job[@"preparing"] boolValue]){Finish(NO,[NSString stringWithFormat:@"HD preparation did not start: %@",job]);return;}
            if(![job[@"message"] hasPrefix:@"Enhancing "]){Capture();--indexInSteps;return;}
            Check(YES,@"in-game preparation downloads and verifies helper/model and starts enhancement");
            Tap(53);CaptureNamed(@"hd-cancelling");
        });
        Add(^{
            if([State()[@"graphics"][@"job"][@"preparing"] boolValue]){Capture();--indexInSteps;return;}
            Check(![State()[@"graphics"][@"hd_active"] boolValue] && [ReadJSON(settingsPath)[@"graphics"] isEqual:@"Classic"],@"cancellation keeps Classic active and preserves the saved preference");
            CaptureNamed(@"hd-cancelled");
        });
    }else if([graphicsMode isEqual:@"fallback"]) {
        Add(^{
            NSDictionary *graphics=State()[@"graphics"];
            Check([graphics[@"requested"] isEqual:@"Hd"] && ![graphics[@"hd_active"] boolValue] && graphics[@"job"][@"error"]!=NSNull.null,@"missing HD cache falls back once and reports the reason");
            Check([ReadJSON(settingsPath)[@"nickname"] isEqual:@"HD Fallback Tester"],@"HD failure preserves unrelated profile settings");
            CaptureNamed(@"hd-fallback");Tap(36);Capture();
        });
        Add(^{Check([ReadJSON(settingsPath)[@"graphics"] isEqual:@"Classic"],@"Classic can be selected and saved after HD fallback");});
    }else{
        Add(^{CaptureNamed(@"hd-options");Tap(53);});
        Add(^{Tap(53);Capture();});
        Add(^{Tap(126);});Add(^{Tap(126);});Add(^{Tap(126);});
        Add(^{Tap(36);Capture();});
        Add(^{if(!Check([State()[@"screen"] isEqual:@"Setup"],@"HD player setup opens"))return;Tap(36);Capture();});
        Add(^{if(!Check([State()[@"screen"] isEqual:@"Arena"],@"HD arena preview opens"))return;Tap(36);delay=16;Capture();});
        Add(^{CaptureNamed(@"hd-battle");});
        Add(^{Check([State()[@"screen"] isEqual:@"Battle"] && [State()[@"graphics"][@"hd_active"] boolValue],@"ten-player battle renders with HD active");[window setContentSize:NSMakeSize(1100,600)];CaptureNamed(@"hd-resized");});
        Add(^{Check(fabs([State()[@"graphics"][@"target_height"] doubleValue]-600*window.backingScaleFactor)<2,@"HD render target follows physical window resizing");});
    }
    Add(^{Finish(YES,nil);});
}
static void BuildSteps(void) {
    steps = [NSMutableArray array]; checks = [NSMutableArray array];
    Add(^{ [NSApp activateIgnoringOtherApps:YES]; [window makeKeyAndOrderFront:nil]; [window setContentSize:NSMakeSize(1280, 720)]; CaptureNamed(@"intro-interplay"); });
    Add(^{ Check(fabs(window.contentView.bounds.size.width - 1280) < 2, @"resizable native window"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Intro"] && [State()[@"frontend"][@"intro"] isEqual:@0] && [State()[@"interface_audio"][@"requested_music"] isEqual:@"TITLE"], @"Interplay logo requests original TITLE music"); Capture(); });
    Add(^{
        if ([State()[@"frontend"][@"intro"] isEqual:@0]) { Capture(); --indexInSteps; return; }
        NSDictionary *groups=State()[@"interface_audio"][@"requested_groups"];
        Check([State()[@"frontend"][@"intro"] isEqual:@1] && [groups[@"20"] isEqual:@1] && [groups[@"10"] isEqual:@1], @"splash timeout advances with original letter and confirm effects");
        CaptureNamed(@"intro-hudson");
    });
    Add(^{ Check([State()[@"frontend"][@"intro"] isEqual:@1] && [State()[@"interface_audio"][@"requested_music"] isEqual:@"TITLE"], @"Hudson keeps original TITLE music"); Tap(36); CaptureNamed(@"intro-title"); });
    Add(^{ Check([State()[@"frontend"][@"intro"] isEqual:@2] && [State()[@"interface_audio"][@"requested_groups"][@"2800"] isEqual:@1], @"title requests one original announcer clip"); Tap(36); CaptureNamed(@"main"); });
    Add(^{ Check([State()[@"interface_audio"][@"requested_music"] isEqual:@"MENU"] && [State()[@"main_actions"][1] isEqual:@"LAN"] && [State()[@"main_actions"][2] isEqual:@"Internet"], @"main menu switches to MENU and exposes LAN and Internet"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Main"], @"title proceeds to main menu"); Click(460,239); CaptureNamed(@"options"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Settings"], @"original Options entry"); Click(200,84); });
    Add(^{ Tap(117); Type(@"Menu Tester"); });
    Add(^{ Tap(36); Capture(); });
    Add(^{ Check([ReadJSON(settingsPath)[@"nickname"] isEqual:@"Menu Tester"], @"global nickname persists"); Click(200,370); CaptureNamed(@"controls"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Controls"], @"letterboxed mouse coordinates open Controls"); });
    Add(^{ Click(300, 236); }); // Keyboard 1 action binding.
    Add(^{ Tap(6); }); // Z; do not probe while a binding capture is pending.
    Add(^{ Check([ReadJSON(settingsPath)[@"keyboard"][0][@"keys"][5] isEqual:@"Z"], @"keyboard remapping persists"); });
    Add(^{ Tap(53); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Settings"], @"controls return to Options"); Tap(53); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Main"], @"Escape returns to menu"); Click(470,277); CaptureNamed(@"credits"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Help"] && [State()[@"frontend"][@"document"] isEqual:@"CREDITS.BM"], @"illustrated original credits viewer"); Tap(53); });
    Add(^{ Click(470,315); CaptureNamed(@"manual-picker"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Manual"], @"original help-file picker"); Tap(36); CaptureNamed(@"manual-document"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Help"], @"selected original help document opens"); Tap(53); });
    Add(^{ Tap(53); });
    Add(^{ Click(460,163); CaptureNamed(@"lan-browser"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Lan"] && [State()[@"lan"][@"actions"] count]==6 && [State()[@"interface_audio"][@"requested_music"] isEqual:@"NETWORK"], @"LAN opens directly with all browser actions and NETWORK music"); Tap(122); CaptureNamed(@"network-help"); });
    Add(^{ Check([State()[@"frontend"][@"document_title"] isEqual:@"LAN and Internet play"], @"LAN F1 uses current LAN and Internet instructions"); Tap(53); });
    Add(^{ Click(180,180); CaptureNamed(@"network-nickname"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Nickname"], @"LAN nickname edit is explicit"); Tap(117); Type(@"LAN Tester"); });
    Add(^{ Tap(36); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Lan"] && [ReadJSON(settingsPath)[@"nickname"] isEqual:@"LAN Tester"], @"LAN nickname saves and returns to the browser"); Click(180,156); CaptureNamed(@"network-controls"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"NetworkDevices"], @"browser local controls open without creating a room"); Click(400,188); });
    Add(^{ Click(100,420); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Lan"] && State()[@"network"]==[NSNull null] && [ReadJSON(settingsPath)[@"network_devices"] isEqual:@[@"Keyboard1"]] && [ReadJSON(settingsPath)[@"participants"][1][@"device"] isEqual:@"Keyboard2"], @"network controls Done saves separately from local players"); Click(180,132); CaptureNamed(@"join-ip"); });
    Add(^{ Check([State()[@"lan"][@"manual"] boolValue] && [State()[@"lan"][@"address"] isEqual:ReadJSON(settingsPath)[@"lan_address"]], @"Join by IP restores the last saved address"); Tap(53); });
    Add(^{ Click(180,228); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Main"], @"LAN Main menu returns directly"); Click(460,201); CaptureNamed(@"internet-browser"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Internet"] && [State()[@"browser"][@"actions"] count]==7 && ![State()[@"browser"][@"saved"] boolValue], @"Internet opens directly with all options before server setup"); Click(180,132); CaptureNamed(@"server-link"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Online"], @"Add server link opens the original-style dialog"); Type(@"stray typing"); });
    Add(^{ Capture(); });
    Add(^{ Check(![State()[@"browser"][@"text_present"] boolValue], @"stray typing does not corrupt a private server link"); Tap(53); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Internet"] && ![State()[@"browser"][@"saved"] boolValue], @"server-link Cancel returns without saving a candidate"); Click(180,204); });
    Add(^{ Tap(117); Type(@"Cancelled Name"); });
    Add(^{ Tap(53); Capture(); });
    Add(^{ Check([ReadJSON(settingsPath)[@"nickname"] isEqual:@"LAN Tester"], @"nickname Cancel preserves the saved global name"); Click(180,252); Capture(); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Main"], @"Internet Main menu returns directly"); });
    if (!getenv("ATOMIC_BOMBERMAN_MAC_UI_SKIP_FULLSCREEN")) {
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
    }
    Add(^{ Click(460,125); CaptureNamed(@"players"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Setup"] && [State()[@"interface_audio"][@"requested_music"] isEqual:@"WIN"], @"original player setup requests WIN music"); Tap(123); });
    Add(^{ Check(![ReadJSON(settingsPath)[@"participants"][0][@"enabled"] boolValue], @"Left turns the local player OFF"); Tap(124); });
    Add(^{ Check([ReadJSON(settingsPath)[@"participants"][0][@"enabled"] boolValue] && [ReadJSON(settingsPath)[@"participants"][0][@"device"] isEqual:@"Bot"], @"Right from OFF selects AI first"); Tap(124); });
    Add(^{ Check([ReadJSON(settingsPath)[@"participants"][0][@"device"] isEqual:@"Keyboard1"], @"Right after AI selects KEY 0"); Tap(36); CaptureNamed(@"arena"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Arena"], @"separate original arena preview"); Tap(124); CaptureNamed(@"arena-next"); });
    Add(^{ Check([ReadJSON(settingsPath)[@"arena"] isEqual:@1], @"arena choice persists without starting battle"); Tap(36); delay = 18; });
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
    Add(^{ Tap(35); CaptureNamed(@"pause"); }); // P.
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
    Add(^{
        NSMutableDictionary *config = [State()[@"simulation"][@"config"] mutableCopy];
        config[@"wins_to_match"] = @1;
        config[@"gold_bomberman"] = @YES;
        NSString *path = [NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_TEST_CONFIG")];
        [[NSJSONSerialization dataWithJSONObject:config options:0 error:NULL] writeToFile:path atomically:YES];
        Tap(101); delay=18; // F9 loads the opt-in debug fixture through production Simulation::new.
    });
    Add(^{ Key(49,YES); }); // Hold across a production frame; battle samples held input.
    Add(^{ Key(49,NO); Capture(); });
    Add(^{ Check([State()[@"simulation"][@"bombs"] count]==1, @"Gold fixture places one actual bomb"); delay=20; });
    Add(^{ CaptureNamed(@"victory"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Victory"], @"native victory artwork before final results"); Tap(36); CaptureNamed(@"results"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Results"], @"original match scoreboard"); Tap(36); CaptureNamed(@"gold-wheel"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Roulette"], @"Gold rematch opens original bonus screen"); Tap(36); delay=35; });
    Add(^{ CaptureNamed(@"gold-reward"); });
    Add(^{
        if (![State()[@"simulation"][@"gold_wheel"][@"phase"] isEqual:@"Settled"]) { --indexInSteps; CaptureNamed(@"gold-reward"); return; }
        Check(State()[@"simulation"][@"gold_reward"] != [NSNull null], @"Gold wheel settles with next-match reward");
        Tap(36); Capture();
    });
    Add(^{ Check([State()[@"screen"] isEqual:@"Battle"], @"Gold reward proceeds to rematch"); });
    Add(^{ Finish(YES, nil); });
}

static void BuildReloadSteps(void) {
    steps=[NSMutableArray array];checks=[NSMutableArray array];
    Add(^{ CaptureNamed(@"reload-main"); });
    Add(^{ Check([State()[@"frontend"][@"nickname"] isEqual:@"LAN Tester"], @"nickname survives a real process restart"); Click(460,163); CaptureNamed(@"reload-lan"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Lan"] && [State()[@"lan"][@"devices"] isEqual:@[@"Keyboard1"]], @"network controls survive a real process restart"); Click(180,132); CaptureNamed(@"reload-ip"); });
    Add(^{ Check([State()[@"lan"][@"address"] isEqual:ReadJSON(settingsPath)[@"lan_address"]], @"last LAN address survives a real process restart"); Tap(53); });
    Add(^{ Click(180,228); });
    Add(^{ Click(460,201); CaptureNamed(@"reload-internet"); });
    Add(^{ Check([State()[@"screen"] isEqual:@"Internet"] && [State()[@"frontend"][@"nickname"] isEqual:@"LAN Tester"], @"Internet uses the saved global nickname"); Finish(YES,nil); });
}

static void RecordSkip(NSString *key, NSInteger expected) {
    NSDictionary *state=State();
    BOOL advanced=expected==3?[state[@"screen"] isEqual:@"Main"]:[state[@"frontend"][@"intro"] integerValue]==expected;
    NSDate *written=[[NSFileManager defaultManager] attributesOfItemAtPath:probePath error:NULL][NSFileModificationDate];
    double elapsed=1000*(written.timeIntervalSince1970-skipRequestedAt);
    [splashTimings addObject:@{@"key":key,@"request_to_snapshot_ms":@(elapsed)}];
    Check(advanced && elapsed>=0 && elapsed<250,[NSString stringWithFormat:@"%@ advances splash within 250 ms",key]);
}

static void BuildSkipSplashSteps(void) {
    steps=[NSMutableArray array];checks=[NSMutableArray array];
    Add(^{ skipRequestedAt=NSDate.date.timeIntervalSince1970;Tap(36);CaptureNamed(@"skip-hudson"); });
    Add(^{ RecordSkip(@"Enter",1);skipRequestedAt=NSDate.date.timeIntervalSince1970;Tap(49);CaptureNamed(@"skip-title"); });
    Add(^{ RecordSkip(@"Space",2);skipRequestedAt=NSDate.date.timeIntervalSince1970;Tap(53);CaptureNamed(@"skip-main"); });
    Add(^{ RecordSkip(@"Escape",3);if(!finished)Finish(YES,nil); });
}

static void ObserveAutomaticSplash(void) {
    NSDictionary *state=State();
    if (!state) { Capture();return; }
    NSInteger stage=[state[@"screen"] isEqual:@"Main"]?3:[state[@"frontend"][@"intro"] integerValue];
    NSTimeInterval now=NSProcessInfo.processInfo.systemUptime;
    if (splashObservedStage<0) {
        if (![state[@"screen"] isEqual:@"Intro"] || stage!=0) { Finish(NO,@"automatic splash begins at Interplay");return; }
        splashObservedStage=0;splashObservedAt=now;
    } else if (stage!=splashObservedStage) {
        double seconds=now-splashObservedAt;
        [splashTimings addObject:@{@"page":@[@"Interplay",@"Hudson",@"Title"][splashObservedStage],@"observed_seconds":@(seconds)}];
        if(!Check(stage==splashObservedStage+1 && seconds>=6 && seconds<=9,@"splash page advances automatically at original timing"))return;
        splashObservedStage=stage;splashObservedAt=now;
        if(stage==3) {
            NSDictionary *groups=state[@"interface_audio"][@"requested_groups"];
            if(!Check([groups[@"20"] isEqual:@3] && [groups[@"10"] isEqual:@3] && [groups[@"2800"] isEqual:@1],@"all automatic splash transitions request the original sound effects"))return;
            if(!Check([state[@"interface_audio"][@"requested_music"] isEqual:@"MENU"],@"automatic opening reaches the main menu and MENU music"))return;
            Finish(YES,nil);return;
        }
    }
    Capture();
}

__attribute__((constructor)) static void InstallDriver(void) {
    const char *report = getenv("ATOMIC_BOMBERMAN_MAC_UI_REPORT");
    if (!report) return;
    reportPath = [NSString stringWithUTF8String:report];
    launcherMode = getenv("ATOMIC_BOMBERMAN_MAC_LAUNCHER_UI") != NULL;
    reloadMode = getenv("ATOMIC_BOMBERMAN_MAC_UI_RELOAD") != NULL;
    const char *splash=getenv("ATOMIC_BOMBERMAN_MAC_UI_SPLASH");
    if(splash) { splashMode=[NSString stringWithUTF8String:splash];splashTimings=[NSMutableArray array]; }
    const char *graphics=getenv("ATOMIC_BOMBERMAN_MAC_UI_GRAPHICS");
    if(graphics)graphicsMode=[NSString stringWithUTF8String:graphics];
    if (!launcherMode) {
        probePath = [NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_STATE_PROBE")];
        settingsPath = [NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_MAC_UI_SETTINGS")];
    }
    checks = [NSMutableArray array];
    dispatch_async(dispatch_get_main_queue(), ^{
        timer = [NSTimer scheduledTimerWithTimeInterval:.25 repeats:YES block:^(NSTimer *unused) {
            if (++ticks > 720) { Finish(NO, @"native UI test timed out"); return; }
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
            if([splashMode isEqual:@"auto"]) {
                if(splashObservedStage<0 && ![State()[@"screen"] isEqual:@"Intro"]) { Capture();return; }
                ObserveAutomaticSplash();return;
            }
            if (!steps) {
                if (![State()[@"screen"] isEqual:(reloadMode||graphicsMode)?@"Main":@"Intro"]) { Capture(); return; }
                if(graphicsMode) BuildGraphicsSteps();
                else if([splashMode isEqual:@"skip"]) BuildSkipSplashSteps();
                else if (reloadMode) BuildReloadSteps(); else BuildSteps();
            }
            if (delay) { --delay; return; }
            if (!finished && indexInSteps < steps.count) ((void (^)(void))steps[indexInSteps++])();
        }];
    });
}
