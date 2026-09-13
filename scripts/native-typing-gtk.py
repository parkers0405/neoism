#!/usr/bin/env python3
"""Observer-only GTK4 Entry fixture. No synthetic events or programmatic text edits."""
import json
import os
import pathlib
import sys
import time
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib

run = pathlib.Path(sys.argv[1])
app = Gtk.Application(application_id='org.neoism.NativeTypingAcceptance')
events = []

def activate(app):
    window = Gtk.ApplicationWindow(application=app, title='Neoism native typing acceptance')
    window.set_default_size(1000, 400)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=24)
    box.set_margin_top(40)
    box.set_margin_start(30)
    box.set_margin_end(30)
    box.append(Gtk.Label(label='Isolated GTK4 Entry — ordinary keyboard and clipboard paths'))
    entry = Gtk.Entry()
    box.append(entry)
    window.set_child(box)
    controller = Gtk.EventControllerKey()
    controller.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
    def record(kind, keyval, keycode, state):
        events.append(dict(type=kind, keyval=keyval, keycode=keycode, state=int(state)))
        del events[:-500]
        return False
    controller.connect('key-pressed', lambda c, v, k, s: record('press', v, k, s))
    controller.connect('key-released', lambda c, v, k, s: record('release', v, k, s))
    window.add_controller(controller)
    def report():
        focus = window.get_focus()
        data = dict(seq=time.monotonic_ns(), pid=os.getpid(), focused=window.is_active(),
                    entryFocused=focus is not None and (focus == entry or focus.is_ancestor(entry)),
                    value=entry.get_text(), events=events)
        temporary = run / 'report.tmp'
        temporary.write_text(json.dumps(data, ensure_ascii=False))
        temporary.replace(run / 'report.json')
        return True
    GLib.timeout_add(50, report)
    window.present()
    entry.grab_focus()
app.connect('activate', activate)
app.run([])
