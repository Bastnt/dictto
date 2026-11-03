"""Capture a known signal from the installed Dictto Microphone on a Windows test machine."""

import argparse
import array
import ctypes as C
from ctypes import wintypes as W
import math
import sys
import time

RATE = 48000
WRITE = 0x8000A004
GET_ACTIVE = 0x8000A010
FRAMES = RATE // 10
TONES = (530, 1310, 790)


class WaveFormat(C.Structure):
    _pack_ = 1
    _fields_ = [("tag", W.WORD), ("channels", W.WORD), ("rate", W.DWORD),
                ("bytes_per_sec", W.DWORD), ("align", W.WORD),
                ("bits", W.WORD), ("extra", W.WORD)]


class InputCaps(C.Structure):
    _fields_ = [("mid", W.WORD), ("pid", W.WORD), ("version", W.DWORD),
                ("name", W.WCHAR * 32), ("formats", W.DWORD),
                ("channels", W.WORD), ("reserved", W.WORD)]


class WaveHeader(C.Structure):
    _fields_ = [("data", C.c_void_p), ("length", W.DWORD), ("recorded", W.DWORD),
                ("user", C.c_size_t), ("flags", W.DWORD), ("loops", W.DWORD),
                ("next", C.c_void_p), ("reserved", C.c_size_t)]


def tones():
    return [array.array("h", (int(10000 * math.sin(2 * math.pi * hz * i / RATE))
                              for i in range(FRAMES))) for hz in TONES]


def verify(samples):
    # Look for all three tones at their original spacing. Quadrature allows an
    # arbitrary capture phase and the normalized score tolerates endpoint gain.
    n = FRAMES // 2
    references = [[complex(math.cos(2 * math.pi * hz * j / RATE),
                           math.sin(2 * math.pi * hz * j / RATE))
                   for j in range(0, n, 8)] for hz in TONES]
    for start in range(0, len(samples) - (len(TONES)-1)*FRAMES - n + 1, RATE // 200):
        for k, ref in enumerate(references):
            pos = start + k * FRAMES
            window = samples[pos:pos+n:8]
            energy = sum(x*x for x in window)
            if not energy or 2*abs(sum(x*r for x, r in zip(window, ref)))**2 < 0.8*energy*len(window):
                break
        else:
            return
    raise ValueError("capture did not contain the three transmitted tones in order")


def bind(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype = result
    fn.argtypes = args
    return fn


def checked(code, action):
    if code:
        raise RuntimeError(f"{action} returned WinMM error {code}")


def capture():
    kernel = C.WinDLL("kernel32", use_last_error=True)
    mm = C.WinDLL("winmm")
    devices = bind(mm, "waveInGetNumDevs", W.UINT)
    caps = bind(mm, "waveInGetDevCapsW", W.UINT, C.c_size_t, C.POINTER(InputCaps), W.UINT)
    matches = []
    for i in range(devices()):
        info = InputCaps()
        if not caps(i, C.byref(info), C.sizeof(info)) and info.name.lower().startswith("dictto microphone"):
            matches.append((i, info.name))
    if len(matches) != 1:
        raise RuntimeError(f"expected one Dictto Microphone capture endpoint, found {matches}")
    create = bind(kernel, "CreateFileW", W.HANDLE, W.LPCWSTR, W.DWORD, W.DWORD,
                  C.c_void_p, W.DWORD, W.DWORD, W.HANDLE)
    ioctl = bind(kernel, "DeviceIoControl", W.BOOL, W.HANDLE, W.DWORD, C.c_void_p,
                 W.DWORD, C.c_void_p, W.DWORD, C.POINTER(W.DWORD), C.c_void_p)
    close = bind(kernel, "CloseHandle", W.BOOL, W.HANDLE)
    feed = create(r"\\.\DicttoMicFeed", 0x40000000, 0, None, 3, 0, None)
    if feed == C.c_void_p(-1).value:
        raise C.WinError(C.get_last_error())

    mic = W.HANDLE()
    fmt = WaveFormat(1, 1, RATE, 2*RATE, 2, 16, 0)
    pcm = C.create_string_buffer(2 * RATE * 3)
    header = WaveHeader(C.addressof(pcm), len(pcm))
    ptr, size = C.byref(header), C.sizeof(header)
    open_input = bind(mm, "waveInOpen", W.UINT, C.POINTER(W.HANDLE), W.UINT,
                      C.POINTER(WaveFormat), C.c_size_t, C.c_size_t, W.DWORD)
    def header_call(name):
        fn = bind(mm, name, W.UINT, W.HANDLE, C.POINTER(WaveHeader), W.UINT)
        return fn(mic, ptr, size)
    def device_call(name):
        return bind(mm, name, W.UINT, W.HANDLE)(mic)

    prepared = False
    try:
        checked(open_input(C.byref(mic), matches[0][0], C.byref(fmt), 0, 0, 0), "open")
        checked(header_call("waveInPrepareHeader"), "prepare")
        prepared = True
        checked(header_call("waveInAddBuffer"), "add capture buffer")
        checked(device_call("waveInStart"), "start")
        until = time.monotonic() + 2
        active, returned = W.DWORD(), W.DWORD()
        while not active.value:
            if not ioctl(feed, GET_ACTIVE, None, 0, C.byref(active), C.sizeof(active),
                         C.byref(returned), None):
                raise C.WinError(C.get_last_error())
            if time.monotonic() >= until:
                raise RuntimeError("capture endpoint did not enter RUN")
            time.sleep(0.005)
        until = time.monotonic() + 2
        for tone in tones():
            data = tone.tobytes()
            written = W.DWORD()
            while not ioctl(feed, WRITE, data, len(data), None, 0, C.byref(written), None):
                error = C.get_last_error()
                if error != 170 or time.monotonic() >= until:
                    raise C.WinError(error)
                time.sleep(0.005)
        until = time.monotonic() + 5
        while not header.flags & 1:
            if time.monotonic() >= until:
                raise RuntimeError("capture endpoint did not return its buffer")
            time.sleep(0.01)
        verify(array.array("h", pcm.raw[:header.recorded]))
        print(f"PASS: all three tones recovered through {matches[0][1]} at {RATE} Hz")
    finally:
        if mic.value:
            device_call("waveInReset")
            if prepared:
                header_call("waveInUnprepareHeader")
            device_call("waveInClose")
        close(feed)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    if parser.parse_args().self_test:
        parts = tones()
        verify(array.array("h", [0]*1234) + parts[0] + parts[1] + parts[2])
        try:
            verify(parts[2] + parts[1] + parts[0])
        except ValueError:
            print("Signal detector self-test passed.")
        else:
            sys.exit("reversed tones were accepted")
    else:
        capture()
