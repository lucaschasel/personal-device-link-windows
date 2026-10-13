#import <Foundation/Foundation.h>
#import <IOBluetooth/IOBluetooth.h>
#include <unistd.h>
#include <signal.h>
#include <errno.h>
#include <string.h>
#include <stdlib.h>

// stdout: JSON status lines until connected, then exclusively RFCOMM bytes.
static void statusLine(NSDictionary *event) {
    NSData *json = [NSJSONSerialization dataWithJSONObject:event options:0 error:nil];
    if (!json || fwrite(json.bytes, 1, json.length, stdout) != json.length || fputc('\n', stdout) == EOF || fflush(stdout) != 0) exit(1);
}
static NSString *normalizedAddress(NSString *raw) {
    if (!raw) return nil;
    for (NSString *separator in @[@":", @"-", @" "]) raw = [raw stringByReplacingOccurrencesOfString:separator withString:@""];
    if (raw.length != 12 || [raw rangeOfCharacterFromSet:[[NSCharacterSet characterSetWithCharactersInString:@"0123456789abcdefABCDEF"] invertedSet]].location != NSNotFound) return nil;
    return raw.uppercaseString;
}
@interface PDLRadio : NSObject <IOBluetoothRFCOMMChannelDelegate>
@property(nonatomic, strong) IOBluetoothRFCOMMChannel *channel;
@property(nonatomic, strong) IOBluetoothSDPServiceRecord *service;
@property(nonatomic, strong) IOBluetoothUserNotification *notification;
@property(nonatomic) BOOL started;
@property(nonatomic) BOOL activating;
@property(nonatomic, strong) NSMutableData *pendingReceived;
- (void)finish:(int)code;
- (BOOL)listen;
@end
@implementation PDLRadio
- (void)finish:(int)code {
    [self.notification unregister]; self.notification = nil;
    [self.service removeServiceRecord]; self.service = nil;
    [self.channel setDelegate:nil];
    [self.channel closeChannel]; self.channel = nil;
    exit(code);
}
- (void)activate:(IOBluetoothRFCOMMChannel *)channel {
    if (self.started || self.activating) return;
    self.activating = YES;
    self.channel = channel;
    IOBluetoothDevice *device = [channel getDevice];
    if (![device isPaired]) {
        fprintf(stderr,"请先在系统设置配对。\n"); [self finish:1]; return;
    }
    // Starts the bridge handshake timeout even if authentication stalls.
    statusLine(@{@"event": @"connected"});
    if ([device requestAuthentication] != kIOReturnSuccess) {
        fprintf(stderr,"蓝牙认证失败，请先在系统设置完成配对。\n");
        [self finish:1]; return;
    }
    self.channel = channel;
    self.started = YES;
    if (self.pendingReceived.length) {
        NSData *pending = self.pendingReceived;
        self.pendingReceived = nil;
        [self rfcommChannelData:channel data:(void *)pending.bytes length:pending.length];
    }
    // Emit before installing the incoming delegate: buffered radio data must not
    // appear before the status prefix, even if delivered immediately by setDelegate.
    if ([channel setDelegate:self] != kIOReturnSuccess) { [self finish:1]; return; }
    [NSThread detachNewThreadWithBlock:^{
        @autoreleasepool {
            unsigned char buffer[512];
            for (;;) {
                ssize_t n = read(STDIN_FILENO, buffer, sizeof(buffer));
                if (n < 0 && errno == EINTR) continue;
                if (n <= 0) break;
                size_t offset = 0;
                while (offset < (size_t)n) {
                    size_t size = MIN((size_t)n - offset, (size_t)[channel getMTU]);
                    if (size == 0) exit(1);
                    IOReturn result = [channel writeSync:buffer + offset length:(UInt16)size];
                    if (result != kIOReturnSuccess) { fprintf(stderr,"RFCOMM write failed: 0x%08x\n",result); exit(1); }
                    offset += size;
                }
            }
            dispatch_async(dispatch_get_main_queue(), ^{ [self finish:0]; });
        }
    }];
}
- (void)rfcommChannelOpenComplete:(IOBluetoothRFCOMMChannel *)channel status:(IOReturn)status {
    if (status != kIOReturnSuccess) { fprintf(stderr,"RFCOMM open failed: 0x%08x\n",status); [self finish:1]; return; }
    [self activate:channel];
}
- (void)incoming:(IOBluetoothUserNotification *)notification channel:(IOBluetoothRFCOMMChannel *)channel {
    if (notification != self.notification) return;
    if (self.channel || ![[channel getDevice] isPaired]) {
        // A second or unpaired peer must not replace the selected session.
        [channel closeChannel]; return;
    }
    self.channel = channel;
    [self activate:channel];
}
- (BOOL)listen {
    IOBluetoothHostController *host = [IOBluetoothHostController defaultController];
    NSString *address = normalizedAddress([host addressAsString]);
    if (!address || [host powerState] != kBluetoothHCIPowerStateON) {
        fprintf(stderr,"没有可用蓝牙适配器，请先开启蓝牙并允许应用使用蓝牙。\n"); return NO;
    }
    // Custom transient service; the OS allocates a free RFCOMM channel.
    const unsigned char uuid[] = {0xa6,0x42,0x85,0x4d,0x61,0x77,0x4e,0x0e,0xb3,0x75,0x36,0x96,0x55,0x50,0x44,0x4c};
    NSDictionary *record = @{
        @"0001 - ServiceClassIDList": @[[IOBluetoothSDPUUID uuidWithBytes:uuid length:sizeof(uuid)]],
        @"0004 - ProtocolDescriptorList": @[
            @[[IOBluetoothSDPUUID uuid16:0x0100]],
            @[[IOBluetoothSDPUUID uuid16:0x0003], @{@"DataElementType": @1, @"DataElementSize": @1, @"DataElementValue": @1}]
        ],
        @"0005 - BrowseGroupList": @[[IOBluetoothSDPUUID uuid16:0x1002]],
        @"0100 - ServiceName": @"Personal Device Link",
        @"LocalAttributes": @{@"Persistent": @NO}
    };
    self.service = [IOBluetoothSDPServiceRecord publishedServiceRecordWithDictionary:record];
    BluetoothRFCOMMChannelID port = 0;
    if (!self.service || [self.service getRFCOMMChannelID:&port] != kIOReturnSuccess || port < 1 || port > 30) {
        fprintf(stderr,"无法发布蓝牙等待服务，请检查蓝牙权限与系统状态。\n"); return NO;
    }
    self.notification = [IOBluetoothRFCOMMChannel registerForChannelOpenNotifications:self selector:@selector(incoming:channel:) withChannelID:port direction:kIOBluetoothUserNotificationChannelDirectionIncoming];
    if (!self.notification) { fprintf(stderr,"无法注册蓝牙连接回调。\n"); return NO; }
    statusLine(@{@"event": @"listening", @"address": address, @"channel": @(port)});
    return YES;
}
- (void)rfcommChannelData:(IOBluetoothRFCOMMChannel *)channel data:(void *)dataPointer length:(size_t)dataLength {
    if (channel != self.channel) { [self finish:1]; return; }
    if (!self.started) {
        // Outgoing channels already have a delegate while authentication completes.
        if (dataLength > 65536 || self.pendingReceived.length > 65536 - dataLength) { [self finish:1]; return; }
        if (!self.pendingReceived) self.pendingReceived = [NSMutableData data];
        [self.pendingReceived appendBytes:dataPointer length:dataLength];
        return;
    }
    size_t offset = 0;
    while (offset < dataLength) {
        ssize_t n = write(STDOUT_FILENO, (unsigned char *)dataPointer + offset, dataLength - offset);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) { [self finish:1]; return; }
        offset += (size_t)n;
    }
}
- (void)rfcommChannelClosed:(IOBluetoothRFCOMMChannel *)channel {
    if (channel == self.channel) { fprintf(stderr,"RFCOMM disconnected.\n"); [self finish:0]; }
}
@end
int main(int argc, const char **argv) {
    @autoreleasepool {
        BOOL listening = argc == 2 && strcmp(argv[1], "--listen") == 0;
        BOOL connecting = argc == 4 && strcmp(argv[1], "--connect") == 0;
        if (!listening && !connecting) { fprintf(stderr,"Usage: pdl-rfcomm-mac --listen | --connect ADDRESS CHANNEL\n"); return 2; }
        signal(SIGPIPE, SIG_IGN);
        const pid_t parent = getppid();
        if (parent == 1) return 1;
        // Independent of the main runloop: cancelling the managed bridge must
        // also terminate a helper blocked in a system authentication call.
        [NSThread detachNewThreadWithBlock:^{
            while (getppid() == parent) usleep(500000);
            // The published service is transient and is removed by the OS.
            _exit(0);
        }];
        PDLRadio *delegate = [PDLRadio new];
        if (listening) {
            if (![delegate listen]) { [delegate finish:1]; return 1; }
        } else {
            char *end = NULL; long port = strtol(argv[3], &end, 10);
            NSString *raw = normalizedAddress([NSString stringWithUTF8String:argv[2]]);
            if (*end || port < 1 || port > 30 || !raw) return 2;
            NSMutableArray *parts = [NSMutableArray array];
            for (NSUInteger i = 0; i < 12; i += 2) [parts addObject:[raw substringWithRange:NSMakeRange(i, 2)]];
            IOBluetoothDevice *device = [IOBluetoothDevice deviceWithAddressString:[parts componentsJoinedByString:@":"]];
            if (!device || ![device isPaired]) { fprintf(stderr,"请先在两台电脑的系统蓝牙设置中完成配对。\n"); return 1; }
            IOBluetoothRFCOMMChannel *channel = nil;
            IOReturn result = [device openRFCOMMChannelAsync:&channel withChannelID:(BluetoothRFCOMMChannelID)port delegate:delegate];
            if (result != kIOReturnSuccess) { fprintf(stderr,"RFCOMM start failed: 0x%08x\n",result); [delegate finish:1]; return 1; }
            delegate.channel = channel;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 20 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                if (!delegate.started) { fprintf(stderr,"蓝牙连接超时，请核对对方的地址、频道及等待状态。\n"); [delegate finish:1]; }
            });
        }
        // Preserve the delegate and runloop; orphaned listeners withdraw their service.
        while (true) { @autoreleasepool {
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.5]];
            if (getppid() != parent) [delegate finish:0];
        } }
    }
}
