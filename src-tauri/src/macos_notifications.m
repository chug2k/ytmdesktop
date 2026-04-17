#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#import <dispatch/dispatch.h>

@interface YTMNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end

@implementation YTMNotificationDelegate
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions options))completionHandler
{
    completionHandler(
        UNNotificationPresentationOptionBanner |
        UNNotificationPresentationOptionList |
        UNNotificationPresentationOptionSound
    );
}
@end

static YTMNotificationDelegate *g_delegate = nil;

static void ytm_ensure_delegate(void) {
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        g_delegate = [YTMNotificationDelegate new];
        [UNUserNotificationCenter currentNotificationCenter].delegate = g_delegate;
    });
}

void ytm_notifications_request_authorization(void) {
    dispatch_async(dispatch_get_main_queue(), ^{
        ytm_ensure_delegate();

        UNAuthorizationOptions options =
            UNAuthorizationOptionAlert |
            UNAuthorizationOptionSound |
            UNAuthorizationOptionBadge;

        [[UNUserNotificationCenter currentNotificationCenter]
            requestAuthorizationWithOptions:options
                          completionHandler:^(BOOL granted, NSError * _Nullable error) {
            if (error != nil) {
                NSLog(@"[YTM Yagami] notification auth error: %@", error);
            } else {
                NSLog(@"[YTM Yagami] notification auth granted=%d", granted);
            }
        }];
    });
}

void ytm_notifications_show(
    const char *identifier,
    const char *title,
    const char *subtitle,
    const char *image_path
) {
    NSString *nsIdentifier = (identifier && identifier[0] != '\0')
        ? [NSString stringWithUTF8String:identifier]
        : [[NSUUID UUID] UUIDString];

    NSString *nsTitle = (title && title[0] != '\0')
        ? [NSString stringWithUTF8String:title]
        : @"";

    NSString *nsSubtitle = (subtitle && subtitle[0] != '\0')
        ? [NSString stringWithUTF8String:subtitle]
        : @"";

    NSString *nsImagePath = (image_path && image_path[0] != '\0')
        ? [NSString stringWithUTF8String:image_path]
        : @"";

    dispatch_async(dispatch_get_main_queue(), ^{
        ytm_ensure_delegate();

        UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
        [center removeAllDeliveredNotifications];

        UNMutableNotificationContent *content = [UNMutableNotificationContent new];
        content.title = nsTitle;
        content.subtitle = nsSubtitle;
        content.sound = [UNNotificationSound defaultSound];

        if (nsImagePath.length > 0) {
            NSURL *fileURL = [NSURL fileURLWithPath:nsImagePath];
            NSError *attachmentError = nil;
            UNNotificationAttachment *attachment =
                [UNNotificationAttachment attachmentWithIdentifier:@"album-art"
                                                               URL:fileURL
                                                           options:nil
                                                             error:&attachmentError];
            if (attachment != nil) {
                content.attachments = @[attachment];
            } else {
                NSLog(@"[YTM Yagami] attachment error: %@", attachmentError);
            }
        }

        UNTimeIntervalNotificationTrigger *trigger =
            [UNTimeIntervalNotificationTrigger triggerWithTimeInterval:0.1 repeats:NO];

        UNNotificationRequest *request =
            [UNNotificationRequest requestWithIdentifier:nsIdentifier
                                                 content:content
                                                 trigger:trigger];

        [[UNUserNotificationCenter currentNotificationCenter]
            addNotificationRequest:request
             withCompletionHandler:^(NSError * _Nullable error) {
            if (error != nil) {
                NSLog(@"[YTM Yagami] add notification error: %@", error);
            }
        }];
    });
}
