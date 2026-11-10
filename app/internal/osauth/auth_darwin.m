//go:build darwin && !ios && cgo && !server

#import <Foundation/Foundation.h>
#import <LocalAuthentication/LocalAuthentication.h>

@interface KTAuthentication : NSObject
@property(nonatomic, strong) LAContext *context;
@property(nonatomic) NSInteger result;
@end
@implementation KTAuthentication
@end

void *KTAuthBegin(const char *reason) {
    @autoreleasepool {
        KTAuthentication *task = [KTAuthentication new];
        task.result = -1;
        task.context = [LAContext new];
        NSError *error = nil;
        if (![task.context canEvaluatePolicy:LAPolicyDeviceOwnerAuthentication error:&error]) {
            task.result = 2;
        } else {
            [task.context evaluatePolicy:LAPolicyDeviceOwnerAuthentication
                localizedReason:[NSString stringWithUTF8String:reason]
                reply:^(BOOL success, NSError *failure) {
                    @synchronized(task) {
                        if (success) task.result = 0;
                        else if (failure.code == LAErrorUserCancel || failure.code == LAErrorSystemCancel || failure.code == LAErrorAppCancel) task.result = 1;
                        else task.result = 3;
                    }
                }];
        }
        return (__bridge_retained void *)task;
    }
}
int KTAuthStatus(void *pointer) {
    KTAuthentication *task = (__bridge KTAuthentication *)pointer;
    @synchronized(task) { return (int)task.result; }
}
void KTAuthEnd(void *pointer) {
    @autoreleasepool {
        KTAuthentication *task = (__bridge_transfer KTAuthentication *)pointer;
        [task.context invalidate];
        task.context = nil;
    }
}
