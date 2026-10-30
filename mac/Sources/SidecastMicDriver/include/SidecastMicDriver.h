#ifndef SIDECAST_MIC_DRIVER_H
#define SIDECAST_MIC_DRIVER_H

// SidecastMic.driver — an AudioServerPlugIn with two devices on one clock:
//   "Sidecast Microphone" (input, visible)      UID SidecastMic_Input
//   "Sidecast Mic Feed"   (output, hidden)      UID SidecastMic_Feed
// Whatever the Sidecast app plays into the feed comes out of the microphone.
// Both are 48 kHz, 1 channel, float32.

#include <CoreFoundation/CoreFoundation.h>

#include "SidecastRing.h"   // umbrella header of the SwiftPM module: re-export the ring

#ifdef __cplusplus
extern "C" {
#endif

/// Bundle version (CFBundleVersion of SidecastMic.driver). Bump on any driver
/// change: the app offers "Update available" when it differs from the installed one.
#define SCM_DRIVER_VERSION "1.0.0"
/// CFPlugIn factory UUID (Info.plist CFPlugInFactories).
#define SCM_FACTORY_UUID "465D62E7-604D-4113-A5B0-5C6B4C9914B6"

#define SCM_INPUT_DEVICE_UID "SidecastMic_Input"
#define SCM_FEED_DEVICE_UID "SidecastMic_Feed"
#define SCM_INPUT_DEVICE_NAME "Sidecast Microphone"
#define SCM_FEED_DEVICE_NAME "Sidecast Mic Feed"
#define SCM_SAMPLE_RATE 48000
#define SCM_CHANNELS 1
/// Frames between zero time stamps (HAL minimum is 10923).
#define SCM_ZERO_TIMESTAMP_PERIOD 16384

/// Object IDs (the plug-in itself is kAudioObjectPlugInObject = 1).
enum {
    SCM_OBJECT_INPUT_DEVICE = 2,
    SCM_OBJECT_INPUT_STREAM = 3,
    SCM_OBJECT_FEED_DEVICE = 4,
    SCM_OBJECT_FEED_STREAM = 5,
};

/// CFPlugIn factory (named in Info.plist CFPlugInFactories). Returns the
/// AudioServerPlugInDriverRef for kAudioServerPlugInTypeUUID, else NULL.
void *SidecastMic_Create(CFAllocatorRef allocator, CFUUIDRef requestedTypeUUID);

#ifdef __cplusplus
}
#endif

#endif
