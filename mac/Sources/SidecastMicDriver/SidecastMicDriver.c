// SidecastMic.driver — AudioServerPlugIn publishing "Sidecast Microphone".
//
// Structure follows Apple's NullAudio sample (one plug-in object, devices,
// streams, property dispatch by object ID), written from scratch:
//
//   object 1  plug-in
//   object 2  device "Sidecast Microphone"  input only, visible  (UID SidecastMic_Input)
//   object 3    its input stream
//   object 4  device "Sidecast Mic Feed"    output only, hidden  (UID SidecastMic_Feed)
//   object 5    its output stream
//
// Both devices run on ONE clock: the same anchor host time, the same
// zero-time-stamp period and the same clock domain, so sample time T is the
// same instant on both. The Sidecast app plays the phone's audio into the
// feed; the feed's WriteMix stores it in the shared ring at its output sample
// times and the microphone's ReadInput reads the ring at its input sample
// times. Output runs ahead of "now" and input behind it, so a frame is always
// written before it is read; anything not fed reads back as silence (the ring
// tags every frame with its sample time).
//
// IO path (GetZeroTimeStamp, DoIOOperation): no locks, no allocation, no
// Objective-C/CF calls — just arithmetic and the lock-free ring.

#include "SidecastMicDriver.h"
#include "SidecastRing.h"

#include <CoreAudio/AudioHardware.h>
#include <CoreAudio/AudioServerPlugIn.h>
#include <mach/mach_time.h>
#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>

#define SCM_EXPORT __attribute__((visibility("default")))

#define kSCMManufacturer CFSTR("Sidecast")
#define kSCMModelUID CFSTR("SidecastMic_Model")
#define kSCMClockDomain ((UInt32)0x53434D43) /* 'SCMC', shared by both devices */

enum { kInput = 0, kFeed = 1 };

// ---------------------------------------------------------------- state

static _Atomic(UInt32) gRefCount = 0;
static AudioServerPlugInHostRef gHost = NULL;
static UInt64 gAnchorHostTime = 0;     // set once in Initialize; never changes afterwards
static Float64 gHostTicksPerFrame = 0;
static _Atomic(UInt32) gIOCount[2];    // StartIO minus StopIO, per device
static _Atomic(UInt32) gStreamActive[2] = {1, 1};

// ---------------------------------------------------------------- objects

static int device_index(AudioObjectID object) {
    switch (object) {
    case SCM_OBJECT_INPUT_DEVICE: return kInput;
    case SCM_OBJECT_FEED_DEVICE: return kFeed;
    default: return -1;
    }
}

static int stream_index(AudioObjectID object) {
    switch (object) {
    case SCM_OBJECT_INPUT_STREAM: return kInput;
    case SCM_OBJECT_FEED_STREAM: return kFeed;
    default: return -1;
    }
}

static AudioObjectID device_id(int d) { return d == kInput ? SCM_OBJECT_INPUT_DEVICE : SCM_OBJECT_FEED_DEVICE; }
static AudioObjectID stream_id(int d) { return d == kInput ? SCM_OBJECT_INPUT_STREAM : SCM_OBJECT_FEED_STREAM; }

static CFStringRef device_name(int d) {
    return d == kInput ? CFSTR(SCM_INPUT_DEVICE_NAME) : CFSTR(SCM_FEED_DEVICE_NAME);
}

static CFStringRef device_uid(int d) {
    return d == kInput ? CFSTR(SCM_INPUT_DEVICE_UID) : CFSTR(SCM_FEED_DEVICE_UID);
}

/// The device's stream lives in its own direction's scope (and in global).
static bool scope_has_stream(int d, AudioObjectPropertyScope scope) {
    AudioObjectPropertyScope own = d == kInput ? kAudioObjectPropertyScopeInput : kAudioObjectPropertyScopeOutput;
    return scope == kAudioObjectPropertyScopeGlobal || scope == own;
}

static AudioStreamBasicDescription stream_format(void) {
    AudioStreamBasicDescription f;
    memset(&f, 0, sizeof f);
    f.mSampleRate = SCM_SAMPLE_RATE;
    f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian | kAudioFormatFlagIsPacked;
    f.mBytesPerPacket = sizeof(Float32) * SCM_CHANNELS;
    f.mFramesPerPacket = 1;
    f.mBytesPerFrame = sizeof(Float32) * SCM_CHANNELS;
    f.mChannelsPerFrame = SCM_CHANNELS;
    f.mBitsPerChannel = 32;
    return f;
}

static bool format_is_ours(const AudioStreamBasicDescription *f) {
    AudioStreamBasicDescription ours = stream_format();
    return f->mSampleRate == ours.mSampleRate && f->mFormatID == ours.mFormatID
        && (f->mFormatFlags & ~kAudioFormatFlagIsNonInterleaved) == ours.mFormatFlags
        && f->mChannelsPerFrame == ours.mChannelsPerFrame && f->mBitsPerChannel == ours.mBitsPerChannel
        && f->mBytesPerFrame == ours.mBytesPerFrame && f->mFramesPerPacket == ours.mFramesPerPacket;
}

// ---------------------------------------------------------------- property output

/// Destination of a property value. `data == NULL` means "size query only".
typedef struct {
    void *data;
    UInt32 capacity;
    UInt32 size;
} Out;

static OSStatus put(Out *o, const void *value, UInt32 size) {
    if (o->data) {
        if (o->capacity < size) return kAudioHardwareBadPropertySizeError;
        memcpy(o->data, value, size);
    }
    o->size = size;
    return kAudioHardwareNoError;
}

static OSStatus put_u32(Out *o, UInt32 v) { return put(o, &v, sizeof v); }
static OSStatus put_f64(Out *o, Float64 v) { return put(o, &v, sizeof v); }
/// CFSTR constants are immortal, so the caller's CFRelease is harmless.
static OSStatus put_string(Out *o, CFStringRef s) { return put(o, &s, sizeof s); }

/// Arrays follow the HAL convention: copy as many items as fit, report the bytes written.
static OSStatus put_array(Out *o, const void *items, UInt32 itemSize, UInt32 count) {
    if (!o->data) {
        o->size = itemSize * count;
        return kAudioHardwareNoError;
    }
    UInt32 n = o->capacity / itemSize;
    if (n > count) n = count;
    if (n > 0) memcpy(o->data, items, (size_t)n * itemSize);
    o->size = n * itemSize;
    return kAudioHardwareNoError;
}

static OSStatus put_ids(Out *o, const AudioObjectID *ids, UInt32 count) {
    return put_array(o, ids, sizeof(AudioObjectID), count);
}

// ---------------------------------------------------------------- properties

static OSStatus plugin_property(const AudioObjectPropertyAddress *a, UInt32 qsize, const void *q, Out *o) {
    static const AudioObjectID devices[] = { SCM_OBJECT_INPUT_DEVICE, SCM_OBJECT_FEED_DEVICE };
    switch (a->mSelector) {
    case kAudioObjectPropertyBaseClass: return put_u32(o, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return put_u32(o, kAudioPlugInClassID);
    case kAudioObjectPropertyOwner: return put_u32(o, kAudioObjectUnknown);
    case kAudioObjectPropertyManufacturer: return put_string(o, kSCMManufacturer);
    case kAudioObjectPropertyOwnedObjects:
    case kAudioPlugInPropertyDeviceList:
        return put_ids(o, devices, 2);
    case kAudioPlugInPropertyBoxList: return put_ids(o, NULL, 0);
    case kAudioPlugInPropertyTranslateUIDToBox:
        if (o->data && (qsize != sizeof(CFStringRef) || !q)) return kAudioHardwareBadPropertySizeError;
        return put_u32(o, kAudioObjectUnknown);
    case kAudioPlugInPropertyTranslateUIDToDevice: {
        AudioObjectID found = kAudioObjectUnknown;
        if (o->data) {
            if (qsize != sizeof(CFStringRef) || !q) return kAudioHardwareBadPropertySizeError;
            CFStringRef uid = *(const CFStringRef *)q;
            for (int d = kInput; d <= kFeed; d++) {
                if (uid && CFStringCompare(uid, device_uid(d), 0) == kCFCompareEqualTo) found = device_id(d);
            }
        }
        return put_u32(o, found);
    }
    case kAudioPlugInPropertyResourceBundle: return put_string(o, CFSTR(""));
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus device_property(int d, const AudioObjectPropertyAddress *a, Out *o) {
    switch (a->mSelector) {
    case kAudioObjectPropertyBaseClass: return put_u32(o, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return put_u32(o, kAudioDeviceClassID);
    case kAudioObjectPropertyOwner: return put_u32(o, kAudioObjectPlugInObject);
    case kAudioObjectPropertyName: return put_string(o, device_name(d));
    case kAudioObjectPropertyManufacturer: return put_string(o, kSCMManufacturer);
    case kAudioObjectPropertyOwnedObjects:
    case kAudioDevicePropertyStreams: {
        AudioObjectID stream = stream_id(d);
        return put_ids(o, &stream, scope_has_stream(d, a->mScope) ? 1 : 0);
    }
    case kAudioObjectPropertyControlList: return put_ids(o, NULL, 0);
    case kAudioDevicePropertyDeviceUID: return put_string(o, device_uid(d));
    case kAudioDevicePropertyModelUID: return put_string(o, kSCMModelUID);
    case kAudioDevicePropertyTransportType: return put_u32(o, kAudioDeviceTransportTypeVirtual);
    case kAudioDevicePropertyRelatedDevices: {
        AudioObjectID self = device_id(d);
        return put_ids(o, &self, 1);
    }
    case kAudioDevicePropertyClockDomain: return put_u32(o, kSCMClockDomain);
    case kAudioDevicePropertyDeviceIsAlive: return put_u32(o, 1);
    case kAudioDevicePropertyDeviceIsRunning:
    case kAudioDevicePropertyDeviceIsRunningSomewhere:
        // All IO goes through coreaudiod, so "running here" is "running somewhere".
        return put_u32(o, atomic_load_explicit(&gIOCount[d], memory_order_relaxed) > 0 ? 1 : 0);
    case kAudioDevicePropertyDeviceCanBeDefaultDevice:
        // Only the microphone, and only as an input.
        return put_u32(o, d == kInput && a->mScope != kAudioObjectPropertyScopeOutput ? 1 : 0);
    case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: return put_u32(o, 0);
    case kAudioDevicePropertyLatency: return put_u32(o, 0);
    case kAudioDevicePropertySafetyOffset: return put_u32(o, 0);
    case kAudioDevicePropertyNominalSampleRate: return put_f64(o, SCM_SAMPLE_RATE);
    case kAudioDevicePropertyAvailableNominalSampleRates: {
        AudioValueRange range = { SCM_SAMPLE_RATE, SCM_SAMPLE_RATE };
        return put_array(o, &range, sizeof range, 1);
    }
    case kAudioDevicePropertyIsHidden: return put_u32(o, d == kFeed ? 1 : 0);
    case kAudioDevicePropertyPreferredChannelsForStereo: {
        UInt32 channels[2] = { 1, 1 };   // mono: both sides are channel 1
        return put(o, channels, sizeof channels);
    }
    case kAudioDevicePropertyPreferredChannelLayout: {
        AudioChannelLayout layout;
        memset(&layout, 0, sizeof layout);
        layout.mChannelLayoutTag = kAudioChannelLayoutTag_UseChannelDescriptions;
        layout.mNumberChannelDescriptions = 1;
        layout.mChannelDescriptions[0].mChannelLabel = kAudioChannelLabel_Mono;
        return put(o, &layout, (UInt32)(offsetof(AudioChannelLayout, mChannelDescriptions) + sizeof(AudioChannelDescription)));
    }
    case kAudioDevicePropertyZeroTimeStampPeriod: return put_u32(o, SCM_ZERO_TIMESTAMP_PERIOD);
    case kAudioDevicePropertyClockIsStable: return put_u32(o, 1);
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus stream_property(int s, const AudioObjectPropertyAddress *a, Out *o) {
    switch (a->mSelector) {
    case kAudioObjectPropertyBaseClass: return put_u32(o, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return put_u32(o, kAudioStreamClassID);
    case kAudioObjectPropertyOwner: return put_u32(o, device_id(s));
    case kAudioObjectPropertyOwnedObjects: return put_ids(o, NULL, 0);
    case kAudioStreamPropertyIsActive:
        return put_u32(o, atomic_load_explicit(&gStreamActive[s], memory_order_relaxed));
    case kAudioStreamPropertyDirection: return put_u32(o, s == kInput ? 1 : 0);   // 1 = input
    case kAudioStreamPropertyTerminalType:
        return put_u32(o, s == kInput ? kAudioStreamTerminalTypeMicrophone : kAudioStreamTerminalTypeLine);
    case kAudioStreamPropertyStartingChannel: return put_u32(o, 1);
    case kAudioStreamPropertyLatency: return put_u32(o, 0);
    case kAudioStreamPropertyVirtualFormat:
    case kAudioStreamPropertyPhysicalFormat: {
        AudioStreamBasicDescription f = stream_format();
        return put(o, &f, sizeof f);
    }
    case kAudioStreamPropertyAvailableVirtualFormats:
    case kAudioStreamPropertyAvailablePhysicalFormats: {
        AudioStreamRangedDescription r;
        memset(&r, 0, sizeof r);
        r.mFormat = stream_format();
        r.mSampleRateRange.mMinimum = SCM_SAMPLE_RATE;
        r.mSampleRateRange.mMaximum = SCM_SAMPLE_RATE;
        return put_array(o, &r, sizeof r, 1);
    }
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus object_property(AudioObjectID object, const AudioObjectPropertyAddress *a,
                                UInt32 qsize, const void *q, Out *o) {
    if (!a) return kAudioHardwareIllegalOperationError;
    if (object == kAudioObjectPlugInObject) return plugin_property(a, qsize, q, o);
    int d = device_index(object);
    if (d >= 0) return device_property(d, a, o);
    int s = stream_index(object);
    if (s >= 0) return stream_property(s, a, o);
    return kAudioHardwareBadObjectError;
}

// ---------------------------------------------------------------- interface

static AudioServerPlugInDriverInterface gInterface;
static AudioServerPlugInDriverInterface *gInterfacePtr = &gInterface;
static AudioServerPlugInDriverRef const gDriverRef = &gInterfacePtr;

static HRESULT SCM_QueryInterface(void *inDriver, REFIID inUUID, LPVOID *outInterface) {
    if (inDriver != gDriverRef || !outInterface) return kAudioHardwareBadObjectError;
    CFUUIDRef requested = CFUUIDCreateFromUUIDBytes(NULL, inUUID);
    if (!requested) return kAudioHardwareIllegalOperationError;
    HRESULT result = E_NOINTERFACE;
    if (CFEqual(requested, IUnknownUUID) || CFEqual(requested, kAudioServerPlugInDriverInterfaceUUID)) {
        atomic_fetch_add(&gRefCount, 1);
        *outInterface = gDriverRef;
        result = S_OK;
    }
    CFRelease(requested);
    return result;
}

static ULONG SCM_AddRef(void *inDriver) {
    if (inDriver != gDriverRef) return 0;
    return atomic_fetch_add(&gRefCount, 1) + 1;
}

static ULONG SCM_Release(void *inDriver) {
    if (inDriver != gDriverRef) return 0;
    UInt32 current = atomic_load(&gRefCount);
    while (current > 0 && !atomic_compare_exchange_weak(&gRefCount, &current, current - 1)) {}
    return current > 0 ? current - 1 : 0;
}

static OSStatus SCM_Initialize(AudioServerPlugInDriverRef inDriver, AudioServerPlugInHostRef inHost) {
    if (inDriver != gDriverRef) return kAudioHardwareBadObjectError;
    gHost = inHost;
    mach_timebase_info_data_t timebase;
    mach_timebase_info(&timebase);
    Float64 hostTicksPerSecond = 1e9 * (Float64)timebase.denom / (Float64)timebase.numer;
    gHostTicksPerFrame = hostTicksPerSecond / SCM_SAMPLE_RATE;
    gAnchorHostTime = mach_absolute_time();
    scm_ring_init(scm_ring_shared());
    return kAudioHardwareNoError;
}

static OSStatus SCM_CreateDevice(AudioServerPlugInDriverRef inDriver, CFDictionaryRef inDescription,
                                 const AudioServerPlugInClientInfo *inClientInfo, AudioObjectID *outDeviceObjectID) {
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus SCM_DestroyDevice(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID) {
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus SCM_AddDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                    const AudioServerPlugInClientInfo *inClientInfo) {
    if (inDriver != gDriverRef || device_index(inDeviceObjectID) < 0) return kAudioHardwareBadObjectError;
    return kAudioHardwareNoError;
}

static OSStatus SCM_RemoveDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                       const AudioServerPlugInClientInfo *inClientInfo) {
    if (inDriver != gDriverRef || device_index(inDeviceObjectID) < 0) return kAudioHardwareBadObjectError;
    return kAudioHardwareNoError;
}

static OSStatus SCM_PerformDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                     UInt64 inChangeAction, void *inChangeInfo) {
    return kAudioHardwareNoError;   // the configuration never changes
}

static OSStatus SCM_AbortDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                                   UInt64 inChangeAction, void *inChangeInfo) {
    return kAudioHardwareNoError;
}

static Boolean SCM_HasProperty(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID,
                               const AudioObjectPropertyAddress *inAddress) {
    if (inDriver != gDriverRef) return false;
    Out sizeOnly = { NULL, 0, 0 };
    return object_property(inObjectID, inAddress, 0, NULL, &sizeOnly) == kAudioHardwareNoError;
}

static OSStatus SCM_IsPropertySettable(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID,
                                       pid_t inClientProcessID, const AudioObjectPropertyAddress *inAddress,
                                       Boolean *outIsSettable) {
    if (inDriver != gDriverRef || !outIsSettable) return kAudioHardwareBadObjectError;
    Out sizeOnly = { NULL, 0, 0 };
    OSStatus status = object_property(inObjectID, inAddress, 0, NULL, &sizeOnly);
    if (status != kAudioHardwareNoError) return status;
    Boolean settable = false;
    if (device_index(inObjectID) >= 0) {
        settable = inAddress->mSelector == kAudioDevicePropertyNominalSampleRate;
    } else if (stream_index(inObjectID) >= 0) {
        settable = inAddress->mSelector == kAudioStreamPropertyIsActive
            || inAddress->mSelector == kAudioStreamPropertyVirtualFormat
            || inAddress->mSelector == kAudioStreamPropertyPhysicalFormat;
    }
    *outIsSettable = settable;
    return kAudioHardwareNoError;
}

static OSStatus SCM_GetPropertyDataSize(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID,
                                        pid_t inClientProcessID, const AudioObjectPropertyAddress *inAddress,
                                        UInt32 inQualifierDataSize, const void *inQualifierData, UInt32 *outDataSize) {
    if (inDriver != gDriverRef || !outDataSize) return kAudioHardwareBadObjectError;
    Out sizeOnly = { NULL, 0, 0 };
    OSStatus status = object_property(inObjectID, inAddress, inQualifierDataSize, inQualifierData, &sizeOnly);
    if (status == kAudioHardwareNoError) *outDataSize = sizeOnly.size;
    return status;
}

static OSStatus SCM_GetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID,
                                    pid_t inClientProcessID, const AudioObjectPropertyAddress *inAddress,
                                    UInt32 inQualifierDataSize, const void *inQualifierData, UInt32 inDataSize,
                                    UInt32 *outDataSize, void *outData) {
    if (inDriver != gDriverRef || !outDataSize || !outData) return kAudioHardwareBadObjectError;
    Out out = { outData, inDataSize, 0 };
    OSStatus status = object_property(inObjectID, inAddress, inQualifierDataSize, inQualifierData, &out);
    if (status == kAudioHardwareNoError) *outDataSize = out.size;
    return status;
}

static OSStatus SCM_SetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID,
                                    pid_t inClientProcessID, const AudioObjectPropertyAddress *inAddress,
                                    UInt32 inQualifierDataSize, const void *inQualifierData, UInt32 inDataSize,
                                    const void *inData) {
    if (inDriver != gDriverRef || !inAddress) return kAudioHardwareBadObjectError;
    if (device_index(inObjectID) >= 0 && inAddress->mSelector == kAudioDevicePropertyNominalSampleRate) {
        if (inDataSize != sizeof(Float64) || !inData) return kAudioHardwareBadPropertySizeError;
        // Only 48 kHz exists; setting it is a no-op, anything else is refused.
        return *(const Float64 *)inData == SCM_SAMPLE_RATE ? kAudioHardwareNoError : kAudioHardwareIllegalOperationError;
    }
    int s = stream_index(inObjectID);
    if (s >= 0) {
        switch (inAddress->mSelector) {
        case kAudioStreamPropertyIsActive:
            if (inDataSize != sizeof(UInt32) || !inData) return kAudioHardwareBadPropertySizeError;
            atomic_store_explicit(&gStreamActive[s], *(const UInt32 *)inData ? 1 : 0, memory_order_relaxed);
            return kAudioHardwareNoError;
        case kAudioStreamPropertyVirtualFormat:
        case kAudioStreamPropertyPhysicalFormat:
            if (inDataSize != sizeof(AudioStreamBasicDescription) || !inData) return kAudioHardwareBadPropertySizeError;
            return format_is_ours(inData) ? kAudioHardwareNoError : kAudioDeviceUnsupportedFormatError;
        default:
            break;
        }
    }
    Out sizeOnly = { NULL, 0, 0 };
    OSStatus exists = object_property(inObjectID, inAddress, inQualifierDataSize, inQualifierData, &sizeOnly);
    return exists == kAudioHardwareNoError ? kAudioHardwareUnsupportedOperationError : exists;
}

static OSStatus SCM_StartIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID) {
    int d = device_index(inDeviceObjectID);
    if (inDriver != gDriverRef || d < 0) return kAudioHardwareBadObjectError;
    atomic_fetch_add(&gIOCount[d], 1);
    return kAudioHardwareNoError;
}

static OSStatus SCM_StopIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID) {
    int d = device_index(inDeviceObjectID);
    if (inDriver != gDriverRef || d < 0) return kAudioHardwareBadObjectError;
    UInt32 current = atomic_load(&gIOCount[d]);
    while (current > 0 && !atomic_compare_exchange_weak(&gIOCount[d], &current, current - 1)) {}
    return kAudioHardwareNoError;
}

/// The latest zero crossing at or before now. Pure arithmetic from the fixed
/// anchor: identical for both devices, lock-free, no state to update.
static OSStatus SCM_GetZeroTimeStamp(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                     UInt32 inClientID, Float64 *outSampleTime, UInt64 *outHostTime, UInt64 *outSeed) {
    if (inDriver != gDriverRef || device_index(inDeviceObjectID) < 0) return kAudioHardwareBadObjectError;
    Float64 ticksPerPeriod = gHostTicksPerFrame * (Float64)SCM_ZERO_TIMESTAMP_PERIOD;
    UInt64 now = mach_absolute_time();
    UInt64 elapsed = now > gAnchorHostTime ? now - gAnchorHostTime : 0;
    UInt64 periods = ticksPerPeriod > 0 ? (UInt64)((Float64)elapsed / ticksPerPeriod) : 0;
    *outSampleTime = (Float64)periods * (Float64)SCM_ZERO_TIMESTAMP_PERIOD;
    *outHostTime = gAnchorHostTime + (UInt64)((Float64)periods * ticksPerPeriod);
    *outSeed = 1;
    return kAudioHardwareNoError;
}

static OSStatus SCM_WillDoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                      UInt32 inClientID, UInt32 inOperationID, Boolean *outWillDo,
                                      Boolean *outWillDoInPlace) {
    int d = device_index(inDeviceObjectID);
    if (inDriver != gDriverRef || d < 0) return kAudioHardwareBadObjectError;
    Boolean willDo = (d == kInput && inOperationID == kAudioServerPlugInIOOperationReadInput)
                  || (d == kFeed && inOperationID == kAudioServerPlugInIOOperationWriteMix);
    if (outWillDo) *outWillDo = willDo;
    if (outWillDoInPlace) *outWillDoInPlace = true;
    return kAudioHardwareNoError;
}

static OSStatus SCM_BeginIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                     UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize,
                                     const AudioServerPlugInIOCycleInfo *inIOCycleInfo) {
    if (inDriver != gDriverRef || device_index(inDeviceObjectID) < 0) return kAudioHardwareBadObjectError;
    return kAudioHardwareNoError;
}

static OSStatus SCM_DoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                  AudioObjectID inStreamObjectID, UInt32 inClientID, UInt32 inOperationID,
                                  UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo *inIOCycleInfo,
                                  void *ioMainBuffer, void *ioSecondaryBuffer) {
    int d = device_index(inDeviceObjectID);
    if (inDriver != gDriverRef || d < 0 || stream_index(inStreamObjectID) != d) return kAudioHardwareBadObjectError;
    if (!ioMainBuffer || !inIOCycleInfo) return kAudioHardwareNoError;
    if (d == kInput && inOperationID == kAudioServerPlugInIOOperationReadInput) {
        scm_ring_read(scm_ring_shared(), (int64_t)llround(inIOCycleInfo->mInputTime.mSampleTime),
                      (float *)ioMainBuffer, inIOBufferFrameSize);
    } else if (d == kFeed && inOperationID == kAudioServerPlugInIOOperationWriteMix) {
        scm_ring_write(scm_ring_shared(), (int64_t)llround(inIOCycleInfo->mOutputTime.mSampleTime),
                       (const float *)ioMainBuffer, inIOBufferFrameSize);
    }
    return kAudioHardwareNoError;
}

static OSStatus SCM_EndIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID,
                                   UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize,
                                   const AudioServerPlugInIOCycleInfo *inIOCycleInfo) {
    if (inDriver != gDriverRef || device_index(inDeviceObjectID) < 0) return kAudioHardwareBadObjectError;
    return kAudioHardwareNoError;
}

static AudioServerPlugInDriverInterface gInterface = {
    NULL,
    SCM_QueryInterface,
    SCM_AddRef,
    SCM_Release,
    SCM_Initialize,
    SCM_CreateDevice,
    SCM_DestroyDevice,
    SCM_AddDeviceClient,
    SCM_RemoveDeviceClient,
    SCM_PerformDeviceConfigurationChange,
    SCM_AbortDeviceConfigurationChange,
    SCM_HasProperty,
    SCM_IsPropertySettable,
    SCM_GetPropertyDataSize,
    SCM_GetPropertyData,
    SCM_SetPropertyData,
    SCM_StartIO,
    SCM_StopIO,
    SCM_GetZeroTimeStamp,
    SCM_WillDoIOOperation,
    SCM_BeginIOOperation,
    SCM_DoIOOperation,
    SCM_EndIOOperation,
};

SCM_EXPORT void *SidecastMic_Create(CFAllocatorRef allocator, CFUUIDRef requestedTypeUUID) {
    (void)allocator;
    if (requestedTypeUUID && CFEqual(requestedTypeUUID, kAudioServerPlugInTypeUUID)) return gDriverRef;
    return NULL;
}
