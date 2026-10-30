// Loads a built SidecastMic.driver the way coreaudiod does (CFPlugIn: Info.plist
// factory + type → factory function), then drives it with a stub host and
// checks the published devices, formats and the feed → microphone loopback.
// Never talks to coreaudiod and installs nothing.
//
//   check-mic-driver path/to/SidecastMic.driver

#include <CoreAudio/AudioHardware.h>
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

static int failures = 0;
#define CHECK(cond, ...) do { if (!(cond)) { failures++; fprintf(stderr, "FAIL: " __VA_ARGS__); fputc('\n', stderr); } } while (0)

static OSStatus host_properties_changed(AudioServerPlugInHostRef h, AudioObjectID o, UInt32 n, const AudioObjectPropertyAddress *a) { return 0; }
static OSStatus host_copy(AudioServerPlugInHostRef h, CFStringRef k, CFPropertyListRef *out) { *out = NULL; return 0; }
static OSStatus host_write(AudioServerPlugInHostRef h, CFStringRef k, CFPropertyListRef d) { return 0; }
static OSStatus host_delete(AudioServerPlugInHostRef h, CFStringRef k) { return 0; }
static OSStatus host_request(AudioServerPlugInHostRef h, AudioObjectID d, UInt64 a, void *i) { return 0; }

static const AudioServerPlugInHostInterface gHost = {
    host_properties_changed, host_copy, host_write, host_delete, host_request,
};

static AudioServerPlugInDriverRef driver;

static OSStatus get(AudioObjectID obj, AudioObjectPropertySelector sel, AudioObjectPropertyScope scope,
                    UInt32 size, void *out, UInt32 qsize, const void *q) {
    AudioObjectPropertyAddress a = { sel, scope, kAudioObjectPropertyElementMain };
    UInt32 outSize = 0;
    return (*driver)->GetPropertyData(driver, obj, 0, &a, qsize, q, size, &outSize, out);
}

static UInt32 get_u32(AudioObjectID obj, AudioObjectPropertySelector sel, AudioObjectPropertyScope scope) {
    UInt32 v = 0xFFFFFFFF;
    CHECK(get(obj, sel, scope, sizeof v, &v, 0, NULL) == 0, "object %u property '%.4s'", obj, (char *)&sel);
    return v;
}

static void get_string(AudioObjectID obj, AudioObjectPropertySelector sel, char *buf, size_t len) {
    CFStringRef s = NULL;
    buf[0] = 0;
    if (get(obj, sel, kAudioObjectPropertyScopeGlobal, sizeof s, &s, 0, NULL) == 0 && s) {
        CFStringGetCString(s, buf, (CFIndex)len, kCFStringEncodingUTF8);
        CFRelease(s);
    }
}

static AudioObjectID stream_of(AudioObjectID device, AudioObjectPropertyScope scope) {
    AudioObjectID stream = kAudioObjectUnknown;
    get(device, kAudioDevicePropertyStreams, scope, sizeof stream, &stream, 0, NULL);
    return stream;
}

static AudioObjectID translate(const char *uid) {
    CFStringRef s = CFStringCreateWithCString(NULL, uid, kCFStringEncodingUTF8);
    AudioObjectID id = kAudioObjectUnknown;
    get(kAudioObjectPlugInObject, kAudioPlugInPropertyTranslateUIDToDevice, kAudioObjectPropertyScopeGlobal,
        sizeof id, &id, sizeof s, &s);
    CFRelease(s);
    return id;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s SidecastMic.driver\n", argv[0]);
        return 2;
    }
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)argv[1], (CFIndex)strlen(argv[1]), true);
    CFPlugInRef plugin = url ? CFPlugInCreate(NULL, url) : NULL;
    if (!plugin) {
        fprintf(stderr, "FAIL: cannot open %s as a CFPlugIn\n", argv[1]);
        return 1;
    }
    CFArrayRef factories = CFPlugInFindFactoriesForPlugInTypeInPlugIn(kAudioServerPlugInTypeUUID, plugin);
    if (!factories || CFArrayGetCount(factories) != 1) {
        fprintf(stderr, "FAIL: expected exactly one AudioServerPlugIn factory in Info.plist\n");
        return 1;
    }
    CFUUIDRef factory = CFArrayGetValueAtIndex(factories, 0);
    CFStringRef factoryString = CFUUIDCreateString(NULL, factory);
    char factoryBuf[64];
    CFStringGetCString(factoryString, factoryBuf, sizeof factoryBuf, kCFStringEncodingUTF8);
    void *iunknown = CFPlugInInstanceCreate(NULL, factory, kAudioServerPlugInTypeUUID);
    if (!iunknown) {
        fprintf(stderr, "FAIL: factory %s returned NULL (symbol missing?)\n", factoryBuf);
        return 1;
    }
    printf("factory %s → driver interface loaded\n", factoryBuf);
    IUnknownVTbl **unknown = iunknown;
    void *iface = NULL;
    CHECK((*unknown)->QueryInterface(unknown, CFUUIDGetUUIDBytes(kAudioServerPlugInDriverInterfaceUUID), &iface) == S_OK,
          "QueryInterface(driver interface)");
    driver = iface;
    CHECK((*driver)->Initialize(driver, &gHost) == 0, "Initialize");

    AudioObjectID devices[4] = { 0 };
    UInt32 outSize = 0;
    AudioObjectPropertyAddress listAddr = { kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    CHECK((*driver)->GetPropertyData(driver, kAudioObjectPlugInObject, 0, &listAddr, 0, NULL, sizeof devices, &outSize, devices) == 0,
          "device list");
    UInt32 count = outSize / sizeof(AudioObjectID);
    CHECK(count == 2, "expected 2 devices, got %u", count);

    for (UInt32 i = 0; i < count; i++) {
        AudioObjectID d = devices[i];
        char name[128], uid[128];
        get_string(d, kAudioObjectPropertyName, name, sizeof name);
        get_string(d, kAudioDevicePropertyDeviceUID, uid, sizeof uid);
        UInt32 hidden = get_u32(d, kAudioDevicePropertyIsHidden, kAudioObjectPropertyScopeGlobal);
        UInt32 inStreams = 0, outStreams = 0;
        AudioObjectPropertyAddress sa = { kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain };
        (*driver)->GetPropertyDataSize(driver, d, 0, &sa, 0, NULL, &inStreams);
        sa.mScope = kAudioObjectPropertyScopeOutput;
        (*driver)->GetPropertyDataSize(driver, d, 0, &sa, 0, NULL, &outStreams);
        AudioObjectID stream = 0;
        AudioObjectPropertyScope scope = inStreams ? kAudioObjectPropertyScopeInput : kAudioObjectPropertyScopeOutput;
        get(d, kAudioDevicePropertyStreams, scope, sizeof stream, &stream, 0, NULL);
        AudioStreamBasicDescription f;
        memset(&f, 0, sizeof f);
        get(stream, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal, sizeof f, &f, 0, NULL);
        Float64 rate = 0;
        get(d, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, sizeof rate, &rate, 0, NULL);
        printf("device %u \"%s\" uid=%s hidden=%u inputStreams=%u outputStreams=%u rate=%.0f format=%.0f Hz %u ch %u-bit %s clockDomain=0x%08x\n",
               d, name, uid, hidden, inStreams / 4, outStreams / 4, rate, f.mSampleRate, f.mChannelsPerFrame,
               f.mBitsPerChannel, (f.mFormatFlags & kAudioFormatFlagIsFloat) ? "float" : "int",
               get_u32(d, kAudioDevicePropertyClockDomain, kAudioObjectPropertyScopeGlobal));
        CHECK(rate == 48000 && f.mSampleRate == 48000 && f.mChannelsPerFrame == 1 && f.mBitsPerChannel == 32,
              "device %u format", d);
    }

    AudioObjectID input = translate("SidecastMic_Input"), feed = translate("SidecastMic_Feed");
    CHECK(input != kAudioObjectUnknown && feed != kAudioObjectUnknown, "UID translation");
    CHECK(get_u32(input, kAudioDevicePropertyIsHidden, kAudioObjectPropertyScopeGlobal) == 0, "microphone must be visible");
    CHECK(get_u32(feed, kAudioDevicePropertyIsHidden, kAudioObjectPropertyScopeGlobal) == 1, "feed must be hidden");

    // Running state.
    CHECK(get_u32(input, kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioObjectPropertyScopeGlobal) == 0, "idle at start");
    (*driver)->StartIO(driver, input, 1);
    (*driver)->StartIO(driver, feed, 2);
    CHECK(get_u32(input, kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioObjectPropertyScopeGlobal) == 1, "running after StartIO");

    // Loopback at the current zero time stamp: feed WriteMix → microphone ReadInput.
    Float64 sampleTime = 0;
    UInt64 hostTime = 0, seed = 0;
    CHECK((*driver)->GetZeroTimeStamp(driver, input, 1, &sampleTime, &hostTime, &seed) == 0, "zero time stamp");
    enum { kFrames = 480 };
    float tone[kFrames], heard[kFrames];
    for (int i = 0; i < kFrames; i++) tone[i] = (float)sin(2 * M_PI * 440 * i / 48000.0);
    AudioObjectID inputStream = stream_of(input, kAudioObjectPropertyScopeInput);
    AudioObjectID feedStream = stream_of(feed, kAudioObjectPropertyScopeOutput);
    CHECK(inputStream != kAudioObjectUnknown && feedStream != kAudioObjectUnknown, "streams");
    AudioServerPlugInIOCycleInfo cycle;
    memset(&cycle, 0, sizeof cycle);
    cycle.mOutputTime.mSampleTime = sampleTime;
    cycle.mInputTime.mSampleTime = sampleTime;
    (*driver)->DoIOOperation(driver, feed, feedStream, 2, kAudioServerPlugInIOOperationWriteMix, kFrames, &cycle, tone, NULL);
    (*driver)->DoIOOperation(driver, input, inputStream, 1, kAudioServerPlugInIOOperationReadInput, kFrames, &cycle, heard, NULL);
    int loopbackOK = memcmp(tone, heard, sizeof tone) == 0;
    CHECK(loopbackOK, "loopback returned different audio");
    cycle.mInputTime.mSampleTime = sampleTime + kFrames;
    (*driver)->DoIOOperation(driver, input, inputStream, 1, kAudioServerPlugInIOOperationReadInput, kFrames, &cycle, heard, NULL);
    float peak = 0;
    for (int i = 0; i < kFrames; i++) peak = fmaxf(peak, fabsf(heard[i]));
    CHECK(peak == 0, "unfed range must be silent");
    printf("loopback: feed → microphone %s, unfed range silent %s\n", loopbackOK ? "OK" : "NO", peak == 0 ? "OK" : "NO");

    (*driver)->StopIO(driver, input, 1);
    (*driver)->StopIO(driver, feed, 2);
    CHECK(get_u32(input, kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioObjectPropertyScopeGlobal) == 0, "idle after StopIO");
    (*driver)->Release(driver);
    (*unknown)->Release(unknown);

    if (failures) {
        printf("mic driver check: %d FAILURE(S)\n", failures);
        return 1;
    }
    printf("mic driver check: OK\n");
    return 0;
}
