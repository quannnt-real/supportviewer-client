#import <AVFoundation/AVFoundation.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#include <os/lock.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static NSArray<AVCaptureDevice *> *sv_list_video_devices(void) {
    NSArray<AVCaptureDevice *> *devices = nil;
    if (@available(macOS 10.15, *)) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        NSArray<AVCaptureDeviceType> *types = @[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternalUnknown
        ];
#pragma clang diagnostic pop
        AVCaptureDeviceDiscoverySession *discovery = [AVCaptureDeviceDiscoverySession
            discoverySessionWithDeviceTypes:types
                                  mediaType:AVMediaTypeVideo
                                   position:AVCaptureDevicePositionUnspecified];
        devices = discovery.devices;
    } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        devices = [AVCaptureDevice devicesWithMediaType:AVMediaTypeVideo];
#pragma clang diagnostic pop
    }
    if (!devices || devices.count == 0) {
        return @[];
    }
    return [devices sortedArrayUsingComparator:^NSComparisonResult(AVCaptureDevice *a, AVCaptureDevice *b) {
        NSString *idA = a.uniqueID ?: @"";
        NSString *idB = b.uniqueID ?: @"";
        return [idA compare:idB];
    }];
}

static void sv_select_preset(AVCaptureDevice *device, NSString **outPreset, int *outW, int *outH) {
    if ([device supportsAVCaptureSessionPreset:AVCaptureSessionPreset1280x720]) {
        if (outPreset) *outPreset = AVCaptureSessionPreset1280x720;
        if (outW) *outW = 1280;
        if (outH) *outH = 720;
    } else {
        if (outPreset) *outPreset = AVCaptureSessionPreset640x480;
        if (outW) *outW = 640;
        if (outH) *outH = 480;
    }
}

@interface SVCameraContext : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate>
@property (nonatomic, strong) AVCaptureSession *session;
@property (nonatomic, strong) AVCaptureDeviceInput *input;
@property (nonatomic, strong) AVCaptureVideoDataOutput *output;
@property (nonatomic, strong) id runtimeErrorObserver;
@property (nonatomic, strong) id interruptedObserver;
@end

@implementation SVCameraContext {
@public
    dispatch_queue_t _queue;
    os_unfair_lock _lock;
    uint8_t *_buffer;
    size_t _bufferCap;
    int _frameWidth;
    int _frameHeight;
    int _frameStride;
    BOOL _hasNewFrame;
    BOOL _hasError;
}

- (instancetype)init {
    self = [super init];
    if (self) {
        _lock = OS_UNFAIR_LOCK_INIT;
        _queue = dispatch_queue_create("com.supportviewer.camera.capture", DISPATCH_QUEUE_SERIAL);
        _buffer = NULL;
        _bufferCap = 0;
        _frameWidth = 0;
        _frameHeight = 0;
        _frameStride = 0;
        _hasNewFrame = NO;
        _hasError = NO;
    }
    return self;
}

- (void)dealloc {
    if (_buffer) {
        free(_buffer);
        _buffer = NULL;
    }
}

- (void)markError {
    os_unfair_lock_lock(&_lock);
    _hasError = YES;
    os_unfair_lock_unlock(&_lock);
}

- (void)captureOutput:(AVCaptureOutput *)output
didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
       fromConnection:(AVCaptureConnection *)connection {
    @autoreleasepool {
        CVImageBufferRef pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer);
        if (!pixelBuffer) {
            return;
        }
        if (CVPixelBufferLockBaseAddress(pixelBuffer, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
            return;
        }
        void *baseAddr = CVPixelBufferGetBaseAddress(pixelBuffer);
        size_t width = CVPixelBufferGetWidth(pixelBuffer);
        size_t height = CVPixelBufferGetHeight(pixelBuffer);
        size_t stride = CVPixelBufferGetBytesPerRow(pixelBuffer);
        size_t totalBytes = stride * height;

        if (baseAddr && width > 0 && height > 0 && stride >= width * 4 && totalBytes > 0) {
            os_unfair_lock_lock(&_lock);
            if (!_hasError) {
                if (totalBytes > _bufferCap) {
                    uint8_t *newBuf = (uint8_t *)realloc(_buffer, totalBytes);
                    if (newBuf) {
                        _buffer = newBuf;
                        _bufferCap = totalBytes;
                    }
                }
                if (_buffer && _bufferCap >= totalBytes) {
                    memcpy(_buffer, baseAddr, totalBytes);
                    _frameWidth = (int)width;
                    _frameHeight = (int)height;
                    _frameStride = (int)stride;
                    _hasNewFrame = YES;
                }
            }
            os_unfair_lock_unlock(&_lock);
        }
        CVPixelBufferUnlockBaseAddress(pixelBuffer, kCVPixelBufferLock_ReadOnly);
    }
}

@end

int sv_camera_auth_status(void) {
    @autoreleasepool {
        AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
        switch (status) {
            case AVAuthorizationStatusAuthorized:
                return 1;
            case AVAuthorizationStatusNotDetermined:
                return 0;
            default:
                return -1;
        }
    }
}

int sv_camera_count(void) {
    @autoreleasepool {
        if (sv_camera_auth_status() != 1) {
            return 0;
        }
        NSArray<AVCaptureDevice *> *devices = sv_list_video_devices();
        return (int)devices.count;
    }
}

int sv_camera_name(int idx, char *buf, int len) {
    @autoreleasepool {
        if (!buf || len <= 0 || idx < 0) {
            return -1;
        }
        buf[0] = '\0';
        if (sv_camera_auth_status() != 1) {
            return -1;
        }
        NSArray<AVCaptureDevice *> *devices = sv_list_video_devices();
        if ((NSUInteger)idx >= devices.count) {
            return -1;
        }
        AVCaptureDevice *device = devices[(NSUInteger)idx];
        NSString *name = device.localizedName;
        if (!name || name.length == 0) {
            name = device.uniqueID ?: @"Camera";
        }
        const char *utf8 = [name UTF8String];
        if (!utf8) {
            return -1;
        }
        size_t copyLen = strlen(utf8);
        if (copyLen >= (size_t)len) {
            copyLen = (size_t)len - 1;
        }
        memcpy(buf, utf8, copyLen);
        buf[copyLen] = '\0';
        return (int)copyLen;
    }
}

int sv_camera_resolution(int idx, int *w, int *h) {
    @autoreleasepool {
        if (!w || !h || idx < 0) {
            return -1;
        }
        if (sv_camera_auth_status() != 1) {
            return -1;
        }
        NSArray<AVCaptureDevice *> *devices = sv_list_video_devices();
        if ((NSUInteger)idx >= devices.count) {
            return -1;
        }
        AVCaptureDevice *device = devices[(NSUInteger)idx];
        sv_select_preset(device, NULL, w, h);
        return 0;
    }
}

void *sv_camera_open(int idx, int *w, int *h) {
    @autoreleasepool {
        if (idx < 0 || sv_camera_auth_status() != 1) {
            return NULL;
        }
        NSArray<AVCaptureDevice *> *devices = sv_list_video_devices();
        if ((NSUInteger)idx >= devices.count) {
            return NULL;
        }
        AVCaptureDevice *device = devices[(NSUInteger)idx];
        NSError *error = nil;
        AVCaptureDeviceInput *input = [AVCaptureDeviceInput deviceInputWithDevice:device error:&error];
        if (!input || error) {
            return NULL;
        }

        AVCaptureSession *session = [[AVCaptureSession alloc] init];
        [session beginConfiguration];

        NSString *preset = nil;
        int presetW = 640;
        int presetH = 480;
        sv_select_preset(device, &preset, &presetW, &presetH);
        if (preset && [session canSetSessionPreset:preset]) {
            session.sessionPreset = preset;
        } else if ([session canSetSessionPreset:AVCaptureSessionPreset640x480]) {
            session.sessionPreset = AVCaptureSessionPreset640x480;
            presetW = 640;
            presetH = 480;
        }

        if (![session canAddInput:input]) {
            [session commitConfiguration];
            return NULL;
        }
        [session addInput:input];

        AVCaptureVideoDataOutput *output = [[AVCaptureVideoDataOutput alloc] init];
        output.videoSettings = @{
            (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA)
        };
        output.alwaysDiscardsLateVideoFrames = YES;

        if (![session canAddOutput:output]) {
            [session removeInput:input];
            [session commitConfiguration];
            return NULL;
        }
        [session addOutput:output];

        SVCameraContext *ctx = [[SVCameraContext alloc] init];
        ctx.session = session;
        ctx.input = input;
        ctx.output = output;
        ctx->_frameWidth = presetW;
        ctx->_frameHeight = presetH;
        ctx->_frameStride = presetW * 4;

        [output setSampleBufferDelegate:ctx queue:ctx->_queue];
        [session commitConfiguration];

        __weak SVCameraContext *weakCtx = ctx;
        NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
        ctx.runtimeErrorObserver = [nc addObserverForName:AVCaptureSessionRuntimeErrorNotification
                                                   object:session
                                                    queue:nil
                                               usingBlock:^(NSNotification *note) {
            [weakCtx markError];
        }];
        ctx.interruptedObserver = [nc addObserverForName:AVCaptureSessionWasInterruptedNotification
                                                  object:session
                                                   queue:nil
                                              usingBlock:^(NSNotification *note) {
            [weakCtx markError];
        }];

        [session startRunning];
        if (!session.isRunning) {
            if (ctx.runtimeErrorObserver) {
                [nc removeObserver:ctx.runtimeErrorObserver];
            }
            if (ctx.interruptedObserver) {
                [nc removeObserver:ctx.interruptedObserver];
            }
            [output setSampleBufferDelegate:nil queue:NULL];
            return NULL;
        }

        if (w) *w = presetW;
        if (h) *h = presetH;
        return (__bridge_retained void *)ctx;
    }
}

int sv_camera_frame(void *h, uint8_t *dst, int dst_len, int *w, int *h_out, int *stride) {
    @autoreleasepool {
        if (!h) {
            return -1;
        }
        SVCameraContext *ctx = (__bridge SVCameraContext *)h;
        int ret = 0;
        BOOL running = ctx.session.isRunning;
        os_unfair_lock_lock(&ctx->_lock);
        if (ctx->_hasError || !running) {
            ret = -2;
        } else if (ctx->_hasNewFrame && ctx->_buffer && ctx->_frameWidth > 0 && ctx->_frameHeight > 0 && ctx->_frameStride >= ctx->_frameWidth * 4) {
            int needed = ctx->_frameStride * ctx->_frameHeight;
            if (w) *w = ctx->_frameWidth;
            if (h_out) *h_out = ctx->_frameHeight;
            if (stride) *stride = ctx->_frameStride;
            if (!dst || dst_len < needed) {
                ret = -3;
            } else {
                memcpy(dst, ctx->_buffer, (size_t)needed);
                ctx->_hasNewFrame = NO;
                ret = 1;
            }
        } else {
            ret = 0;
        }
        os_unfair_lock_unlock(&ctx->_lock);
        return ret;
    }
}

void sv_camera_close(void *h) {
    @autoreleasepool {
        if (!h) {
            return;
        }
        SVCameraContext *ctx = (__bridge_transfer SVCameraContext *)h;
        NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
        if (ctx.runtimeErrorObserver) {
            [nc removeObserver:ctx.runtimeErrorObserver];
            ctx.runtimeErrorObserver = nil;
        }
        if (ctx.interruptedObserver) {
            [nc removeObserver:ctx.interruptedObserver];
            ctx.interruptedObserver = nil;
        }
        if (ctx.output) {
            [ctx.output setSampleBufferDelegate:nil queue:NULL];
        }
        if (ctx->_queue) {
            dispatch_sync(ctx->_queue, ^{});
        }
        if (ctx.session) {
            if (ctx.session.isRunning) {
                [ctx.session stopRunning];
            }
            [ctx.session beginConfiguration];
            if (ctx.input) {
                [ctx.session removeInput:ctx.input];
            }
            if (ctx.output) {
                [ctx.session removeOutput:ctx.output];
            }
            [ctx.session commitConfiguration];
        }
        ctx.input = nil;
        ctx.output = nil;
        ctx.session = nil;
    }
}
