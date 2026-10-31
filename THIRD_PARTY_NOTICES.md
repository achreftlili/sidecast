# Third-party notices

Sidecast ports code and techniques from **SideScreen** (https://github.com/tranvuongquocdat/SideScreen), used under the MIT License:

```
MIT License

Copyright (c) 2025 Side Screen

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Rust crates (`rust/`)

The Rust host, core and probe use these crates from crates.io (and their
dependencies); each crate's license text ships in its source. All are
permissively licensed (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0,
or dual-licensed with one of those); none is used under a copyleft license.

| Crate | License | Used for |
|---|---|---|
| tokio | MIT | async runtime of the server and the probe |
| rustls, tokio-rustls | Apache-2.0 OR ISC OR MIT / MIT OR Apache-2.0 | TLS 1.2/1.3 |
| ring (rustls' crypto provider) | Apache-2.0 AND ISC | TLS cryptography |
| rcgen | MIT OR Apache-2.0 | the host's self-signed P-256 certificate |
| mdns-sd | Apache-2.0 OR MIT | DNS-SD advertisement of `_sidecast._tcp` |
| if-addrs | MIT OR BSD-3-Clause | local interface addresses |
| gethostname | Apache-2.0 | the computer's name |
| serde, serde_json | MIT OR Apache-2.0 | JSON messages and files |
| base64 | MIT OR Apache-2.0 | Base64 in JSON |
| sha2, hmac | MIT OR Apache-2.0 | token hash, pairing proof |
| subtle | BSD-3-Clause | constant-time comparison |
| getrandom | MIT OR Apache-2.0 | system randomness |
| unicode-segmentation | MIT OR Apache-2.0 | splitting typed text between graphemes |
| log | MIT OR Apache-2.0 | logging facade |
| openh264, openh264-sys2 | BSD-2-Clause | software H.264 encoder (optional `openh264` feature) and the probe's decoder |
| windows, windows-core, windows-implement, windows-interface, windows-result, windows-strings, windows-collections, windows-future, windows-numerics, windows-threading | MIT OR Apache-2.0 | the Windows host's bindings to Win32, Direct3D 11 / DXGI, Media Foundation, WASAPI and the shell (Windows builds only) |
| zbus, zvariant | MIT | Linux host: D-Bus (Mutter, portals, logind, notifications, the tray icon) |
| ashpd | MIT | Linux host: the xdg-desktop-portal ScreenCast and RemoteDesktop |
| pipewire, libspa (and their `-sys` bindings, built with bindgen) | MIT | Linux host: screen capture and the virtual microphone |
| async-io, futures-lite | Apache-2.0 OR MIT | Linux host: running D-Bus calls on plain threads |
| gstreamer, gstreamer-app (and glib, gio bindings) | MIT OR Apache-2.0 / MIT | Linux host: hardware H.264 (optional `gstreamer` feature) |

### Not included: drivers the Windows host can use

Sidecast does not include or distribute any driver. On Windows it uses, when
the user installed them, the **Virtual Display Driver**
(https://github.com/VirtualDrivers/Virtual-Display-Driver, MIT License, signed
releases) for a virtual monitor, and a virtual audio cable such as **VB-CABLE**
(https://vb-audio.com/Cable/, VB-Audio's own license) for the phone's
microphone. CI downloads the driver's official release and **nefcon**
(https://github.com/nefarius/nefcon, MIT License) to install it on a throwaway
test runner; neither ends up in what Sidecast builds.

### Linux system libraries

The Linux host links dynamically to the system's **libpipewire** (MIT) and,
with the `gstreamer` feature, to **GStreamer** and **GLib** (LGPL-2.1 or
later). The LGPL libraries are only linked dynamically, never bundled or
modified, so users can replace them; the encoder elements used are the
VA-API and NVENC plugins of GStreamer (LGPL), never x264 or another GPL
element. D-Bus is spoken by zbus in Rust (no libdbus). Building needs
libclang (Apache-2.0 WITH LLVM-exception) for the PipeWire bindings; it is
not part of the binary.

### OpenH264

With the `openh264` cargo feature (on by default in `sidecast-host` and
`sidecast-probe`), Cisco's OpenH264 library is compiled from source into the
binary:

```
Copyright (c) 2013, Cisco Systems
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice, this
  list of conditions and the following disclaimer in the documentation and/or
  other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

**Patent note.** The BSD license covers OpenH264's copyright only. H.264 is
covered by patents (licensed through Via LA, formerly MPEG LA). Cisco pays
those royalties for the OpenH264 *binaries it distributes itself*; a library
compiled from source, as this feature does, is not covered by Cisco's patent
arrangement. Sidecast distributes no binaries (everyone builds from source,
DECISIONS Q33), but anyone who distributes a binary built with the `openh264`
feature is responsible for any H.264 patent licensing that applies where they
distribute it. Build with `--no-default-features` to leave OpenH264 out; the
platform hosts prefer the computer's hardware encoders, whose H.264/HEVC
licensing comes with the hardware or operating system. The Windows host uses
OpenH264 only after the GPU's Media Foundation encoder and Microsoft's own
software H.264 encoder, which ships (and is licensed) with Windows.
