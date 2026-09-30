#import <Foundation/Foundation.h>
#import <CoreBluetooth/CoreBluetooth.h>
#import <dispatch/dispatch.h>

// Scans for caBLE service-data adverts: a phone that scanned the passkey QR
// code broadcasts one to prove it is nearby. The scan is unfiltered because
// phones put the caBLE UUID in the service data, not in the service list
// that CoreBluetooth filters on.

typedef void (*ytm_ble_advert_fn)(void *ctx, const uint8_t *data, size_t len);
typedef void (*ytm_ble_state_fn)(void *ctx, int state);

@interface YTMCableScanner : NSObject <CBCentralManagerDelegate>
@property(nonatomic, strong) CBCentralManager *manager;
@property(nonatomic, strong) dispatch_queue_t queue;
@property(nonatomic, strong) NSArray<CBUUID *> *uuids;
@property(nonatomic, assign) void *ctx;
@property(nonatomic, assign) ytm_ble_advert_fn onAdvert;
@property(nonatomic, assign) ytm_ble_state_fn onState;
@property(nonatomic, assign) BOOL stopped;
@end

@implementation YTMCableScanner
- (void)centralManagerDidUpdateState:(CBCentralManager *)central {
    if (self.stopped) return;
    self.onState(self.ctx, (int)central.state);
    if (central.state == CBManagerStatePoweredOn) {
        [central scanForPeripheralsWithServices:nil
                                        options:@{CBCentralManagerScanOptionAllowDuplicatesKey: @YES}];
    }
}

- (void)centralManager:(CBCentralManager *)central
 didDiscoverPeripheral:(CBPeripheral *)peripheral
     advertisementData:(NSDictionary<NSString *, id> *)advertisementData
                  RSSI:(NSNumber *)RSSI
{
    if (self.stopped) return;
    NSDictionary<CBUUID *, NSData *> *serviceData = advertisementData[CBAdvertisementDataServiceDataKey];
    if (serviceData == nil) return;
    for (CBUUID *uuid in self.uuids) {
        NSData *data = serviceData[uuid];
        if (data != nil) {
            self.onAdvert(self.ctx, data.bytes, data.length);
        }
    }
}
@end

void *ytm_ble_scan_start(void *ctx, ytm_ble_advert_fn on_advert, ytm_ble_state_fn on_state) {
    YTMCableScanner *scanner = [YTMCableScanner new];
    scanner.ctx = ctx;
    scanner.onAdvert = on_advert;
    scanner.onState = on_state;
    // FIDO's caBLE UUID, and Google's older one that iOS 16 still sends.
    scanner.uuids = @[[CBUUID UUIDWithString:@"FFF9"], [CBUUID UUIDWithString:@"FDE2"]];
    scanner.queue = dispatch_queue_create("com.charleslee.ytmyagami.cable", DISPATCH_QUEUE_SERIAL);
    scanner.manager = [[CBCentralManager alloc]
        initWithDelegate:scanner
                   queue:scanner.queue
                 options:@{CBCentralManagerOptionShowPowerAlertKey: @NO}];
    return (__bridge_retained void *)scanner;
}

// Returns only after the last callback has run, so the caller can free `ctx`.
void ytm_ble_scan_stop(void *handle) {
    YTMCableScanner *scanner = (__bridge_transfer YTMCableScanner *)handle;
    dispatch_sync(scanner.queue, ^{
        scanner.stopped = YES;
        if (scanner.manager.state == CBManagerStatePoweredOn) {
            [scanner.manager stopScan];
        }
        scanner.manager.delegate = nil;
    });
}
