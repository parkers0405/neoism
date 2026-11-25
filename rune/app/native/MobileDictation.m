#import <AVFoundation/AVFoundation.h>
#import <Speech/Speech.h>
#import <UIKit/UIKit.h>

typedef void (*DictationReply)(void *, const char *, const char *);

@interface NeoismDictation : NSObject
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) SFSpeechRecognizer *recognizer;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *request;
@property(nonatomic, strong) SFSpeechRecognitionTask *task;
@property(nonatomic, copy) NSString *transcript;
@property(nonatomic) void *context;
@property(nonatomic) DictationReply reply;
@property(nonatomic) BOOL tapInstalled;
@property(nonatomic) BOOL audioActive;
@property(nonatomic) BOOL stopping;
@end

static NeoismDictation *active;

@implementation NeoismDictation
- (void)stopAudio {
    [self.engine stop];
    if (self.tapInstalled) {
        [self.engine.inputNode removeTapOnBus:0];
        self.tapInstalled = NO;
    }
}

- (void)complete:(NSString *)error {
    if (active != self) return;
    active = nil;
    [NSNotificationCenter.defaultCenter removeObserver:self];
    [self stopAudio];
    [self.task cancel];
    if (self.audioActive) {
        [AVAudioSession.sharedInstance setActive:NO withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation error:nil];
    }
    self.reply(self.context, error ? NULL : self.transcript.UTF8String, error.UTF8String);
}

- (void)cancelOnBackground {
    self.transcript = nil;
    [self complete:nil];
}

- (void)finish {
    if (active != self || self.stopping) return;
    self.stopping = YES;
    if (!self.task) { [self complete:nil]; return; }
    [self stopAudio];
    [self.request endAudio];
    __weak NeoismDictation *weak = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [weak complete:nil];
    });
}

- (void)record {
    if (active != self) return;
    self.recognizer = [[SFSpeechRecognizer alloc] init];
    if (!self.recognizer.available) { [self complete:@"Speech recognition is unavailable right now."]; return; }
    AVAudioSession *session = AVAudioSession.sharedInstance;
    if (![session setCategory:AVAudioSessionCategoryRecord mode:AVAudioSessionModeMeasurement options:0 error:nil]
        || ![session setActive:YES error:nil]) {
        [self complete:@"Could not start the microphone."]; return;
    }
    self.audioActive = YES;
    self.engine = [[AVAudioEngine alloc] init];
    AVAudioInputNode *input = self.engine.inputNode;
    AVAudioFormat *format = [input outputFormatForBus:0];
    if (format.sampleRate == 0 || format.channelCount == 0) { [self complete:@"No microphone is available."]; return; }
    SFSpeechAudioBufferRecognitionRequest *request = [[SFSpeechAudioBufferRecognitionRequest alloc] init];
    self.request = request;
    request.shouldReportPartialResults = YES;
    request.requiresOnDeviceRecognition = self.recognizer.supportsOnDeviceRecognition;
    [input installTapOnBus:0 bufferSize:1024 format:format block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
        [request appendAudioPCMBuffer:buffer];
    }];
    self.tapInstalled = YES;
    if (![self.engine startAndReturnError:nil]) { [self complete:@"Could not start the microphone."]; return; }
    __weak NeoismDictation *weak = self;
    self.task = [self.recognizer recognitionTaskWithRequest:request resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
        dispatch_async(dispatch_get_main_queue(), ^{
            NeoismDictation *current = weak;
            if (!current || active != current) return;
            if (result) current.transcript = result.bestTranscription.formattedString;
            if (result.isFinal || error) [current complete:current.transcript.length ? nil : @"No speech was recognized."];
        });
    }];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 45 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        [weak finish];
    });
}
@end

void neoism_mobile_dictate(void *context, DictationReply reply) {
    if (active) { reply(context, NULL, "Dictation is already in progress."); return; }
    NeoismDictation *work = [[NeoismDictation alloc] init];
    active = work;
    work.context = context;
    work.reply = reply;
    [NSNotificationCenter.defaultCenter addObserver:work selector:@selector(cancelOnBackground)
        name:UIApplicationDidEnterBackgroundNotification object:nil];
    [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (active != work) return;
            if (status != SFSpeechRecognizerAuthorizationStatusAuthorized) {
                [work complete:@"Allow Speech Recognition for Neoism in iOS Settings."]; return;
            }
            [AVAudioSession.sharedInstance requestRecordPermission:^(BOOL granted) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (active != work) return;
                    if (granted) [work record];
                    else [work complete:@"Allow Microphone access for Neoism in iOS Settings."];
                });
            }];
        });
    }];
}

void neoism_mobile_dictate_finish(void) { [active finish]; }