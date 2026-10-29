// Exercises the actual game UI. Events stay in this process; no Accessibility
// access or global keyboard injection is used. Clipboard contents are restored.
#import <Cocoa/Cocoa.h>
static NSTimer *timer;
static NSWindow *window;
static NSUInteger ticks, stage, waitingUntil;
static NSString *reportPath, *profilePath, *expectedLink, *copiedLink, *originalText;
static NSArray<NSPasteboardItem *> *originalItems;
static NSMutableArray<NSString *> *checks;
static NSEventModifierFlags modifiers;
static BOOL finished;
static void DriverExited(void) {
    if(!finished) fprintf(stderr,"Clipboard UI driver exited early at stage %lu, tick %lu\n",(unsigned long)stage,(unsigned long)ticks);
}

static void RestoreClipboard(void) {
    NSPasteboard *board=NSPasteboard.generalPasteboard;
    [board clearContents];
    if(originalItems.count) [board writeObjects:originalItems];
}
static void Finish(BOOL passed, NSString *error) {
    if(finished) return;
    finished=YES; [timer invalidate];
    RestoreClipboard();
    NSString *restored=[NSPasteboard.generalPasteboard stringForType:NSPasteboardTypeString];
    BOOL restoredOK=(originalText==nil && restored==nil) || [originalText isEqualToString:restored];
    NSDictionary *report=@{@"passed":(passed && restoredOK) ? @YES : @NO,@"error":error ?: NSNull.null,
        @"platform":@"macOS AppKit, actual packaged game UI",
        @"checks":checks ?: @[],@"original_clipboard_restored":@(restoredOK),
        @"limitations":@[@"Synthetic events stay in the game view; physical keyboard hardware is separate."]};
    NSError *writeError=nil;
    NSData *json=[NSJSONSerialization dataWithJSONObject:report options:NSJSONWritingPrettyPrinted error:&writeError];
    if(!json || ![json writeToFile:reportPath options:NSDataWritingAtomic error:&writeError])
        fprintf(stderr,"Clipboard UI report could not be written: %s\n",writeError.localizedDescription.UTF8String);
    [NSApp terminate:nil];
}
static BOOL Check(BOOL value, NSString *label) {
    if(!value) { Finish(NO,label); return NO; }
    [checks addObject:label]; return YES;
}
static void SetClipboard(NSString *value) {
    [NSPasteboard.generalPasteboard clearContents];
    [NSPasteboard.generalPasteboard setString:value forType:NSPasteboardTypeString];
}
static void Key(unsigned short code, BOOL down) {
    NSString *characters=code==9 ? @"v" : code==7 ? @"x" : @"";
    NSEvent *event=[NSEvent keyEventWithType:down ? NSEventTypeKeyDown : NSEventTypeKeyUp
        location:NSZeroPoint modifierFlags:modifiers timestamp:NSProcessInfo.processInfo.systemUptime
        windowNumber:window.windowNumber context:nil characters:characters
        charactersIgnoringModifiers:characters isARepeat:NO keyCode:code];
    if(down) [window.contentView keyDown:event]; else [window.contentView keyUp:event];
}
static void Command(BOOL down) {
    modifiers=down ? 0x100008 : 0;
    NSEvent *event=[NSEvent keyEventWithType:NSEventTypeFlagsChanged location:NSZeroPoint
        modifierFlags:modifiers timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber
        context:nil characters:@"" charactersIgnoringModifiers:@"" isARepeat:NO keyCode:55];
    [window.contentView flagsChanged:event];
}
static void Click(double x,double y) {
    NSSize size=window.contentView.bounds.size;
    double scale=MIN(size.width/640.,size.height/480.);
    NSPoint point=NSMakePoint((size.width-640*scale)/2+x*scale,size.height-((size.height-480*scale)/2+y*scale));
    for(NSNumber *kind in @[@(NSEventTypeMouseMoved),@(NSEventTypeLeftMouseDown),@(NSEventTypeLeftMouseUp)]) {
        NSEvent *event=[NSEvent mouseEventWithType:kind.unsignedIntegerValue location:point modifierFlags:0
            timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil
            eventNumber:0 clickCount:1 pressure:1];
        if(kind.unsignedIntegerValue==NSEventTypeMouseMoved) [window.contentView mouseMoved:event];
        else if(kind.unsignedIntegerValue==NSEventTypeLeftMouseDown) [window.contentView mouseDown:event];
        else [window.contentView mouseUp:event];
    }
}
static BOOL Authenticated(void) {
    NSData *data=[NSData dataWithContentsOfFile:profilePath];
    if(!data) return NO;
    NSDictionary *profile=[NSJSONSerialization JSONObjectWithData:data options:0 error:NULL];
    NSURLComponents *link=[NSURLComponents componentsWithString:expectedLink];
    NSURLComponents *fields=NSURLComponents.new; fields.percentEncodedQuery=link.percentEncodedFragment;
    NSString *token=nil;
    for(NSURLQueryItem *item in fields.queryItems) if([item.name isEqual:@"token"]) token=item.value;
    return token && [profile[@"token"] isEqual:token];
}
__attribute__((constructor)) static void Install(void) {
    if(!getenv("ATOMIC_BOMBERMAN_CLIPBOARD_REPORT")) return;
    atexit(DriverExited);
    reportPath=[NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_CLIPBOARD_REPORT")];
    profilePath=[NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_CLIPBOARD_PROFILE")];
    expectedLink=[[NSString stringWithContentsOfFile:[NSString stringWithUTF8String:getenv("ATOMIC_BOMBERMAN_CLIPBOARD_LINK")] encoding:NSUTF8StringEncoding error:NULL] stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    checks=NSMutableArray.new;
    NSPasteboard *board=NSPasteboard.generalPasteboard;
    originalText=[[board stringForType:NSPasteboardTypeString] copy];
    NSMutableArray *saved=NSMutableArray.new;
    for(NSPasteboardItem *item in board.pasteboardItems) {
        NSPasteboardItem *copy=NSPasteboardItem.new;
        for(NSPasteboardType type in item.types) {
            NSData *data=[item dataForType:type];
            if(data) [copy setData:[data copy] forType:type];
        }
        [saved addObject:copy];
    }
    originalItems=saved;
    dispatch_async(dispatch_get_main_queue(), ^{
        timer=[NSTimer scheduledTimerWithTimeInterval:.25 repeats:YES block:^(NSTimer *unused) {
            ticks++;
            if(ticks>240) { Finish(NO,@"Mac clipboard UI test timed out"); return; }
            if(!window) {
                for(NSWindow *candidate in NSApp.windows)
                    if([candidate.title hasPrefix:@"Atomic Bomberman"]) { window=candidate; break; }
                if(!window) return;
                [NSApp activateIgnoringOtherApps:YES]; [window makeKeyAndOrderFront:nil];
                waitingUntil=ticks+24;
            }
            if(ticks<waitingUntil) return;
            switch(stage) {
                case 0: Click(460,170); stage++; break; // Multiplayer
                case 1: Click(200,235); stage++; break; // Internet
                case 2: SetClipboard(expectedLink); Click(200,195); stage++; break; // Paste button
                case 3: Click(200,235); waitingUntil=ticks+2; stage++; break; // Connect
                case 4:
                    if(!Authenticated()) return;
                    if(!Check(YES,@"Paste server link button reads macOS clipboard and authenticates")) return;
                    waitingUntil=ticks+2; stage++; break;
                case 5: SetClipboard(@"stale clipboard text"); Click(200,160); stage++; break; // Copy server link
                case 6:
                    copiedLink=[[board stringForType:NSPasteboardTypeString] copy];
                    if(!Check([copiedLink isEqual:expectedLink],@"Copy server link updates the macOS clipboard")) return;
                    Click(200,135); // Change server
                    [[NSFileManager defaultManager] removeItemAtPath:profilePath error:NULL];
                    stage++; break;
                case 7: Click(200,195); stage++; break; // Paste the game-copied link
                case 8: Key(51,YES); stage++; break; // Backspace
                case 9: Key(51,NO); Click(200,235); waitingUntil=ticks+3; stage++; break;
                case 10:
                    if(!Check(![[NSFileManager defaultManager] fileExistsAtPath:profilePath],@"Backspace discards the private link before Connect")) return;
                    Click(200,195); stage++; break;
                case 11: Key(117,YES); stage++; break; // Forward Delete
                case 12: Key(117,NO); Click(200,235); waitingUntil=ticks+3; stage++; break;
                case 13:
                    if(!Check(![[NSFileManager defaultManager] fileExistsAtPath:profilePath],@"Delete discards the private link before Connect")) return;
                    SetClipboard(@"not a server link"); Click(200,195); stage++; break;
                case 14: Click(200,235); waitingUntil=ticks+3; stage++; break;
                case 15:
                    if(!Check(![[NSFileManager defaultManager] fileExistsAtPath:profilePath],@"Invalid clipboard text cannot reuse the previous link")) return;
                    SetClipboard(copiedLink); Click(200,195); stage++; break;
                case 16: [board clearContents]; Click(200,195); stage++; break;
                case 17: Click(200,235); waitingUntil=ticks+3; stage++; break;
                case 18:
                    if(!Check(![[NSFileManager defaultManager] fileExistsAtPath:profilePath],@"Empty clipboard does not reuse stale invitation text")) return;
                    SetClipboard(copiedLink); Command(YES); Key(9,YES); stage++; break;
                case 19: Key(9,NO); Command(NO); stage++; break;
                case 20: Key(7,YES); stage++; break; // Stray typing must not corrupt the hidden link
                case 21: Key(7,NO); Click(200,235); stage++; break;
                case 22:
                    if(!Authenticated()) return;
                    if(!Check(YES,@"Cmd-V authenticates a game-copied link after clearing and accidental typing")) return;
                    Finish(YES,nil); break;
            }
        }];
    });
}
