#import <Foundation/Foundation.h>
#import <TargetConditionals.h>

// `context` belongs to Rust until the single reply, even on failure or cancel.
typedef void (*QRCallback)(void *context, int status, const uint8_t *bytes, size_t length);
enum { QRCancelled, QRScanned, QRError };

static void QRReply(void *context, QRCallback callback, int status, NSString *message) {
    NSData *data = [message dataUsingEncoding:NSUTF8StringEncoding];
    callback(context, status, data.bytes, data.length);
}

#if TARGET_OS_OSX
#import <Vision/Vision.h>

API_AVAILABLE(macos(10.13))
static void QRDecodeImage(NSURL *url, void *context, QRCallback callback) {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        @autoreleasepool {
            VNDetectBarcodesRequest *request = [VNDetectBarcodesRequest new];
            request.symbologies = @[VNBarcodeSymbologyQR];
            VNImageRequestHandler *image = [[VNImageRequestHandler alloc] initWithURL:url options:@{}];
            if (!image || ![image performRequests:@[request] error:NULL]) {
                QRReply(context, callback, QRError, @"Could not read the selected image.");
                return;
            }
            NSString *payload = nil;
            for (VNBarcodeObservation *code in request.results) {
                NSString *text = code.payloadStringValue;
                if (!text.length) continue;
                if (payload && ![payload isEqualToString:text]) {
                    QRReply(context, callback, QRError, @"The image contains multiple QR codes. Choose an image with just the pairing QR code.");
                    return;
                }
                payload = text;
            }
            if (payload) QRReply(context, callback, QRScanned, payload);
            else QRReply(context, callback, QRError, @"No readable QR code was found in the selected image.");
        }
    });
}

void neoism_qr_image(const char *path, void *context, QRCallback callback) {
    if (@available(macOS 10.13, *)) {
        NSURL *url = [NSURL fileURLWithFileSystemRepresentation:path isDirectory:NO relativeToURL:nil];
        QRDecodeImage(url, context, callback);
    } else {
        QRReply(context, callback, QRError, @"Image scanning is not available on this version of macOS. Paste the pairing link instead.");
    }
}

#elif TARGET_OS_IOS
#import <AVFoundation/AVFoundation.h>
#import <UIKit/UIKit.h>

#if !TARGET_OS_SIMULATOR
API_AVAILABLE(ios(13.0))
@interface NeoismQRScanner : UIViewController <AVCaptureMetadataOutputObjectsDelegate>
@property(nonatomic) void *context;
@property(nonatomic) QRCallback callback;
@end

@implementation NeoismQRScanner {
    AVCaptureSession *_session;
    AVCaptureVideoPreviewLayer *_preview;
    dispatch_queue_t _captureQueue;
    BOOL _started;
}

- (void)viewDidLoad {
    [super viewDidLoad];
    self.view.backgroundColor = UIColor.blackColor;
    _session = [AVCaptureSession new];
    _captureQueue = dispatch_queue_create("ai.neoism.mobile.qr", DISPATCH_QUEUE_SERIAL);
    _preview = [AVCaptureVideoPreviewLayer layerWithSession:_session];
    _preview.videoGravity = AVLayerVideoGravityResizeAspectFill;
    [self.view.layer addSublayer:_preview];

    UIButton *cancel = [UIButton buttonWithType:UIButtonTypeSystem];
    [cancel setTitle:@"Cancel" forState:UIControlStateNormal];
    [cancel setTitleColor:UIColor.whiteColor forState:UIControlStateNormal];
    [cancel addTarget:self action:@selector(cancelScan) forControlEvents:UIControlEventTouchUpInside];

    UILabel *prompt = [UILabel new];
    prompt.text = @"Point the camera at the pairing QR code.";
    prompt.textColor = UIColor.whiteColor;
    prompt.numberOfLines = 0;
    prompt.font = [UIFont preferredFontForTextStyle:UIFontTextStyleBody];
    prompt.adjustsFontForContentSizeCategory = YES;

    UIStackView *bar = [[UIStackView alloc] initWithArrangedSubviews:@[prompt, cancel]];
    bar.spacing = 16;
    bar.alignment = UIStackViewAlignmentCenter;
    bar.backgroundColor = [UIColor colorWithWhite:0 alpha:0.8];
    bar.layoutMargins = UIEdgeInsetsMake(8, 16, 8, 16);
    bar.layoutMarginsRelativeArrangement = YES;
    bar.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:bar];
    UILayoutGuide *safe = self.view.safeAreaLayoutGuide;
    [NSLayoutConstraint activateConstraints:@[
        [bar.topAnchor constraintEqualToAnchor:safe.topAnchor],
        [bar.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor],
        [bar.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor],
        [cancel.widthAnchor constraintGreaterThanOrEqualToConstant:64],
        [cancel.heightAnchor constraintGreaterThanOrEqualToConstant:44],
    ]];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    _preview.frame = self.view.bounds;
    AVCaptureConnection *connection = _preview.connection;
    UIInterfaceOrientation orientation = self.view.window.windowScene.interfaceOrientation;
    if (@available(iOS 17.0, *)) {
        CGFloat angle;
        switch (orientation) {
            case UIInterfaceOrientationPortrait: angle = 90; break;
            case UIInterfaceOrientationPortraitUpsideDown: angle = 270; break;
            case UIInterfaceOrientationLandscapeRight: angle = 0; break;
            case UIInterfaceOrientationLandscapeLeft: angle = 180; break;
            default: return;
        }
        if ([connection isVideoRotationAngleSupported:angle]) connection.videoRotationAngle = angle;
    } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        // These enum values intentionally use the same interface-orientation mapping.
        if (orientation != UIInterfaceOrientationUnknown && connection.isVideoOrientationSupported) {
            connection.videoOrientation = (AVCaptureVideoOrientation)orientation;
        }
#pragma clang diagnostic pop
    }
}

- (void)viewDidAppear:(BOOL)animated {
    [super viewDidAppear:animated];
    if (_started || !self.callback) return;
    _started = YES;
    [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(captureError:)
                                               name:AVCaptureSessionRuntimeErrorNotification object:_session];
    AVCaptureSession *session = _session;
    dispatch_async(_captureQueue, ^{
        AVCaptureDevice *camera = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
        AVCaptureDeviceInput *input = camera ? [AVCaptureDeviceInput deviceInputWithDevice:camera error:NULL] : nil;
        AVCaptureMetadataOutput *output = [AVCaptureMetadataOutput new];
        if (input && [session canAddInput:input]) {
            [session addInput:input];
            if ([session canAddOutput:output]) {
                [session addOutput:output];
                if ([output.availableMetadataObjectTypes containsObject:AVMetadataObjectTypeQRCode]) {
                    [output setMetadataObjectsDelegate:self queue:dispatch_get_main_queue()];
                    output.metadataObjectTypes = @[AVMetadataObjectTypeQRCode];
                    [session startRunning];
                    dispatch_async(dispatch_get_main_queue(), ^{ [self.view setNeedsLayout]; });
                    return;
                }
            }
        }
        dispatch_async(dispatch_get_main_queue(), ^{
            [self finish:QRError message:@"The camera could not scan QR codes. Copy and paste the pairing link instead."];
        });
    });
}

- (void)captureError:(__unused NSNotification *)notification {
    dispatch_async(dispatch_get_main_queue(), ^{
        [self finish:QRError message:@"The camera stopped unexpectedly. Try scanning again or paste the pairing link."];
    });
}

- (void)captureOutput:(__unused AVCaptureOutput *)output didOutputMetadataObjects:(NSArray<__kindof AVMetadataObject *> *)objects
       fromConnection:(__unused AVCaptureConnection *)connection {
    NSString *payload = nil;
    for (AVMetadataMachineReadableCodeObject *code in objects) {
        NSString *text = code.stringValue;
        if (!text.length) continue;
        if (payload && ![payload isEqualToString:text]) return;
        payload = text;
    }
    if (payload) [self finish:QRScanned message:payload];
}

- (void)cancelScan {
    [self finish:QRCancelled message:nil];
}

- (void)finish:(int)status message:(NSString *)message {
    QRCallback callback = self.callback;
    if (!callback) return;
    self.callback = NULL;
    void *context = self.context;
    [NSNotificationCenter.defaultCenter removeObserver:self];
    AVCaptureSession *session = _session;
    dispatch_async(_captureQueue, ^{ [session stopRunning]; });
    [self dismissViewControllerAnimated:YES completion:^{ QRReply(context, callback, status, message); }];
}

- (void)dealloc {
    // If UIKit tears down the scene while scanning, release the outstanding native reply.
    if (self.callback) QRReply(self.context, self.callback, QRCancelled, nil);
}
@end

API_AVAILABLE(ios(13.0))
static void QRPresent(void *context, QRCallback callback) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:UIWindowScene.class]) continue;
        UISceneActivationState state = scene.activationState;
        // The permission prompt can leave the scene transiently inactive while dismissing.
        if (state != UISceneActivationStateForegroundActive && state != UISceneActivationStateForegroundInactive) continue;
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            if (!window.isKeyWindow || !window.rootViewController) continue;
            UIViewController *root = window.rootViewController;
            if (root.presentedViewController || !root.view.window) break;
            NeoismQRScanner *scanner = [NeoismQRScanner new];
            scanner.context = context;
            scanner.callback = callback;
            scanner.modalPresentationStyle = UIModalPresentationFullScreen;
            [root presentViewController:scanner animated:YES completion:nil];
            return;
        }
    }
    QRReply(context, callback, QRError, @"No active window is available for the camera. Close any system dialog and try again.");
}
#endif

void neoism_qr_camera(void *context, QRCallback callback) {
#if TARGET_OS_SIMULATOR
    QRReply(context, callback, QRError, @"Camera scanning is not available in the iOS Simulator. Copy the pairing link from desktop Neoism and paste it here.");
#else
    if (@available(iOS 13.0, *)) {
        NSString *denied = @"Camera access is off. Enable it in Settings for Neoism, or paste the pairing link instead.";
        switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo]) {
            case AVAuthorizationStatusAuthorized:
                QRPresent(context, callback);
                return;
            case AVAuthorizationStatusNotDetermined: {
                [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo completionHandler:^(BOOL granted) {
                    dispatch_async(dispatch_get_main_queue(), ^{
                        if (granted) QRPresent(context, callback);
                        else QRReply(context, callback, QRError, denied);
                    });
                }];
                return;
            }
            case AVAuthorizationStatusDenied:
            case AVAuthorizationStatusRestricted:
                QRReply(context, callback, QRError, denied);
                return;
        }
    }
    QRReply(context, callback, QRError, @"Camera scanning requires iOS 13 or later. Paste the pairing link instead.");
#endif
}
#endif