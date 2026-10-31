#!/usr/bin/env python3
"""Stand-ins for the desktop services the Linux host talks to, for
scripts/linux-e2e.sh (a headless GNOME has no shell, so no notification
daemon and no tray):

- org.freedesktop.Notifications: logs every notification and writes the
  pairing code it carries to CODE_FILE, which the probe reads (--code-file);
- org.kde.StatusNotifierWatcher: accepts the host's tray icon and logs it.

Usage: linux-e2e-desktop.py CODE_FILE   (runs until killed; needs python3-gi)
"""

import re
import sys

from gi.repository import Gio, GLib

CODE_FILE = sys.argv[1]

NOTIFICATIONS_XML = """
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="Notify">
      <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
      <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
      <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/>
    </method>
    <method name="CloseNotification"><arg type="u" direction="in"/></method>
    <method name="GetCapabilities"><arg type="as" direction="out"/></method>
    <method name="GetServerInformation">
      <arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/>
      <arg type="s" direction="out"/>
    </method>
  </interface>
</node>
"""

WATCHER_XML = """
<node>
  <interface name="org.kde.StatusNotifierWatcher">
    <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
    <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
    <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
    <property name="ProtocolVersion" type="i" access="read"/>
  </interface>
</node>
"""

items = []
next_id = [1]


def log(line):
    print(f"desktop: {line}", flush=True)


def on_notifications(connection, sender, path, interface, method, params, invocation):
    if method == "Notify":
        app, _replaces, _icon, summary, body, _actions, hints, timeout = params.unpack()
        log(f"notification from {app}: {summary!r} / {body!r} (urgency {hints.get('urgency')}, {timeout} ms)")
        digits = re.sub(r"\D", "", re.search(r"Enter ([\d ]+)", body).group(1)) if "Enter" in body else ""
        if len(digits) == 6:
            with open(CODE_FILE, "w") as f:
                f.write(digits)
            log(f"pairing code {digits} written to {CODE_FILE}")
        nid = next_id[0]
        next_id[0] += 1
        invocation.return_value(GLib.Variant("(u)", (nid,)))
    elif method == "CloseNotification":
        log(f"notification {params.unpack()[0]} closed")
        invocation.return_value(None)
    elif method == "GetCapabilities":
        invocation.return_value(GLib.Variant("(as)", (["body"],)))
    else:
        invocation.return_value(GLib.Variant("(ssss)", ("e2e", "sidecast", "1", "1.2")))


def on_watcher(connection, sender, path, interface, method, params, invocation):
    if method == "RegisterStatusNotifierItem":
        service = params.unpack()[0]
        items.append(service)
        log(f"tray item registered: {service} (from {sender})")
    invocation.return_value(None)


def on_watcher_property(connection, sender, path, interface, name):
    if name == "RegisteredStatusNotifierItems":
        return GLib.Variant("as", items)
    if name == "IsStatusNotifierHostRegistered":
        return GLib.Variant("b", True)
    return GLib.Variant("i", 0)


def on_bus(connection, _name):
    notifications = Gio.DBusNodeInfo.new_for_xml(NOTIFICATIONS_XML).interfaces[0]
    watcher = Gio.DBusNodeInfo.new_for_xml(WATCHER_XML).interfaces[0]
    connection.register_object("/org/freedesktop/Notifications", notifications, on_notifications, None, None)
    connection.register_object("/StatusNotifierWatcher", watcher, on_watcher, on_watcher_property, None)


def main():
    Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.Notifications", Gio.BusNameOwnerFlags.NONE,
                     on_bus, lambda *_: log("owns org.freedesktop.Notifications"), None)
    Gio.bus_own_name(Gio.BusType.SESSION, "org.kde.StatusNotifierWatcher", Gio.BusNameOwnerFlags.NONE,
                     None, lambda *_: log("owns org.kde.StatusNotifierWatcher"), None)
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
